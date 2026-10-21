import type { TagTab } from "@/routes/governance/classifications/tagUrl";

/**
 * The Classification area's vocabulary (spec 2026-10-21-classification-ux-design
 * §4): one word per thing. A string in the Classification, Data and rule pages
 * that names one of these concepts uses the constant, so a rename happens here
 * once. UI text is English only (FE-COPY-1); the spec's Vietnamese column is
 * for the docs.
 *
 * Retired names (must not appear in this area's UI): tag/Tags as a label,
 * "Mask & filter", "Masks & filters", "Masking rules", "Mask and filter rules",
 * "Hand-written" (the classification tab), "Pick a tag", "Selects by",
 * "Reaches this tag", "Carries it", "egret rule", "perm.rules".
 */
export const TERM = {
  /** A label attached to tables and columns (the API and permissions still say `tag`). */
  classification: "Classification",
  classifications: "Classifications",
  /** A classification's stable identifier. */
  key: "Key",
  /** Putting a classification on data / taking it off. */
  attach: "Attach",
  remove: "Remove",
  /** A mask or row filter written in egret. */
  rule: "Rule",
  rules: "Rules",
  mask: "Mask",
  rowFilter: "Row filter",
  /** A rule's kind (Mask or Row filter), as a column or field name. */
  kind: "Kind",
  /** How a listed rule reaches a classification: written on it, or on one with the same name. */
  appliesThrough: "Applies through",
  /** Who a rule does not apply to. */
  exempt: "Exempt",
  /** What a rule is written on: a classification or a path. */
  appliesTo: "Applies to",
  path: "Path",
  /** Who may open data carrying a classification. */
  clearance: "Clearance",
  /** Where a classification is attached. */
  whereUsed: "Where used",
  /**
   * Policies written in an engine profile's own editor, not compiled from a
   * rule or a grant. The spec's "Engine policies" was overridden by the
   * controller: the tab reads "Manual policies".
   */
  manualPolicies: "Manual policies",
  /** Views that may expose classified columns. */
  views: "Views",
  /** The explain action. */
  whoCanRead: "Who can read?",
  addClassification: "Add classification",
  addRule: "Add rule",
} as const;

/**
 * Ranger's mask types (`crates/egret-policy/src/service-defs/trino.json`,
 * `maskTypes`) with the spec §4 glossary's labels, in the rule dialog's order.
 */
export const MASK_TYPES: { value: string; label: string }[] = [
  { value: "MASK", label: "Redact" },
  { value: "MASK_SHOW_LAST_4", label: "Partial (last 4)" },
  { value: "MASK_SHOW_FIRST_4", label: "Partial (first 4)" },
  { value: "MASK_HASH", label: "Hash" },
  { value: "MASK_NULL", label: "Nullify" },
  { value: "MASK_NONE", label: "Unmasked" },
  { value: "MASK_DATE_SHOW_YEAR", label: "Year only" },
  { value: "CUSTOM", label: "Custom" },
];

/**
 * A mask type's label by its stored value. A value not listed (one the API
 * may still hold) has no entry: callers show the value as stored, never drop it.
 */
export const MASK_TYPE_LABEL: Record<string, string> = Object.fromEntries(MASK_TYPES.map((m) => [m.value, m.label]));

/** A classification page's tabs by id (spec §5), in the page's order. */
export const TAG_TAB_LABEL: Record<TagTab, string> = {
  "where-used": TERM.whereUsed,
  clearance: TERM.clearance,
  rules: TERM.rules,
  views: TERM.views,
  handwritten: TERM.manualPolicies,
};
