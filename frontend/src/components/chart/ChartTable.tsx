import { useEffect, useMemo, useRef, useState, type ReactNode, type CSSProperties, type MouseEventHandler } from "react";
import {
  flexRender, getCoreRowModel, getPaginationRowModel, getSortedRowModel, useReactTable,
  type ColumnDef, type ColumnSizingState, type SortingState,
} from "@tanstack/react-table";
import {
  DndContext, PointerSensor, KeyboardSensor, useSensor, useSensors,
  closestCenter, type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext, horizontalListSortingStrategy, useSortable, arrayMove, sortableKeyboardCoordinates,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { formatValue } from "./format";
import { buildConditional } from "./conditional";
import { isNumericType, type ColumnFormat, type Rule } from "./format-types";
import type { TableState } from "@/api/charts";
import { Button } from "@/components/ui/button";
import { tableCellDrill, type DrillContext } from "./drill";

export interface ResultColumn { name: string; type: string }

function SortableHeader({ id, disabled, children, className, style, onClick }: {
  id: string; disabled?: boolean; children: ReactNode; className?: string;
  style?: CSSProperties; onClick?: MouseEventHandler;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id, disabled });
  // Destructure role out so we can override dnd-kit's "button" with "columnheader".
  // `aria-disabled` goes too: dnd-kit sets it whenever the item can't be DRAGGED, but the header
  // it lands on is still fully operable — click to sort, drag the separator to resize. A pinned
  // column was therefore announced (and its resize handle with it, since the state propagates to
  // descendants) as unavailable while both controls worked.
  const { role: _dndRole, "aria-disabled": _dndDisabled, ...restAttributes } = attributes;
  return (
    <th
      ref={setNodeRef}
      role="columnheader"
      onClick={onClick}
      style={{ ...style, transform: CSS.Translate.toString(transform), transition, opacity: isDragging ? 0.6 : 1 }}
      className={className}
      {...restAttributes}
      {...listeners}
    >
      {children}
    </th>
  );
}

const MIN_COL_WIDTH = 60;
/** px width of the "#" gutter — pinned columns start after it, or they slide underneath. */
const ROW_INDEX_WIDTH = 40;

const fillTemplate = (tpl: string, value: string) => tpl.replace(/\{\{\s*value\s*\}\}/g, value);

const URL_RE = /^https?:\/\/\S+$/i;
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

const linkEl = (href: string, text: string) => (
  <a href={href} target="_blank" rel="noreferrer" className="text-primary underline">{text}</a>
);

// Render a cell's content honouring its "Display as" (link/email/image) and mini-bar
// settings — clean-room of Metabase's column formatting. Falls back to the formatted text.
function renderTableCell(
  raw: unknown, isNum: boolean, fmt: ColumnFormat | undefined, formatted: string, cap: number,
): ReactNode {
  const str = raw == null ? "" : String(raw);
  switch (fmt?.view_as) {
    case "link": {
      const url = fmt.link_url ? fillTemplate(fmt.link_url, str) : str;
      const text = fmt.link_text ? fillTemplate(fmt.link_text, str) : formatted;
      return str === "" ? formatted : linkEl(url, text);
    }
    case "email":
      return str === "" ? formatted : <a href={`mailto:${str}`} className="text-primary underline">{formatted}</a>;
    case "image":
      return str === "" ? null : <img src={str} alt="" className="max-h-10 rounded object-contain" />;
    default:
      break;
  }
  // "Automatic" (the default): auto-link bare URLs / emails in text columns, like Metabase.
  if (!isNum && fmt?.view_as !== "text") {
    const t = str.trim();
    if (URL_RE.test(t)) return linkEl(t, formatted);
    if (EMAIL_RE.test(t)) return <a href={`mailto:${t}`} className="text-primary underline">{formatted}</a>;
  }
  if (fmt?.show_mini_bar && isNum) {
    const n = typeof raw === "number" ? raw : Number(raw);
    if (Number.isFinite(n)) {
      const pct = cap > 0 ? Math.min(100, (Math.abs(n) / cap) * 100) : 0;
      return (
        <div className="flex items-center justify-end gap-2">
          <span>{formatted}</span>
          <div className="relative h-2 w-12 shrink-0 rounded bg-muted">
            <div
              data-testid="mini-bar"
              className="absolute inset-y-0 left-0 rounded"
              style={{ width: `${pct}%`, background: n < 0 ? "var(--destructive)" : "var(--primary)" }}
            />
          </div>
        </div>
      );
    }
  }
  return formatted;
}

