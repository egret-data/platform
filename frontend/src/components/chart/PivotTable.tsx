export type FlatRow = unknown[];

export interface PivotRow {
  key: string[];           // row-dimension values (strings)
  cells: Record<string, number | string | null>; // colKey.join("|") → metric value
  isSubtotal: boolean;
}
export interface Pivot {
  colKeys: string[][];     // distinct column-dimension tuples (data only)
  rows: PivotRow[];        // leaf + subtotal rows, in source order
  grandTotal: Record<string, number | string | null> | null;
}

const key = (parts: (string | null)[]) => parts.map((p) => (p == null ? "∅" : String(p))).join("|");

/** Cross-tab flat ROLLUP rows into a pivot grid.
 *
 * Assumes the result came from `GROUP BY ROLLUP(rowDim, colDim)` with AT MOST ONE row
 * dimension and AT MOST ONE column dimension — that ROLLUP yields exactly {leaf cells, one
 * subtotal per row-key (column rolled up), one grand total (all rolled up)}, which is what
 * this function classifies via the `__grp_<i>` markers (1 = rolled up). Multi-level pivots
 * (multiple row or column dimensions) produce intermediate groupings this flat model does
 * not handle and are out of scope here — the explore UI constrains pivots accordingly. */
export function buildPivot(
  columns: string[],
  rows: FlatRow[],
  rowDims: string[],
  colDims: string[],
  metric: string,
): Pivot {
  const idx = (name: string) => columns.indexOf(name);
  const rowDimIdx = rowDims.map(idx);
  const colDimIdx = colDims.map(idx);
  const metricIdx = idx(metric);
  const grpIdx = columns.map((c, i) => (c.startsWith("__grp_") ? i : -1)).filter((i) => i >= 0);
  // grouping markers align with the GROUP BY ROLLUP order = rowDims then colDims.
  const nRow = rowDims.length;

  const colKeySet = new Map<string, string[]>();
  const out: PivotRow[] = [];
  let grand: Record<string, number | string | null> | null = null;
  const byRowKey = new Map<string, PivotRow>();

  for (const r of rows) {
    const markers = grpIdx.map((i) => Number(r[i]) === 1);
    const rowRolled = markers.slice(0, nRow).some(Boolean);
    const colRolled = markers.slice(nRow).some(Boolean);
    const value = r[metricIdx] as number | string | null;

    if (rowRolled && colRolled) {
      grand = grand ?? {};
      grand["__total"] = value;
      continue;
    }
    const rk = rowDimIdx.map((i) => r[i] as string | null);
    const ck = colDimIdx.map((i) => r[i] as string | null);

    if (colRolled) {
      // row total across columns → merge into this row-key's PivotRow as the "__total" cell
      const rkStr = key(rk);
      let pr = byRowKey.get(rkStr);
      if (!pr) {
        pr = { key: rk.map((v) => String(v ?? "")), cells: {}, isSubtotal: false };
        byRowKey.set(rkStr, pr);
        out.push(pr);
      }
      pr.cells["__total"] = value;
      continue;
    }
    // leaf cell
    const ckStr = key(ck);
    // Use the same null sentinel as key() so the renderer's colKeyStr lookup matches.
    if (!colKeySet.has(ckStr)) colKeySet.set(ckStr, ck.map((v) => (v == null ? "∅" : String(v))));
    const rkStr = key(rk);
    let pr = byRowKey.get(rkStr);
    if (!pr) {
      pr = { key: rk.map((v) => String(v ?? "")), cells: {}, isSubtotal: false };
      byRowKey.set(rkStr, pr);
      out.push(pr);
    }
    pr.cells[ckStr] = value;
  }

  return { colKeys: [...colKeySet.values()], rows: out, grandTotal: grand };
}

function fmt(v: number | string | null | undefined): string {
  if (v == null) return "";
  return typeof v === "number" ? v.toLocaleString() : String(v);
}

export function PivotTable(props: {
  columns: string[];
  rows: FlatRow[];
  rowDims: string[];
  colDims: string[];
  metric: string;
}) {
  const p = buildPivot(props.columns, props.rows, props.rowDims, props.colDims, props.metric);
  const colKeyStr = (k: string[]) => k.join("|");
  return (
    <table className="w-full border-collapse text-sm">
      <thead>
        <tr className="border-b text-left text-muted-foreground">
          {props.rowDims.map((d) => <th key={d} className="py-1 pr-3">{d}</th>)}
          {p.colKeys.map((k) => <th key={colKeyStr(k)} className="px-3 text-right">{k.join(" / ")}</th>)}
          <th className="px-3 text-right font-semibold">Total</th>
        </tr>
      </thead>
      <tbody>
        {p.rows.map((r, i) => (
          <tr key={i} className="border-t">
            {r.key.map((v, j) => <td key={j} className="py-1 pr-3">{v}</td>)}
            {p.colKeys.map((k) => (
              <td key={colKeyStr(k)} className="px-3 text-right tabular-nums">
                {fmt(r.cells[colKeyStr(k)])}
              </td>
            ))}
            <td className="px-3 text-right font-semibold tabular-nums">{fmt(r.cells["__total"])}</td>
          </tr>
        ))}
        {p.grandTotal && (
          <tr className="border-t-2 bg-muted font-semibold">
            <td className="py-1 pr-3" colSpan={props.rowDims.length}>Grand total</td>
            {p.colKeys.map((k) => <td key={colKeyStr(k)} />)}
            <td className="px-3 text-right tabular-nums">{fmt(p.grandTotal["__total"])}</td>
          </tr>
        )}
      </tbody>
    </table>
  );
}
