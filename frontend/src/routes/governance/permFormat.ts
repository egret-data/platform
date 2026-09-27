import type { Action, Grant, PermPrincipal, Rule, Scope, Selector } from "@/api/generated/model";

/**
 * Shared formatting for the two steward-facing perm pages (`DataPermissions`
 * and `MaskFilterRules`) — D5's "the sentence is the primary column" applies
 * to both, and a scope/principal set is rendered identically wherever it
 * appears in either one.
 *
 * Nothing here duplicates the five pieces of hardcoded knowledge that live in
 * `PrincipalPicker`/`ScopePicker` (`grep -rn "Hardcoded fact" frontend/src/components/perm`):
 * this module only turns an already-valid `Grant`/`Rule` into readable text,
 * never decides what a picker offers or refuses.
 */

const ACTION_ORDER: Action[] = ["read", "write"];

/** `["read","write"]` → `"read and write"`; a single action → itself. */
export function formatActions(actions: Action[]): string {
  const present = ACTION_ORDER.filter((a) => actions.includes(a));
  if (present.length === 0) return "no actions";
  return present.join(" and ");
}

/**
 * A `Scope` (or a `Selector`'s `path` variant, which is a `Scope` plus a
 * `type` tag) as a dotted path. The `*` marks the DEEPEST unset level, not
 * every unset level — `{catalog_id, schema: "sales"}` reads `wh.sales.*`
 * (any table in that schema), matching the design spec's own example.
 * `schema`/`table`/`column` narrow progressively (a column implies a table
 * implies a schema, enforced by the database and by `ScopePicker`), so at
 * most one of these branches is ever a real narrowing choice for a given row.
 */
export function formatScope(scope: Scope, catalogName: (id: string) => string): string {
  const catalog = catalogName(scope.catalog_id) || scope.catalog_id;
  if (!scope.schema) return `${catalog}.*`;
  if (!scope.table) return `${catalog}.${scope.schema}.*`;
  if (!scope.column) return `${catalog}.${scope.schema}.${scope.table}`;
  return `${catalog}.${scope.schema}.${scope.table}.${scope.column}`;
}

function principalWord(p: PermPrincipal): string {
  return `${p.kind} ${p.ref}`;
}

/** A `PermPrincipal[]` — a `PrincipalSet`'s `include` or `exclude` — joined
 * for reading inline. Empty reads as "nobody" rather than a blank. */
export function formatPrincipals(list: PermPrincipal[]): string {
  if (list.length === 0) return "nobody";
  return list.map(principalWord).join(", ");
}

/**
 * D5: one `Grant` row, one sentence — `effect`, `actions`, the resolved
 * `scope` and the principal subtract (`principals.exclude`) all in one
 * readable line. The subtract is appended as an `except …` clause because it
 * changes the sentence's meaning, exactly as the spec requires; omitted
 * entirely when there is nothing excluded, rather than printed as "except
 * nobody".
 */
export function grantSentence(g: Grant, catalogName: (id: string) => string): string {
  const who = formatPrincipals(g.principals.include);
  const verb = g.effect === "allow" ? "can" : "cannot";
  const actions = formatActions(g.actions);
  const scope = formatScope(g.scope, catalogName);
  const exclude =
    g.principals.exclude.length > 0 ? ` except ${formatPrincipals(g.principals.exclude)}` : "";
  return `${who} ${verb} ${actions} ${scope}${exclude}`;
}

/** What a `Rule`'s `selector` picks out: a tag (by name, resolved from the
 * `key` it actually stores) or a path (a `Scope`, formatted the same way a
 * grant's is). */
export function formatSelector(
  selector: Selector,
  catalogName: (id: string) => string,
  tagName: (key: string) => string,
): string {
  if (selector.type === "tag") return `tag ${tagName(selector.key) || selector.key}`;
  return formatScope(selector, catalogName);
}

/** The rule's kind, actions notwithstanding — mirrors `RuleKind`'s own two
 * shapes so a list column can print it without repeating the branch. */
export function formatRuleKind(rule: Rule): string {
  if (rule.kind.type === "mask") {
    return rule.kind.mask_type === "CUSTOM"
      ? `mask · custom (${rule.kind.value_expr ?? ""})`
      : `mask · ${rule.kind.mask_type}`;
  }
  return `row filter · ${rule.kind.filter_expr}`;
}
