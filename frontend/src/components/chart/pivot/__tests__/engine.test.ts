import { describe, it, expect } from "vitest";
import { indexResult, type PivotInput } from "../engine";

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
