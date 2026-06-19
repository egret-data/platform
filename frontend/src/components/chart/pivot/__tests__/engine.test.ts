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
    expect(p.colLeaves.length).toBe(3); // SMB, ENT, + Total
    expect(p.colLeaves[2].isTotal).toBe(true);
  });
  it("places leaf values and the row total", () => {
    const eu = p.bodyRows[0];
    expect(eu.kind).toBe("leaf");
    expect(eu.values).toEqual([10, 20, 30]); // SMB=10, ENT=20, Total=30
  });
  it("grand total row totals across rows", () => {
    expect(p.bodyRows[2].values).toEqual([15, 20, 35]); // SMB col=15, ENT col=20, grand=35
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
