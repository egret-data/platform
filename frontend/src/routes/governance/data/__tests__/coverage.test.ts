import { describe, expect, it } from "vitest";
import type { Grant, Scope } from "@/api/generated/model";
import { globMatch, grantCoverage } from "@/routes/governance/data/coverage";
import type { ObjectPath } from "@/routes/governance/data/objectPath";

const CAT = "c-ice";

function grant(scope: Omit<Scope, "catalog_id">, catalogId = CAT): Grant {
  return {
    id: "g",
    effect: "allow",
    actions: ["read"],
    scope: { catalog_id: catalogId, ...scope },
    principals: { include: [{ kind: "role", ref: "analyst" }], exclude: [] },
    origin: { type: "explicit" },
  };
}

const CATALOG: ObjectPath = { catalog: "ice" };
const SCHEMA: ObjectPath = { catalog: "ice", schema: "sales" };
const TABLE: ObjectPath = { catalog: "ice", schema: "sales", object: "orders" };
const COLUMN: ObjectPath = { catalog: "ice", schema: "sales", object: "orders", column: "email" };

const ON_CATALOG = grant({});
const ON_SCHEMA = grant({ schema: "sales" });
const ON_TABLE = grant({ schema: "sales", table: "orders" });
const ON_COLUMN = grant({ schema: "sales", table: "orders", column: "email" });

describe("grantCoverage", () => {
  // Arrange: one grant at each level; Act/Assert: its reach at each level of p.
  it.each([
    // p = catalog
    ["catalog", ON_CATALOG, CATALOG, "here"],
    ["schema", ON_SCHEMA, CATALOG, null],
    ["table", ON_TABLE, CATALOG, null],
    ["column", ON_COLUMN, CATALOG, null],
    // p = schema
    ["catalog", ON_CATALOG, SCHEMA, { inheritedFrom: "catalog" }],
    ["schema", ON_SCHEMA, SCHEMA, "here"],
    ["table", ON_TABLE, SCHEMA, null],
    ["column", ON_COLUMN, SCHEMA, null],
    // p = table
    ["catalog", ON_CATALOG, TABLE, { inheritedFrom: "catalog" }],
    ["schema", ON_SCHEMA, TABLE, { inheritedFrom: "schema" }],
    ["table", ON_TABLE, TABLE, "here"],
    ["column", ON_COLUMN, TABLE, "child"],
    // p = column
    ["catalog", ON_CATALOG, COLUMN, { inheritedFrom: "catalog" }],
    ["schema", ON_SCHEMA, COLUMN, { inheritedFrom: "schema" }],
    ["table", ON_TABLE, COLUMN, { inheritedFrom: "table" }],
    ["column", ON_COLUMN, COLUMN, "here"],
  ] as const)("a %s grant at %j", (_level, g, p, expected) => {
    expect(grantCoverage(g, p, CAT)).toEqual(expected);
  });

  it.each([
    ["another schema", grant({ schema: "hr" }), TABLE],
    ["another table", grant({ schema: "sales", table: "customers" }), TABLE],
    ["the same table name in another schema", grant({ schema: "hr", table: "orders" }), TABLE],
    ["another column", grant({ schema: "sales", table: "orders", column: "id" }), COLUMN],
    ["a column of another table", grant({ schema: "sales", table: "customers", column: "email" }), TABLE],
    ["another schema, at schema level", grant({ schema: "hr" }), SCHEMA],
  ])("an unrelated grant on %s reaches nothing", (_what, g, p) => {
    expect(grantCoverage(g, p, CAT)).toBeNull();
  });

  it("a grant in another catalog reaches nothing, at any level and any depth", () => {
    for (const scope of [{}, { schema: "sales" }, { schema: "sales", table: "orders" }]) {
      for (const p of [CATALOG, SCHEMA, TABLE]) {
        expect(grantCoverage(grant(scope, "c-other"), p, CAT)).toBeNull();
      }
    }
  });

  it("treats a null level as unset", () => {
    const g = grant({ schema: "sales", table: null, column: null });
    expect(grantCoverage(g, TABLE, CAT)).toEqual({ inheritedFrom: "schema" });
    expect(grantCoverage(g, SCHEMA, CAT)).toBe("here");
  });

  // egret-perm's evaluator (`crates/egret-perm/src/eval.rs`, `level_covers`)
  // and the engines glob-match a named level; plain equality would miss these.
  it("glob-matches a named level, as the evaluator does", () => {
    expect(grantCoverage(grant({ schema: "sales", table: "*" }), TABLE, CAT)).toBe("here");
    expect(grantCoverage(grant({ schema: "sal*" }), TABLE, CAT)).toEqual({ inheritedFrom: "schema" });
    expect(grantCoverage(grant({ schema: "sales", table: "order?" }), TABLE, CAT)).toBe("here");
    expect(grantCoverage(grant({ schema: "sales", table: "cust*" }), TABLE, CAT)).toBeNull();
    expect(grantCoverage(grant({ schema: "sales", table: "orders", column: "*" }), TABLE, CAT)).toBe("child");
  });
});

describe("globMatch", () => {
  it.each([
    ["orders", "orders", true],
    ["orders", "order", false],
    ["*", "", true],
    ["*", "anything", true],
    ["**", "x", true],
    ["ord*", "orders", true],
    ["*ers", "orders", true],
    ["o*d*s", "orders", true],
    ["o*x", "orders", false],
    ["?rders", "orders", true],
    ["?", "", false],
    // One `?` is one code point, as Rust's `chars()` counts.
    ["?", "😀", true],
  ])("%j against %j is %s", (pattern, value, expected) => {
    expect(globMatch(pattern, value)).toBe(expected);
  });
});
