export interface PivotInput {
  columns: string[];          // result column names
  rows: unknown[][];          // result rows
  rowDims: string[];          // ordered row dimension names
  colDims: string[];          // ordered column dimension names
  metrics: string[];          // ordered metric names
  showRowTotals: boolean;     // right "Total" column (collapse all col dims)
  showColTotals: boolean;     // nested row-subtotal rows + bottom grand-total row
  condense: boolean;          // drop a subtotal row when its group has one child
  collapsed: Set<string>;     // serialized row-group paths that are collapsed
}

export type CellValue = number | string | null;

/** key for the value map: JSON of [rowPath, colPath]. */
const pathKey = (rowPath: (string | null)[], colPath: (string | null)[]) =>
  JSON.stringify([rowPath, colPath]);

export interface Indexed {
  rowLeafPaths: (string | null)[][];   // distinct full row paths, first-appearance order
  colLeafPaths: (string | null)[][];   // distinct full col paths, first-appearance order
  cell: (rowPath: (string | null)[], colPath: (string | null)[], metric: string) => CellValue;
}

/** Classify each flat ROLLUP×ROLLUP row and build a value map keyed by (rowPath, colPath). */
export function indexResult(input: PivotInput): Indexed {
  const idx = (name: string) => input.columns.indexOf(name);
  const m = input.rowDims.length;
  const n = input.colDims.length;
  const rowDimIdx = input.rowDims.map(idx);
  const colDimIdx = input.colDims.map(idx);
  const metricIdx = new Map(input.metrics.map((mn) => [mn, idx(mn)]));
  // marker for dimensions[k] is __grp_k, dimensions = rowDims ++ colDims
  const rowGrpIdx = input.rowDims.map((_, k) => idx(`__grp_${k}`));
  const colGrpIdx = input.colDims.map((_, k) => idx(`__grp_${m + k}`));

  const values = new Map<string, Record<string, CellValue>>();
  const rowLeafPaths: (string | null)[][] = [];
  const colLeafPaths: (string | null)[][] = [];
  const seenRow = new Set<string>();
  const seenCol = new Set<string>();

  for (const r of input.rows) {
    // active prefix length = count of leading dims with GROUPING marker 0
    let i = 0;
    while (i < m && Number(r[rowGrpIdx[i]]) === 0) i++;
    let j = 0;
    while (j < n && Number(r[colGrpIdx[j]]) === 0) j++;
    const rowPath = rowDimIdx.slice(0, i).map((c) => r[c] as string | null);
    const colPath = colDimIdx.slice(0, j).map((c) => r[c] as string | null);
    const rec: Record<string, CellValue> = {};
    for (const [mn, mi] of metricIdx) rec[mn] = r[mi] as CellValue;
    values.set(pathKey(rowPath, colPath), rec);
    if (i === m && j === n) {
      const rk = JSON.stringify(rowPath);
      if (!seenRow.has(rk)) {
        seenRow.add(rk);
        rowLeafPaths.push(rowPath);
      }
      const ck = JSON.stringify(colPath);
      if (!seenCol.has(ck)) {
        seenCol.add(ck);
        colLeafPaths.push(colPath);
      }
    }
  }

  const cell = (rowPath: (string | null)[], colPath: (string | null)[], metric: string): CellValue => {
    const rec = values.get(pathKey(rowPath, colPath));
    return rec ? rec[metric] ?? null : null;
  };

  return { rowLeafPaths, colLeafPaths, cell };
}
