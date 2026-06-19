import { buildPivot, type PivotInput } from "./engine";

export function PivotTable({ input }: { input: PivotInput }) {
  const p = buildPivot(input);
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
            const total = br.kind === "subtotal" || br.kind === "grand";
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
                  const isTotalCol = p.colLeaves[vi]?.isTotal;
                  return (
                    <td
                      key={vi}
                      className={`border-r px-4 py-1.5 text-right tabular-nums last:border-r-0 ${
                        total || isTotalCol ? "font-semibold" : ""
                      }`}
                    >
                      {v}
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
