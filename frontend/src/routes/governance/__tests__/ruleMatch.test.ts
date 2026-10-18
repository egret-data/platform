import { describe, expect, it } from "vitest";
import type { RuleView } from "@/api/generated/model";
import { globMatch, rulesOnTag, tagUnresolved } from "@/routes/governance/ruleMatch";

/**
 * How a tag rule finds its tag (DG redesign #2, P9). `globMatch` and
 * `tagUnresolved` moved here unchanged from `data/coverage.ts`, so the
 * Classifications pages, which are not in the lazy Data area, use the same
 * matcher as the Data area's Masks & filters tab.
 */
function tagRule(id: string, key: string, tagName: string | null): RuleView {
  return {
    id,
    name: `rule ${id}`,
    kind: { type: "mask", mask_type: "MASK_NULL" },
    selector: { type: "tag", key },
    exempt: { include: [], exclude: [] },
    tag_name: tagName,
  };
}

const pathRule: RuleView = {
  id: "p",
  name: "path rule",
  kind: { type: "row_filter", filter_expr: "true" },
  selector: { type: "path", catalog_id: "c", schema: "s" },
  exempt: { include: [], exclude: [] },
  tag_name: null,
  catalog_name: "ice",
};

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

describe("tagUnresolved", () => {
  it("is true only for a tag rule whose tag name did not resolve", () => {
    expect(tagUnresolved(tagRule("a", "pii", null))).toBe(true);
    expect(tagUnresolved(tagRule("b", "pii", "PII"))).toBe(false);
    expect(tagUnresolved(pathRule)).toBe(false);
  });
});

describe("rulesOnTag", () => {
  const tag = { key: "pii_eu", name: "PII" };

  it("lists the rules whose tag NAME matches, its own key or another's, case-sensitively and by glob", () => {
    const rules = [
      tagRule("own", "pii_eu", "PII"),
      tagRule("same-name", "pii_us", "PII"),
      tagRule("glob", "p", "P?I"),
      tagRule("lower", "pii_low", "pii"),
      tagRule("other", "fin", "Finance"),
      tagRule("gone", "pii_eu", null),
      pathRule,
    ];
    expect(rulesOnTag(rules, tag).map((r) => r.id)).toEqual(["own", "same-name", "glob"]);
  });
});