export function ChartTable({
  columns, rows, columnSettings = {}, conditionalFormatting = [],
  tableState = {}, onTableStateChange, dimensionColumns, onCellDrill, temporalUnits,
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
  tableState?: TableState;
  onTableStateChange?: (next: TableState) => void;
  dimensionColumns?: string[];
  onCellDrill?: (ctx: DrillContext, x: number, y: number) => void;
  /** dimension -> temporal unit, so a drill on a bucketed column filters the whole bucket rather
   * than the single instant the cell happens to show. Without it a row grouped by month filtered
   * `= 2026-01-01` and under-reported, the same defect the pivot and the cartesian charts had. */
  temporalUnits?: Record<string, string>;
}) {
  const [sorting, setSorting] = useState<SortingState>([]);

  const colIndexByName = useMemo(
    () => Object.fromEntries(columns.map((c, i) => [c.name, i])),
    [columns],
  );
  const cond = useMemo(
    () => buildConditional(rows, colIndexByName, conditionalFormatting),
    [rows, colIndexByName, conditionalFormatting],
  );

  // Per-column extent (largest magnitude) for mini-bar sizing — only for numeric columns
  // that opted in, so the common case stays free.
  const miniBarCaps = useMemo(() => {
    const caps: Record<string, number> = {};
    columns.forEach((c, i) => {
      if (!columnSettings[c.name]?.show_mini_bar || !isNumericType(c.type)) return;
      let cap = 0;
      for (const r of rows) {
        const n = Number((r as unknown[])[i]);
        if (Number.isFinite(n)) cap = Math.max(cap, Math.abs(n));
      }
      caps[c.name] = cap;
    });
    return caps;
  }, [columns, rows, columnSettings]);

  const cols = useMemo<ColumnDef<unknown[]>[]>(
    () =>
      columns.map((c, i) => {
        const isNum = isNumericType(c.type);
        return {
          id: `col_${i}`,
          accessorFn: (row) => {
            const v = (row as unknown[])[i];
            if (isNum && typeof v === "string" && v.trim() !== "" && Number.isFinite(Number(v))) {
              return Number(v);
            }
            return v;
          },
          header: columnSettings[c.name]?.column_title || c.name,
          cell: (ctx) => {
            const v = ctx.getValue();
            const fmt = columnSettings[c.name];
            return renderTableCell(v, isNum, fmt, formatValue(v, c.type, fmt), miniBarCaps[c.name] ?? 0);
          },
          enableSorting: true,
          minSize: MIN_COL_WIDTH,
          meta: { isNum, name: c.name },
        };
      }),
    [columns, columnSettings, miniBarCaps],
  );

  // persisted widths → controlled sizing (reconcile stale keys), seeding a local sizing state
  const persistedSizing = useMemo<ColumnSizingState>(() => {
    const out: ColumnSizingState = {};
    columns.forEach((c, i) => {
      const w = tableState.columnWidths?.[c.name];
      if (typeof w === "number") out[`col_${i}`] = w;
    });
    return out;
  }, [columns, tableState.columnWidths]);

  // local sizing drives live drag feedback; persisted to the parent only on resize end
  const [localSizing, setLocalSizing] = useState<ColumnSizingState>(persistedSizing);
  useEffect(() => { setLocalSizing(persistedSizing); }, [persistedSizing]);

  const columnVisibility = useMemo<Record<string, boolean>>(() => {
    const hidden = new Set(tableState.hiddenColumns ?? []);
    const out: Record<string, boolean> = {};
    columns.forEach((c, i) => { if (hidden.has(c.name)) out[`col_${i}`] = false; });
    return out;
  }, [columns, tableState.hiddenColumns]);

  const pinnedCount = Math.max(0, Math.min(tableState.pinnedLeft ?? 0, columns.length));
  const columnPinning = useMemo(
    () => ({ left: Array.from({ length: pinnedCount }, (_, i) => `col_${i}`) }),
    [pinnedCount],
  );

  const columnOrder = useMemo<string[]>(() => {
    const byName = new Map(columns.map((c, i) => [c.name, `col_${i}`]));
    const ordered = (tableState.columnOrder ?? [])
      .map((name) => byName.get(name))
      .filter((id): id is string => id != null);
    columns.forEach((_, i) => { if (!ordered.includes(`col_${i}`)) ordered.push(`col_${i}`); });
    return ordered;
  }, [columns, tableState.columnOrder]);

  const showRowIndex = !!tableState.rowIndex;
  const paginationEnabled = !!tableState.pagination?.enabled;
  const pageSize = tableState.pagination?.pageSize ?? 25;
  const [pageIndex, setPageIndex] = useState(0);
  useEffect(() => { setPageIndex(0); }, [paginationEnabled, pageSize, rows]);

  const table = useReactTable({
    data: rows,
    columns: cols,
    state: {
      sorting,
      columnSizing: localSizing,
      columnVisibility,
      columnPinning,
      columnOrder,
      ...(paginationEnabled ? { pagination: { pageIndex, pageSize } } : {}),
    },
    // tanstack defaults numeric columns to descending-first; Metabase sorts ascending on the first
    // click for every column type. Two adjacent tables that disagree on what one click means is a
    // worse trap than either convention on its own.
    sortDescFirst: false,
    columnResizeMode: "onChange",
    enableColumnResizing: true,
    onSortingChange: setSorting,
    onColumnSizingChange: setLocalSizing,
    onPaginationChange: (updater) => {
      const next = typeof updater === "function" ? updater({ pageIndex, pageSize }) : updater;
      setPageIndex(next.pageIndex);
    },
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    ...(paginationEnabled ? { getPaginationRowModel: getPaginationRowModel() } : {}),
  });

  // persist widths (by name, pruned to existing columns) only when a resize gesture ends
  const isResizing = table.getState().columnSizingInfo.isResizingColumn;
  const wasResizing = useRef<string | false>(false);
  useEffect(() => {
    if (wasResizing.current && !isResizing) {
      const widths: Record<string, number> = {};
      Object.entries(localSizing).forEach(([id, px]) => {
        const name = columns[Number(id.slice(4))]?.name;  // "col_3" -> 3
        if (name) widths[name] = px as number;
      });
      onTableStateChange?.({ ...tableState, columnWidths: widths });
    }
    wasResizing.current = isResizing;
  }, [isResizing]);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    // Without this, reordering a column is reachable only by dragging with a mouse. Its
    // activator only listens on keydown (Space/Enter start a drag, arrows move it, Space/Enter
    // again drop it) — sorting is wired to onClick alone, on a wholly separate event, so the
    // two never fire off the same keypress.
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  function handleDragEnd(e: DragEndEvent) {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const ids = table.getState().columnOrder.length ? table.getState().columnOrder : columns.map((_, i) => `col_${i}`);
    const from = ids.indexOf(String(active.id));
    const to = ids.indexOf(String(over.id));
    if (from < 0 || to < 0) return;
    const nextIds = arrayMove(ids, from, to);
    const idToName = new Map(columns.map((c, i) => [`col_${i}`, c.name]));
    const names = nextIds.map((id) => idToName.get(id)).filter((n): n is string => n != null);
    onTableStateChange?.({ ...tableState, columnOrder: names });
  }

  const totalRows = rows.length;
  const start = paginationEnabled ? pageIndex * pageSize : 0;

  // Reachable before only by unchecking every box by hand; "Hide all" makes it one click, so a
  // blank grid (no <th>, no <td>) now needs to read as "this is what you asked for", not a bug.
  // "Columns" (not "Display → Columns") because this section also renders in a dashboard card's
  // per-card panel, which has no Display tab to point at — only the CardVizPanel composes the
  // same "Columns" heading, so that name is the one thing true in both places.
  if (table.getVisibleLeafColumns().length === 0) {
    return (
      <p className="p-4 text-sm text-muted-foreground">
        Every column is hidden. Turn one back on under Columns.
      </p>
    );
  }

  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
    <div className="rounded-md border">
      <div className="max-h-[60vh] overflow-auto">
        <table className="w-full border-separate border-spacing-0 text-xs text-foreground/90">
          <thead className="bg-accent">
            {table.getHeaderGroups().map((hg) => (
              <tr key={hg.id} className="text-left text-muted-foreground">
                {showRowIndex && (
                  // Outside the tanstack column model on purpose: the data columns are keyed
                  // `col_<i>` by position, and sorting, pinning, reordering, resizing, drill and
                  // the CSV export all address them by that key. A real column here would shift
                  // every one of those by one.
                  <th scope="col" style={{ width: ROW_INDEX_WIDTH }} className="sticky left-0 top-0 z-30 border-b border-r border-border/50 bg-accent px-3 py-2 text-right font-medium tabular-nums">
                    #
                  </th>
                )}
                <SortableContext items={table.getVisibleLeafColumns().map((c) => c.id)} strategy={horizontalListSortingStrategy}>
                {hg.headers.map((h) => {
                  const isNum = (h.column.columnDef.meta as { isNum?: boolean } | undefined)?.isNum ?? false;
                  const sized = localSizing[h.column.id] != null;
                  const pinned = h.column.getIsPinned() === "left";
                  return (
                    <SortableHeader
                      key={h.id}
                      id={h.column.id}
                      disabled={pinned}
                      onClick={h.column.getToggleSortingHandler()}
                      style={{
                        ...(sized ? { width: h.getSize() } : {}),
                        ...(pinned ? { left: h.column.getStart("left") + (showRowIndex ? ROW_INDEX_WIDTH : 0) } : {}),
                      }}
                      className={`relative sticky top-0 cursor-pointer select-none border-b border-border/50 bg-accent px-3 py-2 font-medium whitespace-nowrap ${
                        pinned ? "z-30 border-r border-border/50" : "z-10"
                      } ${isNum ? "text-right" : "text-left"}`}
                    >
                      {flexRender(h.column.columnDef.header, h.getContext())}
                      {{ asc: " ▲", desc: " ▼" }[h.column.getIsSorted() as string] ?? ""}
                      {h.column.getCanResize() && (
                        <div
                          role="separator"
                          aria-orientation="vertical"
                          aria-label="Resize column"
                          onMouseDown={h.getResizeHandler()}
                          onTouchStart={h.getResizeHandler()}
                          onPointerDown={(e) => e.stopPropagation()}
                          onClick={(e) => e.stopPropagation()}
                          className="absolute right-0 top-0 h-full w-1 cursor-col-resize touch-none select-none hover:bg-primary/40"
                        />
                      )}
                    </SortableHeader>
                  );
                })}
                </SortableContext>
              </tr>
            ))}
          </thead>
          <tbody>
            {table.getRowModel().rows.map((r, i) => {
              const rowColor = cond.rowBg(r.index);
              return (
                <tr key={r.id} className="hover:bg-accent/40" style={rowColor ? { backgroundColor: rowColor } : undefined}>
                  {showRowIndex && (
                    // Position in what is on screen, not `r.index`: after a sort the underlying
                    // row order is meaningless, and the number has to keep counting across pages.
                    <td
                      className="sticky left-0 z-20 border-b border-r border-border/50 px-3 py-1.5 text-right text-muted-foreground tabular-nums"
                      style={{ backgroundColor: rowColor ?? "var(--background)" }}
                    >
                      {(paginationEnabled ? pageIndex * pageSize : 0) + i + 1}
                    </td>
                  )}
                  {r.getVisibleCells().map((cell) => {
                    const meta = cell.column.columnDef.meta as { isNum?: boolean; name?: string } | undefined;
                    const isNum = meta?.isNum ?? false;
                    const bg = cond.bg(cell.getValue(), meta?.name ?? cell.column.id);
                    const sized = localSizing[cell.column.id] != null;
                    const pinned = cell.column.getIsPinned() === "left";
                    // pinned cells need an opaque base so horizontally-scrolled cells don't bleed
                    // through; composite any (translucent) conditional/row color over the page bg.
                    const pinTint = pinned ? (bg ?? rowColor) : null;
                    return (
                      <td
                        key={cell.id}
                        style={{
                          ...(sized ? { width: cell.column.getSize() } : {}),
                          ...(pinned ? { left: cell.column.getStart("left") + (showRowIndex ? ROW_INDEX_WIDTH : 0) } : {}),
                          ...(pinned
                            ? (pinTint ? { background: `linear-gradient(${pinTint}, ${pinTint}), var(--background)` } : {})
                            : (bg ? { backgroundColor: bg } : {})),
                        }}
                        className={`border-b border-border/50 px-3 py-2 ${isNum ? "text-right tabular-nums" : ""} ${
                          pinned ? `sticky z-20 border-r border-border/50${pinTint ? "" : " bg-background"}` : ""
                        } ${onCellDrill && dimensionColumns?.length ? "cursor-pointer" : ""}`}
                        onClick={onCellDrill && dimensionColumns && dimensionColumns.length > 0
                          ? (e) => onCellDrill(
                              tableCellDrill(dimensionColumns, columns.map((c) => c.name), r.original as unknown[], temporalUnits),
                              e.clientX, e.clientY,
                            )
                          : undefined}
                      >
                        {flexRender(cell.column.columnDef.cell, cell.getContext())}
                      </td>
                    );
                  })}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {paginationEnabled && (
        <div className="flex items-center justify-between border-t border-border/50 px-3 py-2 text-xs text-muted-foreground">
          <span>
            {totalRows === 0 ? "No rows" : `Showing ${start + 1}–${Math.min(start + pageSize, totalRows)} of ${totalRows}`}
          </span>
          <div className="flex items-center gap-2">
            <Button size="sm" variant="outline" className="h-7" disabled={!table.getCanPreviousPage()} onClick={() => table.previousPage()}>Prev</Button>
            <span>Page {pageIndex + 1} / {Math.max(1, table.getPageCount())}</span>
            <Button size="sm" variant="outline" className="h-7" disabled={!table.getCanNextPage()} onClick={() => table.nextPage()}>Next</Button>
          </div>
        </div>
      )}
    </div>
    </DndContext>
  );
}
