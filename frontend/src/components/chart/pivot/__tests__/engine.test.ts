import { describe, it, expect } from "vitest";
import { indexResult, buildPivot, type PivotInput } from "../engine";

// 1 row dim (region) × 1 col dim (seg) × 1 metric (revenue), ROLLUP×ROLLUP result.
// columns order: region, seg, revenue, __grp_0 (region marker), __grp_1 (seg marker)
const input: PivotInput = {
  columns: ["region", "seg", "revenue", "__grp_0", "__grp_1"],
  rows: [
    ["EU", "SMB", 10, 0, 0],   // leaf
    ["EU", "ENT", 20, 0, 0],   // leaf
    ["EU", null, 30, 0, 1],    // EU row-subtotal (seg rolled)
    ["US", "SMB", 5, 0, 0],    // leaf
    ["US", null, 5, 0, 1],     // US row-subtotal
    [null, "SMB", 15, 1, 0],   // SMB col-subtotal (region rolled)
    [null, "ENT", 20, 1, 0],   // ENT col-subtotal
    [null, null, 35, 1, 1],    // grand total
  ],
  rowDims: ["region"],
  colDims: ["seg"],
  metrics: ["revenue"],
  showRowTotals: true,
  showColTotals: true,
  condense: true,
  collapsed: new Set<string>(),
};

describe("indexResult", () => {
  const ix = indexResult(input);

  it("enumerates ordered row and col leaf paths", () => {
    expect(ix.rowLeafPaths).toEqual([["EU"], ["US"]]);
    expect(ix.colLeafPaths).toEqual([["SMB"], ["ENT"]]);
  });
  it("answers leaf cells", () => {
    expect(ix.cell(["EU"], ["SMB"], "revenue")).toBe(10);
    expect(ix.cell(["US"], ["SMB"], "revenue")).toBe(5);
  });
  it("answers row subtotals (col path empty)", () => {
    expect(ix.cell(["EU"], [], "revenue")).toBe(30);
    expect(ix.cell(["US"], [], "revenue")).toBe(5);
  });
  it("answers col subtotals (row path empty)", () => {
    expect(ix.cell([], ["SMB"], "revenue")).toBe(15);
  });
  it("answers the grand total (both empty)", () => {
    expect(ix.cell([], [], "revenue")).toBe(35);
  });
  it("returns null for a missing cell", () => {
    expect(ix.cell(["EU"], ["NOPE"], "revenue")).toBeNull();
  });
});

describe("indexResult edge cases", () => {
  it("handles colDims empty (table-as-pivot)", () => {
    const ix = indexResult({
      columns: ["region", "revenue", "__grp_0"],
      rows: [["EU", 10, 0], ["US", 5, 0], [null, 15, 1]],
      rowDims: ["region"], colDims: [], metrics: ["revenue"],
      showRowTotals: true, showColTotals: true, condense: true, collapsed: new Set(),
    });
    expect(ix.rowLeafPaths).toEqual([["EU"], ["US"]]);
    expect(ix.colLeafPaths).toEqual([[]]);
    expect(ix.cell(["EU"], [], "revenue")).toBe(10);
    expect(ix.cell([], [], "revenue")).toBe(15); // grand total
  });
});

describe("buildPivot single-dim (1 row × 1 col × 1 metric)", () => {
  const p = buildPivot(input);

  it("has one body row per row leaf plus a grand-total row", () => {
    expect(p.bodyRows.map((b) => b.kind)).toEqual(["leaf", "leaf", "grand"]); // EU, US, Grand total
  });
  it("col leaves are the distinct columns, each metric, plus a Total column", () => {
    expect(p.colLeaves.length).toBe(3); // ENT, SMB, + Total
    expect(p.colLeaves[2].isTotal).toBe(true);
  });
  // Columns sort ascending, so ENT precedes SMB regardless of which appeared first in the result.
  it("places leaf values and the row total", () => {
    const eu = p.bodyRows[0];
    expect(eu.kind).toBe("leaf");
    expect(eu.values).toEqual([20, 10, 30]); // ENT=20, SMB=10, Total=30
  });
  it("grand total row totals across rows", () => {
    expect(p.bodyRows[2].values).toEqual([20, 15, 35]); // ENT col=20, SMB col=15, grand=35
  });
});

