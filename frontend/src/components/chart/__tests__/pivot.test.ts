import { describe, it, expect } from "vitest";
import { buildPivot, type FlatRow } from "../PivotTable";

// columns returned by the engine for: rows=[region], cols=[segment], metric=revenue, rollup
const columns = ["region", "segment", "revenue", "__grp_0", "__grp_1"];
const rows: FlatRow[] = [
  ["EU", "SMB", 10, 0, 0],
  ["EU", "ENT", 20, 0, 0],
  ["EU", null, 30, 0, 1], // subtotal for EU
  ["US", "SMB", 5, 0, 0],
  ["US", null, 5, 0, 1],  // subtotal for US
  [null, null, 35, 1, 1], // grand total
];

describe("buildPivot", () => {
  const pivot = buildPivot(columns, rows, ["region"], ["segment"], "revenue");

  it("collects the distinct column keys", () => {
    expect(pivot.colKeys).toEqual([["SMB"], ["ENT"]]);
  });
  it("places leaf cells by (rowKey, colKey)", () => {
    const eu = pivot.rows.find((r) => r.key.join("|") === "EU" && !r.isSubtotal)!;
    expect(eu.cells["SMB"]).toBe(10);
    expect(eu.cells["ENT"]).toBe(20);
  });
  it("marks subtotal + grand-total rows from grouping markers", () => {
    expect(pivot.rows.some((r) => r.isSubtotal && r.key[0] === "EU")).toBe(true);
    expect(pivot.grandTotal).not.toBeNull();
  });
});
