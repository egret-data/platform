import { describe, it, expect } from "vitest";
import { pivotCellDrill, tableCellDrill, drillFilters } from "../drill";

describe("drill filter-builder", () => {
  it("pivotCellDrill: leaf cell yields all row+col dims, skipping nulls", () => {
    const ctx = pivotCellDrill(["region", "seg"], ["EU", "SMB"], ["year"], ["2024"]);
    expect(ctx.dims).toEqual([
      { col: "region", value: "EU" }, { col: "seg", value: "SMB" }, { col: "year", value: "2024" },
    ]);
  });
  it("pivotCellDrill: subtotal (short path) yields partial dims; grand yields none", () => {
    expect(pivotCellDrill(["region", "seg"], ["EU"], [], []).dims).toEqual([{ col: "region", value: "EU" }]);
    expect(pivotCellDrill(["region"], [null], [], []).dims).toEqual([]);
    expect(pivotCellDrill(["region"], [], [], []).dims).toEqual([]);
  });
  it("tableCellDrill: only dimension columns, nulls skipped", () => {
    const ctx = tableCellDrill(["region", "seg"], ["region", "seg", "revenue"], ["EU", null, 100]);
    expect(ctx.dims).toEqual([{ col: "region", value: "EU" }]);
  });
  it("drillFilters: one eq filter per dim", () => {
    expect(drillFilters({ dims: [{ col: "region", value: "EU" }] })).toEqual([
      { col: "region", op: "eq", value: "EU" },
    ]);
  });
});
