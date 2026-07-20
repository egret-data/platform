/** Renders a notebook DataFrame output (application/vnd.egret.table+json) as a
 *  clean egret-styled table — bordered, sticky dtype header, zebra rows, numeric
 *  right-alignment, scrollable. Matches the app's ChartTable look without its BI
 *  machinery (sorting/formatting/drill). */
export interface DataFrameTableData {
  columns: { name: string; type: string }[];
  rows: unknown[][];
  truncated?: boolean;
}

const NUMERIC = /^(u?int|float|decimal|double|number)/i;
const isNumeric = (t: string) => NUMERIC.test(t.trim());

function cell(v: unknown): string {
  if (v === null || v === undefined) return "";
  return typeof v === "string" ? v : String(v);
}

export default function DataFrameTable({ data }: { data: DataFrameTableData }) {
  const { columns, rows } = data;
  const numericCol = columns.map((c) => isNumeric(c.type));

  return (
    <div className="max-w-full rounded-md border">
      <div className="max-h-96 overflow-auto">
        <table className="w-full border-separate border-spacing-0 text-xs text-foreground/90">
          <thead>
            <tr>
              {columns.map((c, i) => (
                <th
                  key={i}
                  className={`sticky top-0 z-10 border-b border-border/50 bg-accent px-3 py-2 font-medium whitespace-nowrap ${
                    numericCol[i] ? "text-right" : "text-left"
                  }`}
                >
                  <div>{c.name}</div>
                  {c.type ? (
                    <div className="font-normal text-[10px] uppercase tracking-wide text-muted-foreground">
                      {c.type}
                    </div>
                  ) : null}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((r, ri) => (
              <tr key={ri} className="hover:bg-accent/40">
                {columns.map((_, ci) => (
                  <td
                    key={ci}
                    className={`border-b border-border/50 px-3 py-1.5 whitespace-nowrap ${
                      numericCol[ci] ? "text-right tabular-nums" : ""
                    }`}
                  >
                    {cell(r[ci])}
                  </td>
                ))}
              </tr>
            ))}
            {rows.length === 0 ? (
              <tr>
                <td
                  colSpan={Math.max(1, columns.length)}
                  className="px-3 py-2 text-center text-muted-foreground"
                >
                  No rows
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
      <div className="border-t border-border/50 px-3 py-1.5 text-[11px] text-muted-foreground">
        {rows.length} {rows.length === 1 ? "row" : "rows"}
        {data.truncated ? " (showing the first rows)" : ""} · {columns.length}{" "}
        {columns.length === 1 ? "column" : "columns"}
      </div>
    </div>
  );
}
