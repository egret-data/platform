import {
  useEffect, useLayoutEffect, useMemo, useRef, useState,
  type ReactNode, type CSSProperties, type MouseEventHandler,
} from "react";
import {
  columnOrderingFeature, columnPinningFeature, columnResizingFeature, columnSizingFeature,
  columnVisibilityFeature, createCoreRowModel, createPaginatedRowModel, createSortedRowModel,
  flexRender, rowPaginationFeature, rowSortingFeature, tableFeatures, useTable,
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
import { formatValue, withIdentifierDefault } from "./format";
import { buildConditional } from "./conditional";
import { isNumericType, type ColumnFormat, type Rule } from "./format-types";
import type { TableState } from "@/api/charts";
import { Button } from "@/components/ui/button";
import { tableCellDrill, type DrillContext } from "./drill";
import { ResizeHandle, SortButton, ariaSort, MIN_COL_WIDTH } from "./TableHeaderControls";

export interface ResultColumn { name: string; type: string }

/**
 * react-table 9 no longer ships every feature to every table: a table declares the
 * ones it uses and the row models that process them, and the resulting object is
 * also the first type argument to `ColumnDef`. Declared at module scope because the
 * library asks for it to be static — rebuilding it per render would rebuild the
 * table's whole feature pipeline on every pass.
 *
 * This is the exact set v8 got implicitly: sorting, pagination, and the four column
 * concerns this table drives from `tableState` (sizing/resizing, visibility, pinning,
 * ordering). Row *pinning* is deliberately absent — the frozen rows are sliced off
 * the sorted model by hand further down, which is what v8 did too.
 */
const TABLE_FEATURES = tableFeatures({
  rowSortingFeature,
  rowPaginationFeature,
  columnSizingFeature,
  columnResizingFeature,
  columnVisibilityFeature,
  columnPinningFeature,
  columnOrderingFeature,
  coreRowModel: createCoreRowModel(),
  sortedRowModel: createSortedRowModel(),
  paginatedRowModel: createPaginatedRowModel(),
});

function SortableHeader({ id, disabled, children, className, style, onClick, label, sort }: {
  id: string; disabled?: boolean; children: ReactNode; className?: string;
  style?: CSSProperties; onClick?: MouseEventHandler;
  /** The column as it is drawn, pinned as the cell's own accessible name — without it Chrome leaves
   * this columnheader unnamed once the header text is inside a labelled button. See `SortButton`. */
  label: string;
  sort?: "asc" | "desc";
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id, disabled });
  // Destructure role out so we can override dnd-kit's "button" with "columnheader".
  // `aria-disabled` goes too: dnd-kit sets it whenever the item can't be DRAGGED, but the header
  // it lands on is still fully operable — click to sort, drag the separator to resize. A pinned
  // column was therefore announced (and its resize handle with it, since the state propagates to
  // descendants) as unavailable while both controls worked.
  //
  // `aria-roledescription` goes the same way, replaced rather than dropped. dnd-kit's word for it
  // is "sortable", meaning drag-to-REORDER — and the audit measured what that cost: a keyboard user
  // heard "sortable", pressed Enter, and got a drag lift, with sorting reachable only by mouse.
  // Sorting now has its own button in the cell, so the two would be announced with one word between
  // them. "draggable column" is also what dnd-kit's OWN screen-reader instructions on this context
  // already say ("To pick up a draggable item, press the space bar"), so this makes the cell agree
  // with the sentence read out beside it rather than introducing a third vocabulary.
  const {
    role: _dndRole, "aria-disabled": _dndDisabled, "aria-roledescription": _dndRoleDesc,
    ...restAttributes
  } = attributes;
  return (
    <th
      ref={setNodeRef}
      role="columnheader"
      aria-label={label}
      aria-roledescription="draggable column"
      // The direction used to be carried ONLY by a " ▲"/" ▼" glyph appended to the text, which a
      // screen reader has no way to interpret. `aria-sort` is the attribute that means it.
      aria-sort={ariaSort(sort)}
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

/** px width of the "#" gutter — pinned columns start after it, or they slide underneath. */
const ROW_INDEX_WIDTH = 40;

/**
 * THE BODY'S STACKING LADDER, which the row freeze turned from a list into a grid.
 *
 * A cell can now be sticky on two independent axes: a frozen row's cell is stuck in y and free in
 * x, a pinned column's cell is stuck in x and free in y. The two are DISJOINT as sets — one lives
 * only in the non-pinned columns, the other only in the non-frozen rows — so neither of them ever
 * covers the other and the square where the two freezes cross belongs to neither. Only the cell
 * that is both can settle that square, and it has to out-rank both single-axis layers to do it.
 *
 * WHY `CORNER` STOPS SHORT OF THE HEADER'S 30. `frozenTop` is a MEASURED number (the header's
 * rendered height), so a sub-pixel result, or a header that reflows between the measurement and
 * the paint, can leave a frozen row a pixel high. Keeping every body layer under both header
 * layers makes that a positioning wobble instead of a header disappearing behind a data row —
 * i.e. it demotes the arithmetic from a correctness problem to a cosmetic one.
 *
 * WHY `FROZEN` IS SPELT OUT AT ALL, given that `position: sticky` with `z-index: auto` already
 * paints above the static cells scrolling underneath it: because "already works" here rests on the
 * painting order of positioned versus non-positioned boxes, which is not a thing the next reader
 * of this file should have to reconstruct. `z-0` says the band sits above ordinary rows and below
 * the header's `z-10`, in the same vocabulary as every other layer here.
 */
const CELL_Z = { plain: "", frozen: "z-0", pinned: "z-20", corner: "z-[25]" } as const;
const cellZ = (pinned: boolean, frozen: boolean) =>
  pinned && frozen ? CELL_Z.corner : pinned ? CELL_Z.pinned : frozen ? CELL_Z.frozen : CELL_Z.plain;

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

  const cols = useMemo<ColumnDef<typeof TABLE_FEATURES, unknown[]>[]>(
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
            return renderTableCell(v, isNum, fmt, formatValue(v, c.type, withIdentifierDefault(c.name, c.type, fmt)), miniBarCaps[c.name] ?? 0);
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

  // `col_<i>` ids in the order they are DRAWN: the stored order first, then everything it does
  // not mention appended in result order. Keyed on the raw `c.name` and never on the header text
  // — `columnSettings[name].column_title` renames the header without renaming the column, so
  // resolving these through the title would lose every renamed column out of the order.
  const displayOrder = useMemo<string[]>(() => {
    const byName = new Map(columns.map((c, i) => [c.name, `col_${i}`]));
    const ordered = (tableState.columnOrder ?? [])
      .map((name) => byName.get(name))
      .filter((id): id is string => id != null);
    columns.forEach((_, i) => { if (!ordered.includes(`col_${i}`)) ordered.push(`col_${i}`); });
    return ordered;
  }, [columns, tableState.columnOrder]);

  /**
   * "Freeze first columns" counts down the list ABOVE — what is on screen, left to right.
   *
   * It used to be `col_0 … col_{n-1}`, i.e. the first N columns of the RESULT, which is an order
   * the user has no way to see once `columnOrder` has moved anything. The two then disagreed, and
   * since tanstack draws pinned columns first, turning the freeze on after a drag silently PUT THE
   * DRAG BACK: with `state` dragged to the front, freeze = 1 pinned `id` and `id` reappeared on the
   * left. Measured on the deployment before this changed.
   *
   * Hidden columns are skipped rather than counted. This half goes beyond the reordering bug: a
   * pinned id that `columnVisibility` has switched off is simply inert in tanstack — it is not in
   * `getVisibleLeafColumns` — so counting it meant "freeze the first 1 column" froze NOTHING
   * whenever the leading column was hidden. Both halves are the same rule: the count is over what
   * the user can see, because that is the only list the control's label can be describing.
   */
  const pinnedCount = Math.max(0, Math.min(tableState.pinnedLeft ?? 0, columns.length));
  const columnPinning = useMemo(
    () => ({
      start: displayOrder.filter((id) => columnVisibility[id] !== false).slice(0, pinnedCount),
      end: [],
    }),
    [displayOrder, columnVisibility, pinnedCount],
  );

  const showRowIndex = !!tableState.rowIndex;
  const paginationEnabled = !!tableState.pagination?.enabled;
  const pageSize = tableState.pagination?.pageSize ?? 25;
  const [pageIndex, setPageIndex] = useState(0);
  useEffect(() => { setPageIndex(0); }, [paginationEnabled, pageSize, rows]);

  const table = useTable({
    features: TABLE_FEATURES,
    data: rows,
    columns: cols,
    state: {
      sorting,
      columnSizing: localSizing,
      columnVisibility,
      columnPinning,
      columnOrder: displayOrder,
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
    // v8 registered the pagination row model ONLY when pagination was on, so
    // `getRowModel()` returned every row otherwise. v9's feature set is static, so the
    // same thing is said with `manualPagination`: it tells the table the rows arrive
    // already paged and to hand them back untouched. Without this, a card with
    // pagination off would silently render only the default page size.
    manualPagination: !paginationEnabled,
  });

  /**
   * FREEZE FIRST ROWS — the other axis of a freeze that `pinnedLeft` only ever had one of.
   *
   * WHAT METABASE FREEZES, which had to be read rather than guessed because the two obvious
   * readings agree on an unsorted single page and differ everywhere else. `table.freeze_rows` /
   * `table.freeze_rows_count` (Table.tsx) reach `TableInteractive.tsx` as `pinnedTopRowsCount`,
   * which `use-data-grid-instance.tsx` hands to `useRowPinningByCount` alongside `sortedRows` —
   * and `sortedRows` is filled from `table.getSortedRowModel().rows` under a comment that states
   * the intent outright: "For pinning row state we need to work with client-side sorted data".
   * That is the SORTED model, not the PAGINATED one. The resulting ids go into tanstack's
   * `rowPinning`, whose `_getPinnedRows` takes the `keepPinnedRows ?? true` branch — the branch
   * its own source comments as "get all rows that are pinned even if they would not be otherwise
   * visible … but not pagination or filtering".
   *
   * So: the first N rows AFTER SORTING and BEFORE PAGINATION, and those same N rows stay on screen
   * on every page. Not the first N rows of the result (they stop being at the top the moment
   * anything is sorted), and not the first N rows of the page you happen to be on (they would
   * change under the user as they paged, which is the opposite of what a frozen reference row is
   * for, and is not what freeze panes means in any spreadsheet either).
   *
   * IMPLEMENTED WITHOUT tanstack's `rowPinning` FEATURE, deliberately. Its state is a list of row
   * IDS, and the ids can only be known from the sorted model, which only exists once the table
   * instance does — Metabase resolves that circularity with a `useState` fed by a `useLayoutEffect`
   * (`setSortedRows(table.getSortedRowModel().rows)`), which costs a second render pass and a frame
   * where the freeze is a row behind the sort. Reading the sorted model here, AFTER `useReactTable`
   * has returned and during the same render, needs no feedback loop at all. What it reproduces is
   * exactly `getTopRows()` + `getCenterRows()` under `keepPinnedRows: true`: take the frozen rows
   * off the top of the sorted model, then draw the current page with those rows filtered out so
   * nothing appears twice.
   *
   * NOT PORTED: Metabase's cap that silently unpins rows once header + frozen rows would pass 80%
   * of the container's height (`useRowPinningByCount`). Two reasons. It renders a different number
   * of rows from the one the user typed with nothing on screen saying so — the failure mode this
   * programme keeps writing comments about. And it is measured off a container rect, so in any
   * layout-free environment `countWithinLimit([HEADER_HEIGHT, 0, 0], 0)` returns 0 and the whole
   * feature pins nothing, which would put the rule below out of reach of every test that is not a
   * browser. egret's scroll box is a fixed `max-h-[60vh]`; a user who freezes more rows than fit
   * gets a table that does not scroll, which is what a spreadsheet does with the same instruction.
   */
  /**
   * BOTH CLAMPS BOUND WORK, NOT OUTPUT, and that is a mutation result rather than a claim. Deleting
   * either of them leaves every assertion in the suite green, because the two things `frozenCount`
   * feeds already clamp themselves: `slice(0, n)` stops at the end of the array whatever `n` is,
   * and every `i < frozenCount` is read against rows that `slice` produced. A stored count of 500
   * on a 3-row result, or a negative one, renders exactly what a clamped count renders.
   *
   * They stay anyway, and the reason is the one loop that is NOT bounded by the data: the offset
   * measurement below iterates `frozenCount` times, so an unclamped 100000 out of a hand-written
   * `params.settings` — which the backend accepts, asserting only that the blob is a JSON object
   * (`chartparams.rs`), as `table-state.ts` records — would spin a hundred thousand times per
   * layout pass to push a hundred thousand identical numbers. Recorded here rather than left as a
   * pair of survivors, because "unkillable" and "unnecessary" are not the same finding and the next
   * person to run a mutant over this line deserves to know which one it is.
   */
  const frozenCount = Math.max(0, Math.min(tableState.pinnedTop ?? 0, rows.length));
  const sortedRows = table.getSortedRowModel().rows;
  const frozenRows = frozenCount > 0 ? sortedRows.slice(0, frozenCount) : [];

  /**
   * Sorted position by row id, which is what the "#" gutter now counts.
   *
   * It used to count `pageIndex * pageSize + i + 1` — the row's slot among the rows DRAWN. Frozen
   * rows break that arithmetic: on page 2 the two rows carried over from page 1 would be numbered
   * 26 and 27, and everything under them shifted by two. Position in the sorted result is the SAME
   * number wherever nothing is frozen (page p, slot i, is sorted position p*pageSize+i), so this
   * is a generalisation of the old rule rather than a change to it, and it is the only rule that
   * stays true once a row can appear on a page it does not belong to.
   *
   * Keyed on `r.id` and not on `r.index`: `index` is the row's position in the SOURCE data, which
   * is the number the old comment on the gutter already explains must not be shown.
   */
  const sortedPosition = useMemo(
    () => new Map(sortedRows.map((r, i) => [r.id, i])),
    [sortedRows],
  );

  // Not memoised, and that is the measurement rather than an omission: this is a slice and a filter
  // over ONE PAGE of rows plus a `Set` of the freeze count, where `sortedPosition` above is a `Map`
  // over the whole result and so earns its `useMemo`. A memo here could not hit anyway — `frozenRows`
  // is a fresh `slice` on every render, so every dependency list containing it is a cache miss.
  const pageRows = table.getRowModel().rows;
  const frozenIds = new Set(frozenRows.map((r) => r.id));
  const drawnRows = frozenCount === 0
    ? pageRows
    : [...frozenRows, ...pageRows.filter((r) => !frozenIds.has(r.id))];

  /**
   * A frozen row's `top` is the header's height plus the heights of the frozen rows above it, and
   * none of those are knowable ahead of layout: the header wraps at narrow widths, a row grows for
   * an image cell or a wrapped value, and a column resize changes both. So they are measured, and
   * kept measured by a `ResizeObserver` on the header and on each frozen row.
   *
   * CSS CANNOT DO THIS ON ITS OWN. `position: sticky` offsets do not accumulate down a set of
   * siblings — every frozen row given `top: 0` would stack in the same place and only the last one
   * drawn would be visible.
   *
   * STICKY IS WITHHELD UNTIL THE OFFSET EXISTS (`frozenTops` starts empty), so there is no first
   * paint in which the band is pinned to the wrong place. `useLayoutEffect` and not `useEffect`
   * because the measurement has to land before the browser paints, or that first frame is exactly
   * what the user sees.
   *
   * PER-CELL AND NOT PER-`<tr>`: sticky on a table ROW is unevenly supported, while sticky on a
   * cell is what the header and the pinned columns in this same table already rely on.
   */
  const theadRef = useRef<HTMLTableSectionElement>(null);
  const frozenRowRefs = useRef<(HTMLTableRowElement | null)[]>([]);
  const [frozenTops, setFrozenTops] = useState<number[]>([]);

  useLayoutEffect(() => {
    if (frozenCount === 0) {
      setFrozenTops((prev) => (prev.length === 0 ? prev : []));
      return;
    }
    const measure = () => {
      let acc = theadRef.current?.getBoundingClientRect().height ?? 0;
      const next: number[] = [];
      for (let j = 0; j < frozenCount; j++) {
        next.push(acc);
        acc += frozenRowRefs.current[j]?.getBoundingClientRect().height ?? 0;
      }
      // Identity-stable when nothing moved. Without it the observer's own callback would re-render
      // on every fire, and a ResizeObserver that re-renders on every fire is a loop.
      setFrozenTops((prev) =>
        prev.length === next.length && prev.every((v, j) => v === next[j]) ? prev : next);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    if (theadRef.current) ro.observe(theadRef.current);
    for (let j = 0; j < frozenCount; j++) {
      const el = frozenRowRefs.current[j];
      if (el) ro.observe(el);
    }
    return () => ro.disconnect();
  }, [frozenCount, rows, localSizing, showRowIndex, sorting, pageIndex]);

  /**
   * `col_<i>` sizing -> widths by column NAME, pruned to columns that still exist.
   *
   * ROUNDED, because `TableState.columnWidths` is documented "colName -> px" and the pivot's
   * writer (`ChartExplore`'s `onResizeColumn`) has always rounded — this was the one of the two
   * that did not. The fraction has exactly one source: the keyboard route starts from
   * `el.getBoundingClientRect().width` when a column has no stored width, which in Chrome is
   * something like `398.21875`. Tanstack's drag handler cannot produce one on its own (it adds an
   * integer `clientX` delta to `header.getSize()`), but it inherits one, because the float this
   * used to store became that column's `getSize()` from then on.
   *
   * HERE AND NOT IN `ResizeHandle`, which would look like the shared place to put it: the flat
   * table's DRAG does not go through the handle's `onResize` at all (it hands tanstack's
   * `getResizeHandler` to `dragHandlers`), so a round there would cover the keyboard and miss the
   * mouse. Rounding belongs to whatever WRITES the store, and this function is it.
   *
   * HERE AND NOT AT RENDER, which would have had the creep the round-trip test rules out: every
   * later gesture starts from the committed number, so rounding once at the commit takes the error
   * against the DOM a single time and bounds it at half a pixel forever. Rounding only on the way
   * out, with the float still seeding the next gesture, would re-introduce it on every commit.
   */
  const commitWidths = (sizing: ColumnSizingState) => {
    const widths: Record<string, number> = {};
    Object.entries(sizing).forEach(([id, px]) => {
      const name = columns[Number(id.slice(4))]?.name;  // "col_3" -> 3
      if (name) widths[name] = Math.round(px as number);
    });
    onTableStateChange?.({ ...tableState, columnWidths: widths });
  };

  // persist widths only when a resize gesture ends
  // v9 split resizing out of the sizing slice: `columnSizingInfo` is now `columnResizing`.
  const isResizing = table.state.columnResizing.isResizingColumn;
  const wasResizing = useRef<string | false>(false);
  useEffect(() => {
    if (wasResizing.current && !isResizing) commitWidths(localSizing);
    wasResizing.current = isResizing;
  }, [isResizing]);

  /**
   * An arrow-key resize, which is a gesture with no drag around it and therefore no END for the
   * effect above to fire on: tanstack's `columnSizingInfo.isResizingColumn` only ever moves for a
   * pointer gesture. Writing the new width into `localSizing` alone would have shown the column
   * change and then lost it on the next render from the parent, which is the "green over a dead
   * path" shape this programme keeps hitting — so the keyboard commits for itself, immediately.
   * There is no spam risk in doing so: one keypress is one step, where a drag is a hundred moves.
   */
  // NO SECOND `Math.max(MIN_COL_WIDTH, …)` HERE, and that is a finding rather than an omission: the
  // first draft clamped in both places, and the mutation that deleted the clamp inside `ResizeHandle`
  // SURVIVED because this one silently covered for it. One clamp, in the shared control, where the
  // pivot's call site depends on it too (`onResizeColumn` there clamps nothing of its own).
  const resizeFromKeyboard = (id: string, width: number) => {
    const next: ColumnSizingState = { ...localSizing, [id]: width };
    setLocalSizing(next);
    commitWidths(next);
  };

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    // Without this, reordering a column is reachable only by dragging with a mouse. Its activator
    // only listens on keydown (Space/Enter start a drag, arrows move it, Space/Enter again drop it).
    //
    // WHAT THAT COST, WHICH THIS NOTE USED TO GET WRONG. It said sorting "is wired to onClick
    // alone, on a wholly separate event, so the two never fire off the same keypress" — true, and
    // the wrong thing to be reassured by. The audit measured what a keyboard user actually got: the
    // `<th>` is tabbable (dnd-kit's `tabIndex=0`) and announces itself `sortable`, Enter set
    // `opacity: 0.6` — a lift — and the text never changed. Sorting had NO keyboard route, because
    // "a wholly separate event" meant a mouse-only one. The two not colliding was the whole problem.
    //
    // Resolved by giving sorting its own control rather than by taking a key off the drag: a
    // `SortButton` inside the `<th>` swallows Enter/Space before this activator sees them (see
    // `TableHeaderControls.tsx`), so one keypress still does exactly one thing — and which thing now
    // depends on which control has focus, which is a distinction a keyboard user can perceive.
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  function handleDragEnd(e: DragEndEvent) {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const ids = table.state.columnOrder.length ? table.state.columnOrder : columns.map((_, i) => `col_${i}`);
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
          <thead ref={theadRef} className="bg-accent">
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
                  const meta = h.column.columnDef.meta as { isNum?: boolean; name?: string } | undefined;
                  const isNum = meta?.isNum ?? false;
                  const sized = localSizing[h.column.id] != null;
                  const pinned = h.column.getIsPinned() === "start";
                  const sorted = h.column.getIsSorted() as "asc" | "desc" | false;
                  const title = String(h.column.columnDef.header ?? meta?.name ?? h.column.id);
                  return (
                    <SortableHeader
                      key={h.id}
                      id={h.column.id}
                      disabled={pinned}
                      label={title}
                      sort={sorted === false ? undefined : sorted}
                      onClick={h.column.getToggleSortingHandler()}
                      style={{
                        ...(sized ? { width: h.getSize() } : {}),
                        ...(pinned ? { left: h.column.getStart("start") + (showRowIndex ? ROW_INDEX_WIDTH : 0) } : {}),
                      }}
                      className={`relative sticky top-0 cursor-pointer select-none border-b border-border/50 bg-accent px-3 py-2 font-medium whitespace-nowrap ${
                        pinned ? "z-30 border-r border-border/50" : "z-10"
                      } ${isNum ? "text-right" : "text-left"}`}
                    >
                      {/* SORTING'S ONLY ROUTE USED TO BE THE `<th>`'s `onClick`. The cell is
                          tabbable — dnd-kit puts `tabIndex=0` on it — so a keyboard user reached it
                          and Enter lifted it into a drag instead, leaving sorting with no keyboard
                          route at all (`SortRows.tsx` edits the query's `orderby` and lives only in
                          the builder). The `<th>` keeps its click, so a mouse still sorts by hitting
                          anywhere in the cell; the button adds the route that was missing. */}
                      <SortButton
                        label={title}
                        direction={sorted === false ? undefined : sorted}
                        onClick={() => h.column.toggleSorting()}
                      >
                        {flexRender(h.column.columnDef.header, h.getContext())}
                      </SortButton>
                      {h.column.getCanResize() && (
                        <ResizeHandle
                          label={`Resize ${title}`}
                          getWidth={(el) => localSizing[h.column.id] ?? el.getBoundingClientRect().width}
                          onResize={(w) => resizeFromKeyboard(h.column.id, w)}
                          dragHandlers={{ onMouseDown: h.getResizeHandler(), onTouchStart: h.getResizeHandler() }}
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
            {drawnRows.map((r, i) => {
              const rowColor = cond.rowBg(r.index);
              // The frozen rows are drawn first and in order, so a row's slot IS its place in the
              // band. `frozenTops[j]` is `undefined` until the first measurement lands, and that
              // absence is what withholds sticky for the frame before it.
              const frozenTop = i < frozenCount ? frozenTops[i] : undefined;
              const isFrozen = frozenTop != null;
              /**
               * The edge of the band, which without a line of its own is invisible whenever the row
               * index is off: a frozen row is drawn exactly like the rows scrolling past it, so
               * "1, 2, 9, 10" reads as a table that has lost some rows rather than as two rows held
               * in place. Metabase draws the same edge — `.pinnedRowsSection` carries
               * `border-bottom: 2px solid var(--mb-color-border-subtle)` in DataGrid.module.css,
               * with a comment noting it is matched against the pinned COLUMNS' 3px `border-right`
               * so the two axes read as one pair.
               *
               * WEIGHT CHOSEN AGAINST EGRET'S OWN COLUMN EDGE, not against Metabase's number. The
               * pinned column here is separated by `border-r border-border/50` — 1px at half
               * opacity — so importing a 3px rule onto the row axis alone would make the two halves
               * of one feature look unrelated. 2px at full opacity is the smallest step that reads
               * as deliberate beside it. Raising the column's edge to match is a change to shipped
               * behaviour and belongs to whoever revisits `pinnedLeft`, not here.
               *
               * INLINE, NOT A UTILITY CLASS. `border-b border-border/50` on these cells is a
               * `border-color` SHORTHAND; overriding just the bottom edge with a `border-b-*`
               * utility puts a longhand and a shorthand of equal specificity in the cascade, where
               * the winner is decided by the order Tailwind happens to emit them and not by the
               * order they appear in the class attribute. An inline style has no such argument.
               */
              const bandEdge = isFrozen && i === frozenCount - 1
                ? { borderBottom: "2px solid var(--border)" }
                : undefined;
              return (
                <tr
                  key={r.id}
                  ref={(el) => { if (i < frozenCount) frozenRowRefs.current[i] = el; }}
                  className="hover:bg-accent/40"
                  style={rowColor ? { backgroundColor: rowColor } : undefined}
                >
                  {showRowIndex && (
                    // Position in the SORTED result, not `r.index` and no longer the row's slot on
                    // the page: after a sort the underlying row order is meaningless, and a frozen
                    // row appears on pages it does not belong to. See `sortedPosition`.
                    <td
                      className={`sticky left-0 border-b border-r border-border/50 px-3 py-1.5 text-right text-muted-foreground tabular-nums ${
                        isFrozen ? CELL_Z.corner : CELL_Z.pinned
                      }`}
                      style={{
                        backgroundColor: rowColor ?? "var(--background)",
                        ...(isFrozen ? { top: frozenTop } : {}),
                        ...bandEdge,
                      }}
                    >
                      {(sortedPosition.get(r.id) ?? i) + 1}
                    </td>
                  )}
                  {r.getVisibleCells().map((cell) => {
                    const meta = cell.column.columnDef.meta as { isNum?: boolean; name?: string } | undefined;
                    const isNum = meta?.isNum ?? false;
                    const bg = cond.bg(cell.getValue(), meta?.name ?? cell.column.id);
                    const sized = localSizing[cell.column.id] != null;
                    const pinned = cell.column.getIsPinned() === "start";
                    // A cell that does not move with what is behind it needs an opaque base, or the
                    // rows sliding underneath it are legible straight through — an ordinary cell
                    // carries no background of its own, the row's colour being on the `<tr>`. True
                    // on BOTH axes, which is the whole of what the row freeze added here: composite
                    // any (translucent) conditional/row colour over the page bg.
                    const stuck = pinned || isFrozen;
                    const tint = stuck ? (bg ?? rowColor) : null;
                    return (
                      <td
                        key={cell.id}
                        style={{
                          ...(sized ? { width: cell.column.getSize() } : {}),
                          ...(pinned ? { left: cell.column.getStart("start") + (showRowIndex ? ROW_INDEX_WIDTH : 0) } : {}),
                          ...(isFrozen ? { top: frozenTop } : {}),
                          ...(stuck
                            ? (tint ? { background: `linear-gradient(${tint}, ${tint}), var(--background)` } : {})
                            : (bg ? { backgroundColor: bg } : {})),
                          ...bandEdge,
                        }}
                        className={`border-b border-border/50 px-3 py-2 ${isNum ? "text-right tabular-nums" : ""} ${
                          stuck
                            ? `sticky ${cellZ(pinned, isFrozen)}${pinned ? " border-r border-border/50" : ""}${tint ? "" : " bg-background"}`
                            : ""
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
      {/*
        THE ROW COUNT IS NOT A PAGINATION DETAIL, and it used to be nested inside one — the whole
        footer hung off `paginationEnabled`, which is off unless a card turns it on, so egret's
        ordinary 60-row table said nothing at all about how many rows it was showing.

        Metabase prints a count under every table and its wording is `Showing ${n} row(s)` —
        `getRowCountMessage` (common/utils/get-row-count-message.ts) and `QuestionRowCount` both
        build it from `formatRowCount`, which is an `ngettext` over "row"/"rows", so one row reads
        "Showing 1 row". The page RANGE is a separate control there (`PaginationControls`, which
        renders `1 - 10 of 100`) and appears only when there is a page to move between — which is
        why the two are separate spans here rather than one sentence that changes shape.

        NOT PORTED: Metabase's "Showing first N rows", which it says when the result was truncated
        at the hard row limit. egret's table is handed the rows it has and is told nothing about
        whether the engine cut them short, so claiming "first" would be a guess. Row limits live on
        the query side (`shared.rowLimit`); wiring the truncation flag through to here is its own
        change.
      */}
      <div className="flex items-center justify-between border-t border-border/50 px-3 py-2 text-xs text-muted-foreground">
        <span>Showing {totalRows} {totalRows === 1 ? "row" : "rows"}</span>
        {paginationEnabled && (
          <div className="flex items-center gap-2">
            <span>{start + 1}–{Math.min(start + pageSize, totalRows)} of {totalRows}</span>
            <Button size="sm" variant="outline" className="h-7" disabled={!table.getCanPreviousPage()} onClick={() => table.previousPage()}>Prev</Button>
            <span>Page {pageIndex + 1} / {Math.max(1, table.getPageCount())}</span>
            <Button size="sm" variant="outline" className="h-7" disabled={!table.getCanNextPage()} onClick={() => table.nextPage()}>Next</Button>
          </div>
        )}
      </div>
    </div>
    </DndContext>
  );
}
