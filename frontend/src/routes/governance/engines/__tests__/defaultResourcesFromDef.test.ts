import { describe, it, expect } from "vitest";
import type { ServiceDefResource } from "@/api/policies";
import { defaultResourcesFromDef } from "@/routes/governance/engines/PolicyEditorPage";

// Shapes emitted by PolicyEditorPage's emptyLevel()/wildcardLevel() helpers.
const EMPTY = { values: [], isExcludes: false, isRecursive: false };
const WILD = { values: ["*"], isExcludes: false, isRecursive: false };

// These mirror the def's TOP-LEVEL `resources` (parent/level present) — the
// exact input PolicyEditorPage now feeds the helper for every kind. The
// masking/row-filter subsets (`dataMaskDef.resources`/`rowFilterDef.resources`)
// are flat parentless name-refs, so seeding must use this full hierarchy.

// StarRocks-shaped hierarchy: catalog → database → table → column.
const STARROCKS: ServiceDefResource[] = [
  { name: "catalog", parent: "", level: 10 },
  { name: "database", parent: "catalog", level: 20 },
  { name: "table", parent: "database", level: 30 },
  { name: "column", parent: "table", level: 40 },
];

// Trino-shaped hierarchy: catalog → schema → table → column.
const TRINO: ServiceDefResource[] = [
  { name: "catalog", parent: "", level: 10 },
  { name: "schema", parent: "catalog", level: 20 },
  { name: "table", parent: "schema", level: 30 },
  { name: "column", parent: "table", level: 40 },
];

describe("defaultResourcesFromDef", () => {
  it("builds the full StarRocks masking chain (catalog/database/table/column), root empty rest wildcard", () => {
    const out = defaultResourcesFromDef(STARROCKS, "masking");
    // Note: database, NOT schema — proves the chain follows the def, not a hardcode.
    expect(out).toEqual({
      catalog: EMPTY,
      database: WILD,
      table: WILD,
      column: WILD,
    });
    // Ordering of the keys follows the hierarchy root → leaf.
    expect(Object.keys(out)).toEqual(["catalog", "database", "table", "column"]);
  });

  it("stops the StarRocks row-filter chain at table (no column key)", () => {
    const out = defaultResourcesFromDef(STARROCKS, "rowfilter");
    expect(out).toEqual({
      catalog: EMPTY,
      database: WILD,
      table: WILD,
    });
    expect(out).not.toHaveProperty("column");
  });

  it("reproduces the Trino access chain (catalog/schema/table/column)", () => {
    const out = defaultResourcesFromDef(TRINO, "access");
    expect(out).toEqual({
      catalog: EMPTY,
      schema: WILD,
      table: WILD,
      column: WILD,
    });
  });

  it("returns an empty map (no crash) when there is no catalog root or usable root chain", () => {
    // A single standalone root with no children — e.g. a `role`-only def.
    const out = defaultResourcesFromDef([{ name: "role", parent: "", level: 10 }], "access");
    expect(out).toEqual({});
  });

  it("returns a single-key map for a bare catalog root with no children", () => {
    const out = defaultResourcesFromDef([{ name: "catalog", parent: "", level: 10 }], "access");
    expect(out).toEqual({ catalog: EMPTY });
  });
});
