import { describe, expect, it } from "vitest";
import {
  clearAllRuleDrafts,
  clearRuleDraft,
  loadRuleDraft,
  loadStoredRuleDraft,
  ruleDraftKey,
  saveRuleDraft,
  seedRule,
} from "@/routes/governance/ruleDraft";
import type { Rule } from "@/api/generated/model";

describe("seedRule: defaults per entry point (spec C8)", () => {
  it("from a classification: a Redact mask on that classification, named after it", () => {
    expect(seedRule({ kind: "classification", tagKey: "pii", tagName: "PII" })).toEqual({
      name: "Mask PII",
      kind: { type: "mask", mask_type: "MASK" },
      selector: { type: "tag", key: "pii" },
      exempt: { include: [], exclude: [] },
    });
  });
  it("from a table: a mask on the table's path, column left empty", () => {
    expect(seedRule({ kind: "table", catalogId: "c", schema: "s", table: "t" })).toEqual({
      name: "",
      kind: { type: "mask", mask_type: "MASK" },
      selector: { type: "path", catalog_id: "c", schema: "s", table: "t" },
      exempt: { include: [], exclude: [] },
    });
  });
  it("from a column: a mask on that column", () => {
    const r = seedRule({ kind: "column", catalogId: "c", schema: "s", table: "t", column: "email", classifications: [] });
    expect(r.selector).toEqual({ type: "path", catalog_id: "c", schema: "s", table: "t", column: "email" });
    expect(r.kind).toEqual({ type: "mask", mask_type: "MASK" });
  });
  it("from a column with a classification of its own: a mask on that classification, named after it", () => {
    const r = seedRule({
      kind: "column",
      catalogId: "c",
      schema: "s",
      table: "t",
      column: "email",
      classifications: [
        { key: "conf", name: "Confidential", wholeTable: true },
        { key: "pii", name: "PII" },
      ],
    });
    expect(r).toEqual({ name: "Mask PII", kind: { type: "mask", mask_type: "MASK" }, selector: { type: "tag", key: "pii" }, exempt: { include: [], exclude: [] } });
  });
  it("from a column whose only classification is its table's: the column's path (a mask on a whole-table classification is refused)", () => {
    const r = seedRule({ kind: "column", catalogId: "c", schema: "s", table: "t", column: "email", classifications: [{ key: "conf", name: "Confidential", wholeTable: true }] });
    expect(r.selector).toEqual({ type: "path", catalog_id: "c", schema: "s", table: "t", column: "email" });
  });
  it("from a table whose columns carry classifications: a mask on the first of them", () => {
    const r = seedRule({ kind: "table", catalogId: "c", schema: "s", table: "t", classifications: [{ key: "pii", name: "PII" }] });
    expect(r.selector).toEqual({ type: "tag", key: "pii" });
    expect(r.name).toBe("Mask PII");
  });
  it("from a catalog or schema page: a mask on that scope, no level invented", () => {
    expect(seedRule({ kind: "scope", catalogId: "c", schema: "s" }).selector).toEqual({ type: "path", catalog_id: "c", schema: "s" });
    expect(seedRule({ kind: "scope", catalogId: "c" }).selector).toEqual({ type: "path", catalog_id: "c" });
  });
  it("from an existing rule: the rule's own fields, not a default", () => {
    const rule: Rule = {
      id: "r1",
      name: "filter-us",
      kind: { type: "row_filter", filter_expr: "region = 'us'" },
      selector: { type: "path", catalog_id: "c", schema: "s", table: "t" },
      exempt: { include: [{ kind: "user", ref: "alice" }], exclude: [] },
    };
    expect(seedRule({ kind: "edit", rule })).toEqual({
      name: "filter-us",
      kind: rule.kind,
      selector: rule.selector,
      exempt: rule.exempt,
    });
  });
});

describe("the draft store", () => {
  it("keys drafts by user and entry, and clears one without touching another", () => {
    const a = ruleDraftKey("u1", { kind: "classification", tagKey: "pii", tagName: "PII" });
    const b = ruleDraftKey("u2", { kind: "classification", tagKey: "pii", tagName: "PII" });
    expect(a).not.toBe(b);
    const draft = seedRule({ kind: "classification", tagKey: "pii", tagName: "PII" });
    saveRuleDraft(a, { ...draft, name: "kept" });
    expect(loadRuleDraft(a)?.name).toBe("kept");
    expect(loadRuleDraft(b)).toBeUndefined();
    clearRuleDraft(a);
    expect(loadRuleDraft(a)).toBeUndefined();
  });

  it("tells entry points apart: a table, one of its columns and a rule each keep their own draft", () => {
    const keys = [
      ruleDraftKey("u1", { kind: "table", catalogId: "c", schema: "s", table: "t" }),
      ruleDraftKey("u1", { kind: "column", catalogId: "c", schema: "s", table: "t", column: "email", classifications: [] }),
      ruleDraftKey("u1", { kind: "scope", catalogId: "c", schema: "s" }),
      ruleDraftKey("u1", {
        kind: "edit",
        rule: { id: "r1", name: "x", kind: { type: "mask", mask_type: "MASK" }, selector: { type: "tag", key: "pii" }, exempt: { include: [], exclude: [] } },
      }),
    ];
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("cannot be confused by a separator inside a name", () => {
    const a = ruleDraftKey("u1", { kind: "table", catalogId: "c", schema: "a/b", table: "t" });
    const b = ruleDraftKey("u1", { kind: "table", catalogId: "c", schema: "a", table: "b/t" });
    const c = ruleDraftKey("u1|table:c", { kind: "scope", catalogId: "x" });
    const d = ruleDraftKey("u1", { kind: "scope", catalogId: "|table:c" });
    expect(a).not.toBe(b);
    expect(c).not.toBe(d);
  });

  it("keeps the version a draft began from beside it", () => {
    const key = ruleDraftKey("u1", { kind: "classification", tagKey: "pii", tagName: "PII" });
    const draft = seedRule({ kind: "classification", tagKey: "pii", tagName: "PII" });
    saveRuleDraft(key, draft, "base-1");
    expect(loadStoredRuleDraft(key)).toEqual({ draft, base: "base-1" });
    clearAllRuleDrafts();
    expect(loadStoredRuleDraft(key)).toBeUndefined();
  });
});