describe("buildPivot nested rows (2 row × 1 col)", () => {
  const nested: PivotInput = {
    columns: ["region", "nation", "seg", "revenue", "__grp_0", "__grp_1", "__grp_2"],
    rows: [
      ["EU", "DE", "SMB", 4, 0, 0, 0],
      ["EU", "FR", "SMB", 6, 0, 0, 0],
      ["EU", "DE", null, 4, 0, 0, 1],   // DE row subtotal
      ["EU", "FR", null, 6, 0, 0, 1],   // FR row subtotal
      ["EU", null, "SMB", 10, 0, 1, 0], // EU subtotal (nation rolled), seg full
      ["EU", null, null, 10, 0, 1, 1],  // EU total across seg
      [null, null, "SMB", 10, 1, 1, 0],
      [null, null, null, 10, 1, 1, 1],  // grand total
    ],
    rowDims: ["region", "nation"], colDims: ["seg"], metrics: ["revenue"],
    showRowTotals: true, showColTotals: true, condense: true, collapsed: new Set(),
  };
  const p = buildPivot(nested);

  it("emits leaves, a region subtotal row, and a grand-total row", () => {
    expect(p.bodyRows.map((b) => b.kind)).toEqual(["leaf", "leaf", "subtotal", "grand"]);
  });
  it("the region cell rowSpans its two leaf rows", () => {
    const region = p.bodyRows[0].headerCells.find((c) => c.depth === 0)!;
    expect(region.rowSpan).toBe(2);
  });
});

describe("buildPivot 3 row dims — header cell depth order", () => {
  // region → nation → city, 1 col dim seg. Markers __grp_0..3.
  const p = buildPivot({
    columns: ["region", "nation", "city", "seg", "revenue", "__grp_0", "__grp_1", "__grp_2", "__grp_3"],
    rows: [
      ["EU", "DE", "BER", "SMB", 3, 0, 0, 0, 0],
      ["EU", "DE", "MUC", "SMB", 4, 0, 0, 0, 0],
      // DE city subtotals + region/nation rollups (only what the renderer needs for structure)
      ["EU", "DE", "BER", null, 3, 0, 0, 0, 1],
      ["EU", "DE", "MUC", null, 4, 0, 0, 0, 1],
      ["EU", "DE", null, "SMB", 7, 0, 0, 1, 0],
      ["EU", "DE", null, null, 7, 0, 0, 1, 1],
      ["EU", null, null, "SMB", 7, 0, 1, 1, 0],
      ["EU", null, null, null, 7, 0, 1, 1, 1],
      [null, null, null, "SMB", 7, 1, 1, 1, 0],
      [null, null, null, null, 7, 1, 1, 1, 1],
    ],
    rowDims: ["region", "nation", "city"], colDims: ["seg"], metrics: ["revenue"],
    showRowTotals: true, showColTotals: true, condense: true, collapsed: new Set(),
  });

  it("orders the first leaf row's header cells by depth (region, nation, city)", () => {
    const depths = p.bodyRows[0].headerCells.map((c) => c.depth);
    expect(depths).toEqual([...depths].sort((a, b) => a - b)); // ascending
    expect(depths[0]).toBe(0); // region first
  });
});

describe("buildPivot 2 col dims — merged outer header", () => {
  const p = buildPivot({
    columns: ["region", "ccat", "seg", "revenue", "__grp_0", "__grp_1", "__grp_2"],
    rows: [
      ["EU", "A", "SMB", 1, 0, 0, 0],
      ["EU", "A", "ENT", 2, 0, 0, 0],
      ["EU", "B", "SMB", 3, 0, 0, 0],
      ["EU", null, null, 6, 0, 1, 1],
      [null, null, null, 6, 1, 1, 1],
    ],
    rowDims: ["region"], colDims: ["ccat", "seg"], metrics: ["revenue"],
    showRowTotals: false, showColTotals: true, condense: true, collapsed: new Set(),
  });

  it("merges the outer (ccat) header: A spans its two seg leaves", () => {
    const level0 = p.topHeader[0];
    const a = level0.find((c) => c.label === "A")!;
    expect(a.colSpan).toBe(2); // A/SMB + A/ENT, nMetric=1
    const b = level0.find((c) => c.label === "B")!;
    expect(b.colSpan).toBe(1);
  });
  it("has a second header level for the inner (seg) dim", () => {
    expect(p.topHeader.length).toBe(2);
  });
});

