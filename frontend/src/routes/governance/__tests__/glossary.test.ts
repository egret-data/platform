import { describe, expect, it } from "vitest";
import { MASK_TYPE_LABEL, MASK_TYPES, TAG_TAB_LABEL, TERM } from "@/routes/governance/glossary";
import { formatRuleKind, formatSelector } from "@/routes/governance/permFormat";
import type { Rule, RuleView } from "@/api/generated/model";

const rule = (kind: Rule["kind"]): Rule => ({
  id: "r",
  name: "r",
  kind,
  selector: { type: "tag", key: "pii" },
  exempt: { include: [], exclude: [] },
});

describe("the glossary's mask types", () => {
  it("labels every type the dialog offers, and formatRuleKind uses the label", () => {
    expect(MASK_TYPES.map((m) => m.value)).toEqual([
      "MASK", "MASK_SHOW_LAST_4", "MASK_SHOW_FIRST_4", "MASK_HASH", "MASK_NULL", "MASK_NONE", "MASK_DATE_SHOW_YEAR", "CUSTOM",
    ]);
    expect(formatRuleKind(rule({ type: "mask", mask_type: "MASK_HASH" }))).toBe("Mask · Hash");
    expect(formatRuleKind(rule({ type: "mask", mask_type: "CUSTOM", value_expr: "upper(x)" }))).toBe("Mask · Custom (upper(x))");
    expect(formatRuleKind(rule({ type: "row_filter", filter_expr: "region = 'us'" }))).toBe("Row filter · region = 'us'");
    // An unknown type the API may still hold is shown as stored, never dropped.
    expect(formatRuleKind(rule({ type: "mask", mask_type: "MASK_FUTURE" }))).toBe("Mask · MASK_FUTURE");
    expect(MASK_TYPES.map((m) => m.label)).toEqual([
      "Redact", "Partial (last 4)", "Partial (first 4)", "Hash", "Nullify", "Unmasked", "Year only", "Custom",
    ]);
    expect(MASK_TYPE_LABEL.MASK).toBe("Redact");
    expect(formatRuleKind(rule({ type: "mask", mask_type: "MASK_DATE_SHOW_YEAR" }))).toBe("Mask · Year only");
  });
});

describe("the glossary's terms", () => {
  it("names the classification page's tabs, the Hand-written one as Manual policies", () => {
    expect(Object.values(TAG_TAB_LABEL)).toEqual(["Where used", "Clearance", "Rules", "Views", "Manual policies"]);
    expect(TERM.manualPolicies).toBe("Manual policies");
  });

  it("a rule applies to a classification by its name, never to a 'tag'", () => {
    const r: RuleView = { ...rule({ type: "mask", mask_type: "MASK" }), tag_name: "PII Data" };
    expect(formatSelector(r)).toBe("Classification PII Data");
    expect(formatSelector({ ...r, tag_name: null })).toBe("Classification pii");
  });
});
