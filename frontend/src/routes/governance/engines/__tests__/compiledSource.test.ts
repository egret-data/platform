import { describe, expect, it } from "vitest";
import { compiledSource } from "@/routes/governance/engines/compiledSource";

/*
 * The frontend twin of `attribute_policy_name`
 * (`crates/egret-perm-compile/src/attribution.rs`); every case of its tests
 * (`crates/egret-perm-compile/tests/attribution.rs` and the module's own) is
 * ported here, plus the shapes Task 1 M1 saw live.
 */

const ID = "3f1c9a2e-5b7d-4c11-9e0a-2b6d8f4a1c35";
const TAG = "a0b1c2d3-e4f5-4a6b-8c7d-9e0f1a2b3c4d";

describe("compiledSource — every shape the compiler mints", () => {
  it("a grant is a bare uuid", () => {
    expect(compiledSource(`perm-grant-${ID}`)).toEqual({ kind: "grant", id: ID });
  });

  it("a rule is a bare uuid (path- or tag-addressed)", () => {
    expect(compiledSource(`perm-rule-${ID}`)).toEqual({ kind: "rule", id: ID });
  });

  it("a per-column rule keeps its row-filter and mask locators verbatim", () => {
    expect(compiledSource(`perm-rule-${ID}-analytics.public.orders`)).toEqual({
      kind: "rule",
      id: ID,
      locator: "analytics.public.orders",
    });
    expect(compiledSource(`perm-rule-${ID}-analytics.public.orders.ssn`)).toEqual({
      kind: "rule",
      id: ID,
      locator: "analytics.public.orders.ssn",
    });
  });

  it("a locator survives a hyphenated catalog name", () => {
    expect(compiledSource(`perm-rule-${ID}-sp3c2-trino-live.public.orders.ssn`)).toEqual({
      kind: "rule",
      id: ID,
      locator: "sp3c2-trino-live.public.orders.ssn",
    });
  });

  it("a clearance names its tag, bare or with a per-column locator", () => {
    expect(compiledSource(`perm-clearance-${TAG}`)).toEqual({ kind: "clearance", id: TAG });
    expect(compiledSource(`perm-clearance-${TAG}-analytics.public.customers.email`)).toEqual({
      kind: "clearance",
      id: TAG,
      locator: "analytics.public.customers.email",
    });
    // Task 1 M1: a StarRocks clearance mask on one column.
    expect(compiledSource(`perm-clearance-${TAG}-mask-ice.s.t.email`)).toEqual({
      kind: "clearance",
      id: TAG,
      locator: "mask-ice.s.t.email",
    });
  });

  it("the engine-wide materialized-view deny is its own kind, and only that exact name", () => {
    expect(compiledSource("perm-clearance-mv")).toEqual({ kind: "mv_deny" });
    expect(compiledSource("perm-clearance-mv-ice.db1")).toBeNull();
  });

  it("an engine grant is a bare uuid and never carries a locator", () => {
    expect(compiledSource(`perm-engine-${ID}`)).toEqual({ kind: "engine_grant", id: ID });
    expect(compiledSource(`perm-engine-${ID}-x`)).toBeNull();
    expect(compiledSource("perm-engine-not-a-uuid")).toBeNull();
  });

  it("an upper-case uuid parses, as Rust's Uuid::parse_str accepts it", () => {
    const upper = ID.toUpperCase();
    expect(compiledSource(`perm-grant-${upper}`)).toEqual({ kind: "grant", id: upper });
  });
});

describe("compiledSource — names with no model object", () => {
  it.each([
    "egret-admin - catalog, schema, table, column",
    "all - catalog, schema, table, column",
    "egret-delegator-impersonate",
    "egret-query-timeout",
    "egret-user-impersonate-self",
    "egret-user-execute-query",
    "nightly-audit-row-filter",
    "superuser",
    "",
  ])("%j is not attributed", (name) => {
    expect(compiledSource(name)).toBeNull();
  });
});

describe("compiledSource — a wrong or garbled name never yields a plausible source", () => {
  it("a truncated uuid after the grant or rule prefix", () => {
    expect(compiledSource(`perm-grant-${ID.slice(0, 30)}`)).toBeNull();
    expect(compiledSource(`perm-rule-${ID.slice(0, 30)}`)).toBeNull();
  });

  it("a same-length string that is not a uuid", () => {
    expect(compiledSource("perm-grant-zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz")).toBeNull();
  });

  it("a grant with a trailing locator", () => {
    expect(compiledSource(`perm-grant-${ID}-extra`)).toBeNull();
  });

  it("a rule whose locator has no separator, or is empty", () => {
    expect(compiledSource(`perm-rule-${ID}extra`)).toBeNull();
    expect(compiledSource(`perm-rule-${ID}-`)).toBeNull();
  });

  it("the prefix in another case, or an unknown prefix", () => {
    expect(compiledSource(`Perm-Grant-${ID}`)).toBeNull();
    expect(compiledSource(`perm-thing-${ID}`)).toBeNull();
  });
});