describe("buildPivot no column dims (grouped table)", () => {
  const p = buildPivot({
    columns: ["region", "nation", "revenue", "__grp_0", "__grp_1"],
    rows: [
      ["EU", "DE", 4, 0, 0],
      ["EU", "FR", 6, 0, 0],
      ["EU", null, 10, 0, 1],   // EU subtotal
      [null, null, 10, 1, 1],   // grand total
    ],
    rowDims: ["region", "nation"], colDims: [], metrics: ["revenue"],
    showRowTotals: true, showColTotals: true, condense: true, collapsed: new Set(),
  });

  it("has exactly one value column (no duplicate Total column)", () => {
    expect(p.colLeaves.length).toBe(1);
    expect(p.colLeaves[0].isTotal).toBe(false);
  });
  it("each body row has exactly one value", () => {
    for (const br of p.bodyRows) expect(br.values.length).toBe(1);
  });
  it("renders a header row (metric level) so columns are labeled", () => {
    expect(p.topHeader.length).toBe(1);
    expect(p.topHeader[0].some((c) => c.label === "revenue")).toBe(true);
  });
});

// 2 row dims (region, seg) × 0 col dims × 1 metric. ROLLUP(region, seg).
const rowGrouped: PivotInput = {
  columns: ["region", "seg", "revenue", "__grp_0", "__grp_1"],
  rows: [
    ["EU", "SMB", 10, 0, 0],
    ["EU", "ENT", 20, 0, 0],
    ["EU", null, 30, 0, 1],   // EU subtotal
    ["US", "SMB", 5, 0, 0],
    ["US", "ENT", 7, 0, 0],
    ["US", null, 12, 0, 1],   // US subtotal
    [null, null, 42, 1, 1],   // grand total
  ],
  rowDims: ["region", "seg"], colDims: [], metrics: ["revenue"],
  showRowTotals: false, showColTotals: true, condense: true, collapsed: new Set<string>(),
};

describe("collapse", () => {
  it("marks subtotal-bearing groups collapsible when expanded", () => {
    const p = buildPivot(rowGrouped);
    const euCell = p.bodyRows.flatMap((r) => r.headerCells).find((h) => h.label === "EU");
    expect(euCell?.collapsible).toBe(true);
    expect(euCell?.collapsed).toBe(false);
    expect(euCell?.path).toEqual(["EU"]);
    expect(p.bodyRows.some((r) => JSON.stringify(r.rowPath) === JSON.stringify(["EU", "SMB"]))).toBe(true);
  });

  it("collapsing a group hides its children and shows a collapsed subtotal row", () => {
    const p = buildPivot({ ...rowGrouped, collapsed: new Set(['["EU"]']) });
    expect(p.bodyRows.some((r) => JSON.stringify(r.rowPath) === JSON.stringify(["EU", "SMB"]))).toBe(false);
    const euRow = p.bodyRows.find((r) => JSON.stringify(r.rowPath) === JSON.stringify(["EU"]) && r.kind === "subtotal");
    expect(euRow).toBeTruthy();
    expect(euRow!.values[0]).toBe(30);
    const euCell = euRow!.headerCells.find((h) => h.label === "EU");
    expect(euCell?.collapsible).toBe(true);
    expect(euCell?.collapsed).toBe(true);
    expect(p.bodyRows.some((r) => JSON.stringify(r.rowPath) === JSON.stringify(["US", "SMB"]))).toBe(true);
  });
});

describe("dimension sort", () => {
  it("sorts a row dimension descending when requested", () => {
    const p = buildPivot({ ...rowGrouped, sortDims: { region: "desc" } });
    const firstGroupLabel = p.bodyRows.flatMap((r) => r.headerCells).find((h) => h.depth === 0)?.label;
    expect(firstGroupLabel).toBe("US");
  });

  it("sorts ascending when no sort is set", () => {
    const p = buildPivot(rowGrouped);
    const firstGroupLabel = p.bodyRows.flatMap((r) => r.headerCells).find((h) => h.depth === 0)?.label;
    expect(firstGroupLabel).toBe("EU");
  });
});

/* ── Column ordering, per-dimension subtotals ──────────────────────────────────────────────── */

/** Column values supplied deliberately OUT of order — the shape a month-bucketed axis arrives in,
 * because the pivot's query sets `orderby: []` and takes whatever the engine returned. Row values
 * are out of order too, and reverse-alphabetical, so "sorted" and "as it arrived" cannot agree by
 * accident the way the case above does. */
