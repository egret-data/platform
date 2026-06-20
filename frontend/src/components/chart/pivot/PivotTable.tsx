import { buildPivot, type PivotInput } from "./engine";
import { formatValue } from "../format";
import { buildConditional } from "../conditional";
import type { ColumnFormat, Rule } from "../format-types";
import { pivotCellDrill, type DrillContext } from "../drill";

export function PivotTable({
  input, columnSettings = {}, conditionalFormatting = [], metricTypes = {},
  onToggleCollapse, onCycleSort, onCellDrill,
}: {
  input: PivotInput;
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
  metricTypes?: Record<string, string>;
  onToggleCollapse?: (serializedPath: string) => void;
  onCycleSort?: (dimName: string) => void;
  onCellDrill?: (ctx: DrillContext, x: number, y: number) => void;
}) {
  const p = buildPivot(input);
  const condRows = p.bodyRows.filter((br) => br.kind === "leaf").map((br) => br.values);
  const condColIndex: Record<string, number> = {};
  p.colLeaves.forEach((cl, i) => { if (!(cl.metric in condColIndex)) condColIndex[cl.metric] = i; });
  const cond = buildConditional(condRows, condColIndex, conditionalFormatting, { isPivot: true });
  const rowDimCount = p.rowDimNames.length;
  const levels = p.topHeader.length;
  const sortDirs = input.sortDims ?? {};

  return (
    <div className="max-h-[60vh] overflow-auto rounded-md border">
      <table className="w-full border-collapse text-sm">
        <thead className="bg-muted/40 text-foreground">
          {p.topHeader.map((level, li) => (
            <tr key={li} className="border-b border-border/60">
              {/* empty corner above the row-dim name headers (only when >1 header level) */}
              {li === 0 && levels > 1 && (
                <th className="w-px sticky left-0 top-0 z-30 border-r border-border/60 bg-muted/40" colSpan={rowDimCount} rowSpan={levels - 1} />
              )}
              {/* row-dim names as their own column headers on the last header row (clickable to sort) */}
              {li === levels - 1 &&
                p.rowDimNames.map((name, di) => {
                  const dir = sortDirs[name];
                  return (
                    <th
                      key={`rd-${di}`}
                      onClick={onCycleSort ? () => onCycleSort(name) : undefined}
                      className={`w-px sticky top-0 z-30 border-r border-border/60 bg-muted/40 px-4 py-2 text-left font-semibold whitespace-nowrap ${di === 0 ? "left-0" : ""} ${onCycleSort ? "cursor-pointer select-none" : ""}`}
                    >
                      {name}{dir === "asc" ? " ▲" : dir === "desc" ? " ▼" : ""}
                    </th>
                  );
                })}
              {level.map((c, ci) => (
                <th
                  key={ci}
                  className="sticky top-0 z-10 border-r border-border/60 bg-muted/40 px-4 py-2 text-right font-semibold whitespace-nowrap last:border-r-0"
                  colSpan={c.colSpan}
                  rowSpan={c.rowSpan ?? 1}
                >
                  {c.label}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody>
          {p.bodyRows.map((br, ri) => (
            <tr
              key={ri}
              className={
                br.kind === "grand"
                  ? "border-t-2 border-border font-semibold"
                  : br.kind === "subtotal"
                    ? "border-t border-border/60 bg-muted/20 font-semibold"
                    : "border-t border-border/60 hover:bg-muted/20"
              }
            >
              {br.headerCells.map((hc, hi) => (
                <th
                  key={hi}
                  scope="row"
                  className={`w-px border-r border-border/60 bg-background px-4 py-2 text-left align-top font-normal whitespace-nowrap ${hc.depth === 0 ? "sticky left-0 z-20" : ""}`}
                  rowSpan={hc.rowSpan}
                  colSpan={hc.colSpan}
                >
                  {hc.collapsible && hc.path && onToggleCollapse && (
                    <button
                      type="button"
                      onClick={() => onToggleCollapse(JSON.stringify(hc.path))}
                      aria-label={hc.collapsed ? "Expand group" : "Collapse group"}
                      className="mr-1 inline-flex h-4 w-4 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
                    >
                      {hc.collapsed ? "▸" : "▾"}
                    </button>
                  )}
                  {hc.label}
                </th>
              ))}
              {br.values.map((v, vi) => {
                const cl = p.colLeaves[vi];
                const text = formatValue(v, metricTypes[cl.metric] ?? "double", columnSettings[cl.metric]);
                const bg = cond.bg(v, cl.metric);
                return (
                  <td
                    key={vi}
                    className={`border-l border-border/60 px-4 py-2 text-right tabular-nums last:border-r-0 ${
                      br.kind === "subtotal" || br.kind === "grand" || cl.isTotal ? "font-semibold" : ""
                    } ${onCellDrill && br.kind === "leaf" ? "cursor-pointer" : ""}`}
                    style={bg ? { backgroundColor: bg } : undefined}
                    onClick={onCellDrill && br.kind === "leaf"
                      ? (e) => onCellDrill(
                          pivotCellDrill(input.rowDims, br.rowPath, input.colDims, cl.colPath),
                          e.clientX, e.clientY,
                        )
                      : undefined}
                  >
                    {text}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
