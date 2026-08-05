import { buildPivot, pivotProblem, type PivotInput } from "./engine";
import { formatValue } from "../format";
import { buildConditional } from "../conditional";
import type { ColumnFormat, Rule } from "../format-types";
import { pivotCellDrill, type DrillContext } from "../drill";
// `ResizeHandle` used to live here. It moved out when `ChartTable` needed the same control — see
// `TableHeaderControls.tsx` for why it is shared rather than copied.
import { ResizeHandle, SortButton, ariaSort } from "../TableHeaderControls";

export interface PivotColumnWidths {
  /** by row-dimension position */
  left?: Record<number, number>;
  /** by column-leaf index */
  value?: Record<number, number>;
}

export function PivotTable({
  input, columnSettings = {}, conditionalFormatting = [], metricTypes = {},
  onToggleCollapse, onCycleSort, onCellDrill, columnWidths, onResizeColumn, temporalUnits = {},
  columnTypes = {},
}: {
  input: PivotInput;
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
  metricTypes?: Record<string, string>;
  onToggleCollapse?: (serializedPath: string) => void;
  onCycleSort?: (dimName: string) => void;
  onCellDrill?: (ctx: DrillContext, x: number, y: number) => void;
  columnWidths?: PivotColumnWidths;
  /** Omitted in read-only surfaces (a dashboard card), which disables the handles. */
  onResizeColumn?: (kind: "left" | "value", index: number, width: number) => void;
  /** dimension -> temporal unit, so a drill on a bucketed column filters the whole bucket rather
   * than the single instant its label happens to show. */
  temporalUnits?: Record<string, string>;
  /** column name -> SQL type, for the DIMENSIONS as well as the measures. Without it a date
   * dimension's labels render as the raw `2026-01-01` even when the user has set a date format. */
  columnTypes?: Record<string, string>;
}) {
  // Checked BEFORE building, the way `sankeyProblem` is — a result the engine cannot place must
  // not reach `buildPivot`, which would silently assemble a one-cell table out of it.
  //
  // Rendered HERE rather than at the two call sites, which is where the sankey message lives. That
  // check sits outside `SankeyChart` because it decides whether an error boundary renders at all
  // and hooks cannot follow an early return in `ChartRenderer`; neither applies to this component,
  // which uses no hooks and has no boundary of its own. What does apply is that `PivotTable` is
  // rendered from both `ChartRenderer` (dashboard cards) and `PreviewPane` (the editor) — putting
  // the branch at the call sites would duplicate it, and a pivot that fails on a dashboard but not
  // in the editor is exactly the asymmetry this guard exists to expose.
  const problem = pivotProblem(input);
  if (problem) {
    return (
      <div className="flex h-full min-h-[8rem] items-center justify-center p-6 text-center text-sm text-muted-foreground">
        {problem}
      </div>
    );
  }

  // The engine needs to know whether any metric was renamed, so it can give the sub-header a row
  // to show the name in — see `renamedMetric` there.
  const metricTitles: Record<string, string> = {};
  for (const m of input.metrics) {
    const t = columnSettings[m]?.column_title;
    if (t) metricTitles[m] = t;
  }
  const p = buildPivot({ ...input, metricTitles });
  const condRows = p.bodyRows.filter((br) => br.kind === "leaf").map((br) => br.values);
  const condColIndex: Record<string, number> = {};
  p.colLeaves.forEach((cl, i) => { if (!(cl.metric in condColIndex)) condColIndex[cl.metric] = i; });
  const cond = buildConditional(condRows, condColIndex, conditionalFormatting, { isPivot: true });
  const rowDimCount = p.rowDimNames.length;
  const levels = p.topHeader.length;
  const sortDirs = input.sortDims ?? {};

  // Hairline grid mirroring Metabase: horizontal (table-border ~8%) slightly stronger than the
  // vertical (border-subtle ~5%). Opaque bg-accent total band; opaque row-headers so sticky panes
  // never bleed. border-separate keeps cell z-index working.
  const GRID = "border-b border-border/50 border-r border-border/30";

  /** A dimension label, formatted the way its column says to. A bucketed date arrives as a raw
   * `2026-01-01`; with a date format set it should read like a date. `raw == null` means the cell
   * carries prose (a subtotal caption), which is left alone. */
  const dimLabel = (dim: string | undefined, raw: string | null | undefined, fallback: string) => {
    if (dim == null || raw == null) return fallback;
    const fmt = columnSettings[dim];
    if (!fmt) return fallback;
    const out = formatValue(raw, columnTypes[dim] ?? "varchar", fmt);
    return out === "" ? fallback : out;
  };

  return (
    <div className="max-h-[60vh] overflow-auto rounded-md border">
      <table className="w-full border-separate border-spacing-0 text-xs text-foreground/90">
        {/* Widths live on <colgroup> so a body cell never has to know its own width — otherwise
            every one of them would need the setting threaded down and re-rendered on a drag. */}
        <colgroup>
          {p.rowDimNames.map((_, i) => (
            <col key={`l-${i}`} style={columnWidths?.left?.[i] ? { width: columnWidths.left[i] } : undefined} />
          ))}
          {p.colLeaves.map((_, i) => (
            <col key={`v-${i}`} style={columnWidths?.value?.[i] ? { width: columnWidths.value[i] } : undefined} />
          ))}
        </colgroup>
        <thead className="text-foreground">
          {p.topHeader.map((level, li) => (
            <tr key={li}>
              {/* empty corner above the row-dim name headers (only when >1 header level) */}
              {li === 0 && levels > 1 && (
                <th className={`w-px sticky left-0 top-0 z-30 ${GRID} bg-accent`} colSpan={rowDimCount} rowSpan={levels - 1} />
              )}
              {/* row-dim names as their own column headers on the last header row (clickable to sort) */}
              {li === levels - 1 &&
                p.rowDimNames.map((name, di) => {
                  const dir = sortDirs[name];
                  const title = columnSettings[name]?.column_title || name;
                  return (
                    <th
                      key={`rd-${di}`}
                      onClick={onCycleSort ? () => onCycleSort(name) : undefined}
                      // The plain title. Without it Chrome leaves this columnheader UNNAMED once
                      // its text is inside a labelled button — measured, see `SortButton` — and a
                      // columnheader's name is announced for every cell under it.
                      aria-label={title}
                      // `aria-sort` only where there IS a sort to report: a dashboard card renders
                      // this read-only, and announcing "not sorted" on a column that can never be
                      // sorted is the same kind of lie the glyph told by staying silent.
                      aria-sort={onCycleSort ? ariaSort(dir) : undefined}
                      className={`relative w-px sticky top-0 z-30 ${GRID} bg-accent px-4 py-1.5 text-left font-semibold whitespace-nowrap ${di === 0 ? "left-0" : ""} ${onCycleSort ? "cursor-pointer select-none" : ""}`}
                    >
                      {/* THIS HEADER USED TO BE A `<th onClick>` AND NOTHING ELSE — no `tabIndex`,
                          no `onKeyDown`. Measured during the audit: `th.focus()` did not move
                          `activeElement` and Enter fired the handler zero times, so the pivot's only
                          sort control was unreachable without a mouse. */}
                      {onCycleSort
                        ? <SortButton label={title} direction={dir} onClick={() => onCycleSort(name)}>{title}</SortButton>
                        : <>{title}{dir === "asc" ? " ▲" : dir === "desc" ? " ▼" : ""}</>}
                      {onResizeColumn && (
                        <ResizeHandle
                          label={`Resize ${name}`}
                          getWidth={(el) => columnWidths?.left?.[di] ?? el.getBoundingClientRect().width}
                          onResize={(w) => onResizeColumn("left", di, w)}
                        />
                      )}
                    </th>
                  );
                })}
              {(() => {
                // A header cell only lines up with a single value column when it spans one. That
                // is true on the last level — either the metric sub-header (one cell per leaf) or,
                // with a single metric, the innermost column-dimension level. Higher levels group
                // several leaves, so they get no handle rather than a wrong one.
                let offset = 0;
                return level.map((c, ci) => {
                  const leafIndex = offset;
                  offset += c.colSpan;
                  const resizable = onResizeColumn && c.colSpan === 1 && leafIndex < p.colLeaves.length;
                  return (
                    <th
                      key={ci}
                      className={`relative sticky top-0 z-10 ${GRID} bg-accent px-4 py-1.5 text-right font-semibold whitespace-nowrap last:border-r-0`}
                      colSpan={c.colSpan}
                      rowSpan={c.rowSpan ?? 1}
                    >
                      {c.metric
                        ? (columnSettings[c.metric]?.column_title || c.label)
                        : dimLabel(c.dim, c.raw, c.label)}
                      {resizable && (
                        <ResizeHandle
                          label={`Resize ${c.label}`}
                          getWidth={(el) => columnWidths?.value?.[leafIndex] ?? el.getBoundingClientRect().width}
                          onResize={(w) => onResizeColumn("value", leafIndex, w)}
                        />
                      )}
                    </th>
                  );
                });
              })()}
            </tr>
          ))}
        </thead>
        <tbody>
          {p.bodyRows.map((br, ri) => {
            const isTotal = br.kind === "subtotal" || br.kind === "grand";
            return (
              <tr key={ri} className={isTotal ? "font-semibold" : "hover:bg-accent/40"}>
                {br.headerCells.map((hc, hi) => (
                  <th
                    key={hi}
                    scope="row"
                    className={`w-px ${GRID} ${isTotal ? "bg-accent" : "bg-background"} px-4 py-1.5 text-left align-top font-normal whitespace-nowrap ${hc.depth === 0 ? "sticky left-0 z-20" : ""}`}
                    rowSpan={hc.rowSpan}
                    colSpan={hc.colSpan}
                  >
                    {hc.collapsible && hc.path && onToggleCollapse && (
                      <button
                        type="button"
                        onClick={() => onToggleCollapse(JSON.stringify(hc.path))}
                        aria-label={hc.collapsed ? "Expand group" : "Collapse group"}
                        className="mr-1.5 inline-flex h-4 w-4 items-center justify-center rounded border bg-background text-xs leading-none text-muted-foreground hover:bg-accent hover:text-foreground"
                      >
                        {hc.collapsed ? "+" : "–"}
                      </button>
                    )}
                    {dimLabel(p.rowDimNames[hc.depth], hc.raw, hc.label)}
                  </th>
                ))}
                {br.values.map((v, vi) => {
                  const cl = p.colLeaves[vi];
                  const text = formatValue(v, metricTypes[cl.metric] ?? "double", columnSettings[cl.metric]);
                  const bg = cond.bg(v, cl.metric);
                  return (
                    <td
                      key={vi}
                      className={`${GRID} px-4 py-1.5 text-right tabular-nums last:border-r-0 ${
                        isTotal || cl.isTotal ? "font-semibold" : ""
                      } ${isTotal && !bg ? "bg-accent" : ""} ${onCellDrill && br.kind === "leaf" ? "cursor-pointer" : ""}`}
                      style={bg ? { backgroundColor: bg } : undefined}
                      onClick={onCellDrill && br.kind === "leaf"
                        ? (e) => onCellDrill(
                            pivotCellDrill(input.rowDims, br.rowPath, input.colDims, cl.colPath, temporalUnits),
                            e.clientX, e.clientY,
                          )
                        : undefined}
                    >
                      {text}
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