const unordered: PivotInput = {
  columns: ["region", "cat", "d", "revenue", "__grp_0", "__grp_1", "__grp_2"],
  rows: [
    ["US", "SMB", "2026-02", 10, 0, 0, 0],
    ["US", "SMB", "2026-01", 11, 0, 0, 0],
    ["US", "ENT", "2026-03", 12, 0, 0, 0],
    ["EU", "SMB", "2026-01", 13, 0, 0, 0],
    ["US", null, "2026-01", 33, 0, 1, 0],
    ["EU", null, "2026-01", 13, 0, 1, 0],
    [null, null, null, 46, 1, 1, 1],
  ],
  rowDims: ["region", "cat"],
  colDims: ["d"],
  metrics: ["revenue"],
  showRowTotals: false,
  showColTotals: true,
  condense: false,
  collapsed: new Set(),
};

describe("column ordering", () => {
  it("sorts column values ascending rather than keeping arrival order", () => {
    // Arrival order is 2026-02, 2026-01, 2026-03. A time axis rendered out of sequence is wrong in
    // a way the reader cannot recover from, and the order was not even stable between runs.
    expect(buildPivot(unordered).topHeader[0].map((c) => c.label))
      .toEqual(["2026-01", "2026-02", "2026-03"]);
  });

  it("sorts row groups ascending even when they arrived reversed", () => {
    const labels = buildPivot(unordered).bodyRows
      .flatMap((r) => r.headerCells)
      .filter((h) => h.depth === 0 && !/^(Totals|Grand)/.test(h.label))
      .map((h) => h.label);
    expect(labels).toEqual(["EU", "US"]);
  });
});

describe("per-dimension subtotals", () => {
  const subtotals = (i: PivotInput) =>
    buildPivot(i).bodyRows
      .filter((r) => r.kind === "subtotal")
      .flatMap((r) => r.headerCells.map((h) => h.label));

  it("shows a band for an outer dimension by default", () => {
    expect(subtotals(unordered)).toEqual(["Totals for EU", "Totals for US"]);
  });

  it("drops the band when that dimension is switched off, leaving leaves and the grand total", () => {
    const p = buildPivot({ ...unordered, dimTotals: { region: false } });
    expect(p.bodyRows.filter((r) => r.kind === "subtotal")).toHaveLength(0);
    expect(p.bodyRows.some((r) => r.kind === "grand")).toBe(true);
    // three distinct leaf row PATHS — (US,SMB) occurs twice in the data, once per date column
    expect(p.bodyRows.filter((r) => r.kind === "leaf")).toHaveLength(3);
  });

  it("an explicit true matches the default", () => {
    expect(subtotals({ ...unordered, dimTotals: { region: true } }))
      .toEqual(["Totals for EU", "Totals for US"]);
  });

  it("the global switch still overrides a per-dimension true", () => {
    const p = buildPivot({ ...unordered, showColTotals: false, dimTotals: { region: true } });
    expect(p.bodyRows.filter((r) => r.kind === "subtotal")).toHaveLength(0);
    expect(p.bodyRows.some((r) => r.kind === "grand")).toBe(false);
  });
});

describe("metric sub-header", () => {
  const oneMetric: PivotInput = {
    columns: ["region", "d", "revenue", "__grp_0", "__grp_1"],
    rows: [["EU", "2026-01", 10, 0, 0], ["EU", null, 10, 0, 1], [null, null, 10, 1, 1]],
    rowDims: ["region"], colDims: ["d"], metrics: ["revenue"],
    showRowTotals: false, showColTotals: true, condense: false, collapsed: new Set(),
  };

  it("is omitted for a single unnamed metric under a column dimension", () => {
    const p = buildPivot(oneMetric);
    expect(p.topHeader).toHaveLength(1);
    expect(p.topHeader[0].some((c) => c.metric)).toBe(false);
  });

  it("appears when that metric is renamed, so the title has somewhere to show", () => {
    // Without this, typing a Column title for the only metric changed nothing on screen.
    const p = buildPivot({ ...oneMetric, metricTitles: { revenue: "Doanh thu" } });
    expect(p.topHeader).toHaveLength(2);
    expect(p.topHeader[1].every((c) => c.metric === "revenue")).toBe(true);
  });

  it("an empty title is not a rename", () => {
    expect(buildPivot({ ...oneMetric, metricTitles: { revenue: "" } }).topHeader).toHaveLength(1);
  });
});
