import { buildPivot, type PivotInput } from "./engine";

export function PivotTable({ input }: { input: PivotInput }) {
  const p = buildPivot(input);
  const rowDimCount = p.rowDimNames.length;

  return (
    <div className="overflow-auto">
      <table className="border-collapse text-sm">
        <thead>
          {p.topHeader.map((level, li) => (
            <tr key={li} className="border-b text-muted-foreground">
              {li === 0 && (
                <th
                  className="px-3 py-1 text-left"
                  colSpan={rowDimCount}
                  rowSpan={p.topHeader.length}
                >
                  {p.rowDimNames.join(" / ")}
                </th>
              )}
              {level.map((c, ci) => (
                <th
                  key={ci}
                  className="border-l px-3 py-1 text-center"
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
              <tr key={ri} className={`border-t ${total ? "bg-muted/50 font-medium" : ""}`}>
                {br.headerCells.map((hc, hi) => (
                  <th
                    key={hi}
                    scope="row"
                    className="px-3 py-1 text-left align-top font-normal"
                    rowSpan={hc.rowSpan}
                    colSpan={hc.colSpan}
                  >
                    {hc.label}
                  </th>
                ))}
                {br.values.map((v, vi) => (
                  <td
                    key={vi}
                    className={`border-l px-3 py-1 text-right tabular-nums ${
                      p.colLeaves[vi]?.isTotal ? "font-medium" : ""
                    }`}
                  >
                    {v}
                  </td>
                ))}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
