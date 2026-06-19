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

describe("buildPivot null column value", () => {
  it("keeps a genuinely-null column-dimension cell reachable", () => {
    const columns = ["region", "segment", "revenue", "__grp_0", "__grp_1"];
    const rows: FlatRow[] = [
      ["EU", null, 7, 0, 0],   // a real row whose segment value is NULL (not a rollup)
      ["EU", null, 7, 0, 1],   // EU subtotal (segment rolled up)
      [null, null, 7, 1, 1],   // grand total
    ];
    const p = buildPivot(columns, rows, ["region"], ["segment"], "revenue");
    // the null-valued column key is represented with the "∅" sentinel and its cell is reachable
    expect(p.colKeys).toEqual([["∅"]]);
    const eu = p.rows.find((r) => r.key.join("|") === "EU" && !r.isSubtotal)!;
    expect(eu.cells["∅"]).toBe(7);
  });
});
