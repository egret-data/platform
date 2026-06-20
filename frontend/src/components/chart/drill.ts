import type { ChartFilter } from "@/api/charts";

export interface DrillContext { dims: { col: string; value: unknown }[] }

/** Pivot value cell → the dims of its (rowPath, colPath), skipping null/short segments. */
export function pivotCellDrill(
  rowDims: string[], rowPath: (string | null)[],
  colDims: string[], colPath: (string | null)[],
): DrillContext {
  const dims: { col: string; value: unknown }[] = [];
  rowDims.forEach((c, i) => { if (i < rowPath.length && rowPath[i] != null) dims.push({ col: c, value: rowPath[i] }); });
  colDims.forEach((c, i) => { if (i < colPath.length && colPath[i] != null) dims.push({ col: c, value: colPath[i] }); });
  return { dims };
}

/** Aggregated-table value cell → the dims (dimension columns) of its row. */
export function tableCellDrill(
  dimensionColumns: string[], columns: string[], row: unknown[],
): DrillContext {
  const dims: { col: string; value: unknown }[] = [];
  for (const c of dimensionColumns) {
    const i = columns.indexOf(c);
    if (i >= 0 && row[i] != null) dims.push({ col: c, value: row[i] });
  }
  return { dims };
}

/** Build `=` filters from a drill context. */
export function drillFilters(ctx: DrillContext): ChartFilter[] {
  return ctx.dims.map((d) => ({ col: d.col, op: "eq" as const, value: d.value }));
}
