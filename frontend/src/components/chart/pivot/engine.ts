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
  sortDims?: Record<string, "asc" | "desc">;  // row-dimension name -> direction; absent = natural order
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
  metric?: string;    // set when this cell is a metric sub-header, for column_title lookup
}
export interface RowHeaderCell {
  label: string;
  depth: number;           // which row-dim column it starts at
  rowSpan: number;
  colSpan: number;
  path?: (string | null)[]; // the group path this cell heads (for the collapse toggle)
  collapsible?: boolean;    // group has a subtotal → can be collapsed
  collapsed?: boolean;      // currently collapsed (renderer shows [+] vs [–])
}
export interface BodyRow {
  rowPath: (string | null)[];
  kind: "leaf" | "subtotal" | "grand";
  headerCells: RowHeaderCell[]; // only cells that START on this row
  values: CellValue[];          // raw, aligned to colLeaves
}
export interface Pivot {
  rowDimNames: string[];
  topHeader: TopCell[][];       // one array per top-header level
  colLeaves: ColLeaf[];
  bodyRows: BodyRow[];
}

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

// Total order over dimension values: null sorts as minimum (so it leads asc, trails desc),
// then all numeric-looking values (compared numerically) before all other strings — this keeps
// the comparator transitive even when a dimension mixes numbers and digit-leading text.
function cmpValues(a: string | null, b: string | null): number {
  if (a === b) return 0;
  if (a === null) return -1;
  if (b === null) return 1;
  const fa = a.trim() !== "" && Number.isFinite(Number(a));
  const fb = b.trim() !== "" && Number.isFinite(Number(b));
  if (fa && fb) return Number(a) - Number(b);
  if (fa !== fb) return fa ? -1 : 1;
  return a.localeCompare(b);
}

function sortTree(node: TreeNode, depth: number, dims: string[], sortDims: Record<string, "asc" | "desc">) {
  const dir = sortDims[dims[depth]];
  if (dir) {
    const sign = dir === "desc" ? -1 : 1;
    node.children.sort((a, b) => cmpValues(a.value, b.value) * sign);
  }
  for (const ch of node.children) sortTree(ch, depth + 1, dims, sortDims);
}

export function buildPivot(input: PivotInput): Pivot {
  const ix = indexResult(input);
  const m = input.rowDims.length;
  const rowTree = buildTree(ix.rowLeafPaths);
  sortTree(rowTree, 0, input.rowDims, input.sortDims ?? {});
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
    topHeader.push(colLeaves.map((cl) => ({ label: cl.metric, colSpan: 1, metric: cl.metric })));
  }

  // ----- body rows (depth-first over row tree, inserting subtotals; collapse-aware) -----
  const bodyRows: BodyRow[] = [];

  function rowValues(rowPath: (string | null)[]): CellValue[] {
    return colLeaves.map((cl) => ix.cell(rowPath, cl.colPath, cl.metric));
  }

  // emit body rows for `node`'s children, each occupying row-dim column `depth`. returns rows emitted.
  function emitChildren(node: TreeNode, path: (string | null)[], depth: number): number {
    let span = 0;
    for (const ch of node.children) {
      const childPath = [...path, ch.value];
      const isGroup = ch.children.length > 0;
      const hasSubtotal = input.showColTotals && isGroup && (!input.condense || ch.children.length > 1);

      if (!isGroup) {
        const start = bodyRows.length;
        bodyRows.push({ rowPath: childPath, kind: "leaf", headerCells: [], values: rowValues(childPath) });
        bodyRows[start].headerCells.push({ label: String(ch.value ?? ""), depth, rowSpan: 1, colSpan: 1 });
        span += 1;
        continue;
      }

      if (hasSubtotal && input.collapsed.has(JSON.stringify(childPath))) {
        // collapsed group → a single subtotal row carrying the group's aggregate + a collapsed toggle
        bodyRows.push({
          rowPath: childPath,
          kind: "subtotal",
          headerCells: [{ label: String(ch.value ?? ""), depth, rowSpan: 1, colSpan: m - depth, path: childPath, collapsible: true, collapsed: true }],
          values: rowValues(childPath),
        });
        span += 1;
        continue;
      }

      const start = bodyRows.length;
      const childSpan = emitChildren(ch, childPath, depth + 1);
      bodyRows[start].headerCells.push({
        label: String(ch.value ?? ""),
        depth,
        rowSpan: childSpan,
        colSpan: 1,
        ...(hasSubtotal ? { path: childPath, collapsible: true, collapsed: false } : {}),
      });
      span += childSpan;

      if (hasSubtotal) {
        bodyRows.push({
          rowPath: childPath,
          kind: "subtotal",
          headerCells: [{ label: `Totals for ${String(ch.value ?? "")}`, depth, rowSpan: 1, colSpan: m - depth }],
          values: rowValues(childPath),
        });
        span += 1;
      }
    }
    return span;
  }

  emitChildren(rowTree, [], 0);

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
