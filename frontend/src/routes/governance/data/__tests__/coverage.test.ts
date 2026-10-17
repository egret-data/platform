import { describe, expect, it } from "vitest";
import type { Grant, ResourceTagDto, Rule, Scope } from "@/api/generated/model";
import { globMatch, grantCoverage, patternLevels, ruleSelects, tagKeysOn } from "@/routes/governance/data/coverage";
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

describe("patternLevels", () => {
  it("lists the levels set to a pattern, and nothing for named or unset levels", () => {
    expect(patternLevels({ catalog_id: CAT })).toEqual([]);
    expect(patternLevels({ catalog_id: CAT, schema: "sales", table: "orders", column: "email" })).toEqual([]);
    expect(patternLevels({ catalog_id: CAT, schema: "sales", table: "*" })).toEqual([{ level: "table", value: "*" }]);
    expect(patternLevels({ catalog_id: CAT, schema: "sal*", table: "order?" })).toEqual([
      { level: "schema", value: "sal*" },
      { level: "table", value: "order?" },
    ]);
  });
});

function rule(selector: Rule["selector"], kind: Rule["kind"] = { type: "mask", mask_type: "MASK_HASH" }): Rule {
  return { id: "r", name: "r", kind, selector, exempt: { include: [], exclude: [] } };
}
const pathRule = (scope: Omit<Scope, "catalog_id">, catalogId = CAT) => rule({ type: "path", catalog_id: catalogId, ...scope });
const tagRule = (key: string, kind?: Rule["kind"]) => rule({ type: "tag", key }, kind);

describe("ruleSelects", () => {
  // A path selector selects p when it is set at p's level or above (egret-perm `selector_hits` → `covers`).
  it.each([
    ["catalog", {}, CATALOG, "path"],
    ["catalog", {}, SCHEMA, "path"],
    ["catalog", {}, TABLE, "path"],
    ["catalog", {}, COLUMN, "path"],
    ["schema", { schema: "sales" }, CATALOG, null],
    ["schema", { schema: "sales" }, SCHEMA, "path"],
    ["schema", { schema: "sales" }, COLUMN, "path"],
    ["table", { schema: "sales", table: "orders" }, SCHEMA, null],
    ["table", { schema: "sales", table: "orders" }, TABLE, "path"],
    ["table", { schema: "sales", table: "orders" }, COLUMN, "path"],
    ["column", { schema: "sales", table: "orders", column: "email" }, TABLE, null],
    ["column", { schema: "sales", table: "orders", column: "email" }, COLUMN, "path"],
    ["another column", { schema: "sales", table: "orders", column: "id" }, COLUMN, null],
    ["a pattern table", { schema: "sales", table: "ord*" }, COLUMN, "path"],
    ["a pattern that misses", { schema: "sales", table: "cust*" }, TABLE, null],
  ] as const)("a %s path rule at %j", (_what, scope, p, expected) => {
    expect(ruleSelects(pathRule(scope), p, CAT, [])).toBe(expected);
  });

  it("a path rule in another catalog selects nothing", () => {
    for (const p of [CATALOG, SCHEMA, TABLE, COLUMN]) {
      expect(ruleSelects(pathRule({}, "c-other"), p, CAT, [])).toBeNull();
      expect(ruleSelects(pathRule({ schema: "sales", table: "orders" }, "c-other"), p, CAT, [])).toBeNull();
    }
  });

  it("a tag rule selects a table or a column whose resolved tags carry its key", () => {
    expect(ruleSelects(tagRule("pii"), COLUMN, CAT, ["pii"])).toBe("tag");
    expect(ruleSelects(tagRule("pii"), TABLE, CAT, ["confidential", "pii"])).toBe("tag");
    expect(ruleSelects(tagRule("pii"), COLUMN, CAT, ["confidential"])).toBeNull();
  });

  it("a tag rule never selects a catalog or a schema, which cannot carry a tag", () => {
    expect(ruleSelects(tagRule("pii"), CATALOG, CAT, ["pii"])).toBeNull();
    expect(ruleSelects(tagRule("pii"), SCHEMA, CAT, ["pii"])).toBeNull();
  });

  it("a tag rule matches the tag's key, never its name", () => {
    expect(ruleSelects(tagRule("pii"), COLUMN, CAT, ["PII"])).toBeNull();
  });
});

describe("tagKeysOn", () => {
  const a = (column: string, key: string, table = "orders", schema = "sales"): ResourceTagDto => ({
    schema,
    table,
    column,
    tag: { id: `t-${key}`, key, name: key.toUpperCase(), description: "", color: "#000", assignment_count: 0, created_at: "" },
  });
  const ASSIGNMENTS = [a("email", "pii"), a("", "confidential"), a("id", "key"), a("", "other", "customers")];

  it("gives a column its own tags and its table's (the table's reach it as an ancestor)", () => {
    expect(tagKeysOn(ASSIGNMENTS, COLUMN).sort()).toEqual(["confidential", "pii"]);
  });

  it("gives a table only its own tags: a column's tag does not reach up to its table", () => {
    expect(tagKeysOn(ASSIGNMENTS, TABLE)).toEqual(["confidential"]);
  });

  it("gives a catalog or a schema no tags", () => {
    expect(tagKeysOn(ASSIGNMENTS, CATALOG)).toEqual([]);
    expect(tagKeysOn(ASSIGNMENTS, SCHEMA)).toEqual([]);
  });

  it("matches the column, table and schema without regard to case, as egret resolves them", () => {
    const upper = [a("EMAIL", "pii", "ORDERS", "Sales")];
    expect(tagKeysOn(upper, COLUMN)).toEqual(["pii"]);
  });

  it("puts a tag reached at both levels in once", () => {
    expect(tagKeysOn([a("email", "pii"), a("", "pii")], COLUMN)).toEqual(["pii"]);
  });
});
