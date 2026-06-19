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

export interface ColLeaf {
  colPath: (string | null)[];
  metric: string;
  isTotal: boolean;        // the right-hand "Total" column group (colPath = [])
}
export interface TopCell {
  label: string;
  colSpan: number;
  rowSpan?: number;
}
export interface RowHeaderCell {
  label: string;
  depth: number;           // which row-dim column it starts at
  rowSpan: number;
  colSpan: number;
}
export interface BodyRow {
  rowPath: (string | null)[];
  kind: "leaf" | "subtotal" | "grand";
  headerCells: RowHeaderCell[]; // only cells that START on this row
  values: string[];             // formatted, aligned to colLeaves
}
export interface Pivot {
  rowDimNames: string[];
  topHeader: TopCell[][];       // one array per top-header level
  colLeaves: ColLeaf[];
  bodyRows: BodyRow[];
}

const fmt = (v: CellValue): string => {
  if (v == null) return "";
  if (typeof v === "number") return v.toLocaleString();
  const n = Number(v);
  return typeof v === "string" && v.trim() !== "" && Number.isFinite(n)
    ? n.toLocaleString()
    : String(v);
};

interface TreeNode {
  value: string | null;
  children: TreeNode[];
  childByValue: Map<string | null, TreeNode>;
}
const newNode = (value: string | null): TreeNode => ({ value, children: [], childByValue: new Map() });

function buildTree(paths: (string | null)[][]): TreeNode {
  const root = newNode(null);
  for (const path of paths) {
    let cur = root;
    for (const v of path) {
      let next = cur.childByValue.get(v);
      if (!next) {
        next = newNode(v);
        cur.childByValue.set(v, next);
        cur.children.push(next);
      }
      cur = next;
    }
  }
  return root;
}

export function buildPivot(input: PivotInput): Pivot {
  const ix = indexResult(input);
  const m = input.rowDims.length;
  const rowTree = buildTree(ix.rowLeafPaths);
  const colTree = buildTree(ix.colLeafPaths);

  // ----- column leaves (full col paths × metrics, + optional Total group) -----
  const colLeafPaths: (string | null)[][] = [];
  (function collect(node: TreeNode, path: (string | null)[]) {
    if (node.children.length === 0) {
      if (path.length > 0) colLeafPaths.push(path);
      return;
    }
    for (const ch of node.children) collect(ch, [...path, ch.value]);
  })(colTree, []);
  const effectiveColPaths = colLeafPaths.length ? colLeafPaths : [[]];

  const colLeaves: ColLeaf[] = [];
  for (const cp of effectiveColPaths) {
    for (const metric of input.metrics) colLeaves.push({ colPath: cp, metric, isTotal: false });
  }
  if (input.showRowTotals && input.colDims.length > 0) {
    for (const metric of input.metrics) colLeaves.push({ colPath: [], metric, isTotal: true });
  }

  // ----- top header levels (merge consecutive same-prefix column cells) -----
  const C = input.colDims.length;
  const nMetric = input.metrics.length;
  const topHeader: TopCell[][] = [];
  for (let d = 0; d < C; d++) {
    const level: TopCell[] = [];
    let g = 0;
    while (g < effectiveColPaths.length) {
      const pref = JSON.stringify(effectiveColPaths[g].slice(0, d + 1));
      let count = 0;
      while (
        g + count < effectiveColPaths.length &&
        JSON.stringify(effectiveColPaths[g + count].slice(0, d + 1)) === pref
      ) {
        count++;
      }
      level.push({ label: String(effectiveColPaths[g][d] ?? ""), colSpan: count * nMetric });
      g += count;
    }
    // the right-hand "Total" group spans all column-dim header levels — emit once on level 0.
    if (input.showRowTotals && d === 0) {
      level.push({ label: "Total", colSpan: nMetric, rowSpan: C });
    }
    topHeader.push(level);
  }
  // metric sub-header (only when more than one metric) — one cell per colLeaf, aligned to values.
  if (nMetric > 1 || C === 0) {
    topHeader.push(colLeaves.map((cl) => ({ label: cl.metric, colSpan: 1 })));
  }

  // ----- body rows (depth-first over row tree, inserting subtotals) -----
  const bodyRows: BodyRow[] = [];

  function rowValues(rowPath: (string | null)[]): string[] {
    return colLeaves.map((cl) => fmt(ix.cell(rowPath, cl.colPath, cl.metric)));
  }

  // returns the number of body rows produced for `node`'s subtree, EXCLUDING node's own subtotal row.
  function emit(node: TreeNode, path: (string | null)[], depth: number): number {
    if (node.children.length === 0) {
      bodyRows.push({ rowPath: path, kind: "leaf", headerCells: [], values: rowValues(path) });
      return 1;
    }
    let span = 0;
    for (const ch of node.children) {
      const childStart = bodyRows.length;
      const childSpan = emit(ch, [...path, ch.value], depth + 1);
      bodyRows[childStart].headerCells.push({
        label: String(ch.value ?? ""),
        depth: depth + 1,
        rowSpan: childSpan,
        colSpan: 1,
      });
      span += childSpan;
      const childHasSubtotal =
        input.showColTotals && ch.children.length > 0 && (!input.condense || ch.children.length > 1);
      if (childHasSubtotal) {
        const subPath = [...path, ch.value];
        bodyRows.push({
          rowPath: subPath,
          kind: "subtotal",
          headerCells: [{ label: `Totals for ${String(ch.value ?? "")}`, depth: depth + 1, rowSpan: 1, colSpan: m - (depth + 1) }],
          values: rowValues(subPath),
        });
        span += 1;
      }
    }
    return span;
  }

  // top-level row groups (depth 0)
  for (const ch of rowTree.children) {
    const start = bodyRows.length;
    const childSpan = emit(ch, [ch.value], 0);
    bodyRows[start].headerCells.unshift({
      label: String(ch.value ?? ""),
      depth: 0,
      rowSpan: childSpan,
      colSpan: 1,
    });
    const hasSubtotal =
      input.showColTotals && ch.children.length > 0 && (!input.condense || ch.children.length > 1);
    if (hasSubtotal) {
      const subPath = [ch.value];
      bodyRows.push({
        rowPath: subPath,
        kind: "subtotal",
        headerCells: [{ label: `Totals for ${String(ch.value ?? "")}`, depth: 0, rowSpan: 1, colSpan: m }],
        values: rowValues(subPath),
      });
    }
  }

  if (input.showColTotals) {
    bodyRows.push({
      rowPath: [],
      kind: "grand",
      headerCells: [{ label: "Grand totals", depth: 0, rowSpan: 1, colSpan: m }],
      values: rowValues([]),
    });
  }

  for (const br of bodyRows) br.headerCells.sort((a, b) => a.depth - b.depth);

  return { rowDimNames: input.rowDims, topHeader, colLeaves, bodyRows };
}
