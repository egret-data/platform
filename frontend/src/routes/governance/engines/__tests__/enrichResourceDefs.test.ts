import { describe, it, expect } from "vitest";
import type { ServiceDefResource } from "@/api/policies";
import { enrichResourceDefs } from "@/routes/governance/engines/PolicyEditorPage";

// The full top-level `resources` carry parent/level/flags; the dataMaskDef/
// rowFilterDef subsets are flat name-only refs. enrichResourceDefs maps the
// subset onto the full entries so ResourceSelector can cascade them.
const FULL: ServiceDefResource[] = [
  { name: "catalog", parent: "", level: 10, mandatory: true, isValidLeaf: true },
  { name: "database", parent: "catalog", level: 20, mandatory: true, isValidLeaf: true },
  { name: "table", parent: "database", level: 30, mandatory: true, isValidLeaf: true },
  { name: "column", parent: "table", level: 40, isValidLeaf: true },
  { name: "view", parent: "database", level: 30, isValidLeaf: true },
];

describe("enrichResourceDefs", () => {
  it("gives the flat masking subset its parent/level from the full hierarchy", () => {
    const subset: ServiceDefResource[] = [
      { name: "catalog" },
      { name: "database" },
      { name: "table" },
      { name: "column" },
    ];
    const out = enrichResourceDefs(subset, FULL);
    expect(out.map((r) => [r.name, r.parent, r.level])).toEqual([
      ["catalog", "", 10],
      ["database", "catalog", 20],
      ["table", "database", 30],
      ["column", "table", 40],
    ]);
    // Flags come along too (needed for the required-field + leaf logic).
    expect(out.find((r) => r.name === "database")?.mandatory).toBe(true);
  });

  it("stops the row-filter subset at table (only its members, but now parented)", () => {
    const subset: ServiceDefResource[] = [
      { name: "catalog" },
      { name: "database" },
      { name: "table" },
    ];
    const out = enrichResourceDefs(subset, FULL);
    expect(out.map((r) => r.name)).toEqual(["catalog", "database", "table"]);
    expect(out.find((r) => r.name === "table")?.parent).toBe("database");
    expect(out.some((r) => r.name === "column")).toBe(false);
  });

  it("preserves subset order/length and keeps a name-only entry when full has no match", () => {
    const subset: ServiceDefResource[] = [{ name: "catalog" }, { name: "mystery" }];
    const out = enrichResourceDefs(subset, FULL);
    expect(out.map((r) => r.name)).toEqual(["catalog", "mystery"]);
    // Unmatched entry is returned unchanged (no crash, no invented parent).
    expect(out[1]).toEqual({ name: "mystery" });
  });

  it("returns an empty array for an empty subset", () => {
    expect(enrichResourceDefs([], FULL)).toEqual([]);
  });
});
