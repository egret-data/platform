import { buildPivot, type PivotInput } from "./engine";
import { formatValue } from "../format";
import { buildConditional } from "../conditional";
import type { ColumnFormat, Rule } from "../format-types";

export function PivotTable({
  input, columnSettings = {}, conditionalFormatting = [], metricTypes = {},
}: {
  input: PivotInput;
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
  metricTypes?: Record<string, string>;
}) {
  const p = buildPivot(input);
  const condRows = p.bodyRows.filter((br) => br.kind === "leaf").map((br) => br.values);
  const condColIndex: Record<string, number> = {};
  p.colLeaves.forEach((cl, i) => { if (!(cl.metric in condColIndex)) condColIndex[cl.metric] = i; });
  const cond = buildConditional(condRows, condColIndex, conditionalFormatting, { isPivot: true });
  const rowDimCount = p.rowDimNames.length;
  const levels = p.topHeader.length;

  return (
    <div className="overflow-auto rounded-md border">
      <table className="border-collapse text-sm">
        <thead className="bg-muted/40 text-foreground">
          {p.topHeader.map((level, li) => (
            <tr key={li} className="border-b">
              {/* empty corner above the row-dim name headers (only when >1 header level) */}
              {li === 0 && levels > 1 && (
                <th className="border-r" colSpan={rowDimCount} rowSpan={levels - 1} />
              )}
              {/* row-dim names as their own column headers on the last header row */}
              {li === levels - 1 &&
                p.rowDimNames.map((name, di) => (
                  <th key={`rd-${di}`} className="border-r px-4 py-2 text-left font-semibold whitespace-nowrap">
                    {name}
                  </th>
                ))}
              {level.map((c, ci) => (
                <th
                  key={ci}
                  className="border-r px-4 py-2 text-right font-semibold whitespace-nowrap last:border-r-0"
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
          {p.bodyRows.map((br, ri) => {
            return (
              <tr
                key={ri}
                className={
                  br.kind === "grand"
                    ? "border-t-2 font-semibold"
                    : br.kind === "subtotal"
                      ? "border-t bg-muted/20 font-semibold"
                      : "border-t hover:bg-muted/20"
                }
              >
                {br.headerCells.map((hc, hi) => (
                  <th
                    key={hi}
                    scope="row"
                    className="border-r px-4 py-1.5 text-left align-top font-normal whitespace-nowrap"
                    rowSpan={hc.rowSpan}
                    colSpan={hc.colSpan}
                  >
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
                      className={`border-l px-4 py-1.5 text-right tabular-nums last:border-r-0 ${
                        br.kind === "subtotal" || br.kind === "grand" || cl.isTotal ? "font-semibold" : ""
                      }`}
                      style={bg ? { backgroundColor: bg } : undefined}
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
