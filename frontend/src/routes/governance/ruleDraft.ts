import type { NewRule, Rule } from "@/api/generated/model";

/**
 * Where the rule dialog was opened from (spec 2026-10-21-classification-ux
 * C8.3). Each entry point seeds its own defaults ({@link seedRule}) and keeps
 * its own draft ({@link ruleDraftKey}).
 *
 * - `classification`: a classification's page; a Redact mask on it, named after it.
 * - `table`: a table's Rules tab in Data; a mask on the first classification
 *   its columns carry (`classifications`), else on the table's path, Column left empty.
 * - `scope`: a catalog's or schema's Rules tab in Data; a mask on that path.
 *   (Not in the plan's list: the tab is on those pages too, and a table entry
 *   would have to invent a table.)
 * - `column`: the column panel. `classifications` are the ones the column
 *   carries (its own, then its table's, marked `wholeTable`). The default is a
 *   mask on the first of its OWN (Task 13: the steward's job is one fewer
 *   detour), named after it; with none, a mask on the column's path. A
 *   whole-table classification is never the default: a mask on it makes the
 *   table unreadable and is refused. The others, and the column's path, are
 *   each one click away ("Or apply it to …").
 * - `edit`: an existing rule; its own fields.
 */
export type RuleEntry =
  | { kind: "classification"; tagKey: string; tagName: string }
  | { kind: "table"; catalogId: string; schema: string; table: string; classifications?: EntryClassification[] }
  | { kind: "scope"; catalogId: string; schema?: string }
  | {
      kind: "column";
      catalogId: string;
      schema: string;
      table: string;
      column: string;
      classifications: EntryClassification[];
    }
  | { kind: "edit"; rule: Rule };

/** A classification an entry point offers: `wholeTable` when it is on the table, not the column. */
export type EntryClassification = { key: string; name: string; wholeTable?: boolean };

/** The classification an entry point preselects: the first not on a whole table. */
export function defaultClassification(entry: RuleEntry): EntryClassification | undefined {
  if (entry.kind !== "column" && entry.kind !== "table") return undefined;
  return (entry.classifications ?? []).find((c) => !c.wholeTable);
}

/** A mask on `c`, named after it: what the classification page's entry seeds too. */
const onClassification = (c: EntryClassification): NewRule => ({
  name: `Mask ${c.name}`,
  kind: REDACT(),
  selector: { type: "tag", key: c.key },
  exempt: NOBODY_EXEMPT(),
});

/**
 * The path an entry point is on (a table's, a column's), kept for "Applies to
 * = Path" when the rule starts on a classification; `null` for the others.
 */
export function entryScope(entry: RuleEntry): { catalog_id: string; schema?: string; table?: string; column?: string } | null {
  if (entry.kind === "table") return { catalog_id: entry.catalogId, schema: entry.schema, table: entry.table };
  if (entry.kind === "column") return { catalog_id: entry.catalogId, schema: entry.schema, table: entry.table, column: entry.column };
  return null;
}

const NOBODY_EXEMPT = (): NewRule["exempt"] => ({ include: [], exclude: [] });
const REDACT = (): NewRule["kind"] => ({ type: "mask", mask_type: "MASK" });

/** The rule a new dialog starts from, by entry point (spec C8.3). */
export function seedRule(entry: RuleEntry): NewRule {
  const preselected = defaultClassification(entry);
  if (preselected) return onClassification(preselected);
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
 * a reload. Not `localStorage`, which FE-DATA-3 keeps for preferences. Every
 * identity switch empties it (`clearUserScopedCache`).
 *
 * Each draft keeps its `base`: the snapshot of the rule it began from. A rule
 * carries no version, so this is how the dialog tells that an `edit` draft
 * was written against a rule that has changed since, and must not be restored
 * over it.
 */
export type StoredRuleDraft = { draft: NewRule; base: string };
const drafts = new Map<string, StoredRuleDraft>();

/**
 * One draft per signed-in user and entry point. The parts are JSON-encoded, so
 * a separator inside a name (a schema `a/b`) cannot make two entries collide.
 */
export function ruleDraftKey(userId: string, entry: RuleEntry): string {
  switch (entry.kind) {
    case "classification":
      return JSON.stringify([userId, entry.kind, entry.tagKey]);
    case "table":
      return JSON.stringify([userId, entry.kind, entry.catalogId, entry.schema, entry.table]);
    case "scope":
      return JSON.stringify([userId, entry.kind, entry.catalogId, entry.schema ?? null]);
    case "column":
      return JSON.stringify([userId, entry.kind, entry.catalogId, entry.schema, entry.table, entry.column]);
    case "edit":
      return JSON.stringify([userId, entry.kind, entry.rule.id]);
  }
}

export function loadRuleDraft(key: string): NewRule | undefined {
  return drafts.get(key)?.draft;
}

/** The draft with the snapshot of the rule it began from. */
export function loadStoredRuleDraft(key: string): StoredRuleDraft | undefined {
  return drafts.get(key);
}

export function saveRuleDraft(key: string, draft: NewRule, base = ""): void {
  drafts.set(key, { draft, base });
}

export function clearRuleDraft(key: string): void {
  drafts.delete(key);
}

/** Forgets every draft: on an identity switch, and between tests. */
export function clearAllRuleDrafts(): void {
  drafts.clear();
}
