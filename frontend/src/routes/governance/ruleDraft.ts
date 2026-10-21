import type { NewRule, Rule } from "@/api/generated/model";

/**
 * Where the rule dialog was opened from (spec 2026-10-21-classification-ux
 * C8.3). Each entry point seeds its own defaults ({@link seedRule}) and keeps
 * its own draft ({@link ruleDraftKey}).
 *
 * - `classification`: a classification's page; a Redact mask on it, named after it.
 * - `table`: a table's Rules tab in Data; a mask on the table's path, Column left empty.
 * - `scope`: a catalog's or schema's Rules tab in Data; a mask on that path.
 *   (Not in the plan's list: the tab is on those pages too, and a table entry
 *   would have to invent a table.)
 * - `column`: the column panel; a mask on the column. `classifications` are the
 *   ones the column carries (its own and its table's), each offered as
 *   "Or apply it to <name>".
 * - `edit`: an existing rule; its own fields.
 */
export type RuleEntry =
  | { kind: "classification"; tagKey: string; tagName: string }
  | { kind: "table"; catalogId: string; schema: string; table: string }
  | { kind: "scope"; catalogId: string; schema?: string }
  | {
      kind: "column";
      catalogId: string;
      schema: string;
      table: string;
      column: string;
      classifications: { key: string; name: string }[];
    }
  | { kind: "edit"; rule: Rule };

const NOBODY_EXEMPT = (): NewRule["exempt"] => ({ include: [], exclude: [] });
const REDACT = (): NewRule["kind"] => ({ type: "mask", mask_type: "MASK" });

/** The rule a new dialog starts from, by entry point (spec C8.3). */
export function seedRule(entry: RuleEntry): NewRule {
  switch (entry.kind) {
    case "classification":
      return {
        name: `Mask ${entry.tagName}`,
        kind: REDACT(),
        selector: { type: "tag", key: entry.tagKey },
        exempt: NOBODY_EXEMPT(),
      };
    case "table":
      return {
        name: "",
        kind: REDACT(),
        selector: { type: "path", catalog_id: entry.catalogId, schema: entry.schema, table: entry.table },
        exempt: NOBODY_EXEMPT(),
      };
    case "scope":
      return {
        name: "",
        kind: REDACT(),
        selector:
          entry.schema === undefined
            ? { type: "path", catalog_id: entry.catalogId }
            : { type: "path", catalog_id: entry.catalogId, schema: entry.schema },
        exempt: NOBODY_EXEMPT(),
      };
    case "column":
      return {
        name: "",
        kind: REDACT(),
        selector: {
          type: "path",
          catalog_id: entry.catalogId,
          schema: entry.schema,
          table: entry.table,
          column: entry.column,
        },
        exempt: NOBODY_EXEMPT(),
      };
    case "edit": {
      const { name, kind, selector, exempt } = entry.rule;
      return { name, kind, selector, exempt };
    }
  }
}

/**
 * The draft store (spec C8.4): in memory only, so a draft survives a detour
 * within the app (the dialog unmounted, reopened from the same place) but not
 * a reload. Not `localStorage`, which FE-DATA-3 keeps for preferences.
 */
const drafts = new Map<string, NewRule>();

/** One draft per signed-in user and entry point. */
export function ruleDraftKey(userId: string, entry: RuleEntry): string {
  switch (entry.kind) {
    case "classification":
      return `${userId}|classification:${entry.tagKey}`;
    case "table":
      return `${userId}|table:${entry.catalogId}/${entry.schema}/${entry.table}`;
    case "scope":
      return `${userId}|scope:${entry.catalogId}/${entry.schema ?? ""}`;
    case "column":
      return `${userId}|column:${entry.catalogId}/${entry.schema}/${entry.table}/${entry.column}`;
    case "edit":
      return `${userId}|edit:${entry.rule.id}`;
  }
}

export function loadRuleDraft(key: string): NewRule | undefined {
  return drafts.get(key);
}

export function saveRuleDraft(key: string, draft: NewRule): void {
  drafts.set(key, draft);
}

export function clearRuleDraft(key: string): void {
  drafts.delete(key);
}

/** Forgets every draft. The store outlives a test's render, so tests reset it. */
export function clearAllRuleDrafts(): void {
  drafts.clear();
}
