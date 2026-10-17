import type { Action, GrantView, PermPrincipal, Rule, RuleView, Scope } from "@/api/generated/model";

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
 *
 * A level whose value is a pattern (`*`, `ord*`) is quoted (`wh.sales.“*”`):
 * the bare trailing `*` means only "this level is not set".
 *
 * `catalogName` is the ALREADY-RESOLVED display name, not a lookup function
 * (SP4b-2 finding 1). The read surface puts it on the payload
 * (`GrantView.catalog_name`, `RuleView.catalog_name`) precisely so rendering
 * a grant needs no second request: `GET /control-plane/catalogs` gates on
 * `catalog_registry:read`, these pages gate on `policy:read`, and a client
 * holding only the latter used to render its primary column as a raw uuid.
 * Absent (a catalog deleted under the read), the id is shown — the honest
 * fallback, never a guess.
 */
export function formatScope(scope: Scope, catalogName?: string | null): string {
  const catalog = catalogName || scope.catalog_id;
  const schema = scope.schema ? level(scope.schema) : null;
  const table = scope.table ? level(scope.table) : null;
  if (!schema) return `${catalog}.*`;
  if (!table) return `${catalog}.${schema}.*`;
  if (!scope.column) return `${catalog}.${schema}.${table}`;
  return `${catalog}.${schema}.${table}.${level(scope.column)}`;
}

/**
 * Whether a stored scope value is a pattern: the engines glob-match a level
 * (`*` any run of characters, `?` one; `crates/egret-perm/src/eval.rs`,
 * `glob_match`), so `*` or `ord*` names every value it matches, not one.
 */
export function isPattern(value: string): boolean {
  return /[*?]/.test(value);
}

/**
 * A named level as the sentence prints it. A pattern is quoted, so a grant
 * whose table is the value `*` (`wh.sales.“*”`) never reads the same as one
 * that sets no table at all (`wh.sales.*`).
 */
function level(value: string): string {
  return isPattern(value) ? `“${value}”` : value;
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
export function grantSentence(g: GrantView): string {
  const who = formatPrincipals(g.principals.include);
  const verb = g.effect === "allow" ? "can" : "cannot";
  const actions = formatActions(g.actions);
  const scope = formatScope(g.scope, g.catalog_name);
  const exclude =
    g.principals.exclude.length > 0 ? ` except ${formatPrincipals(g.principals.exclude)}` : "";
  return `${who} ${verb} ${actions} ${scope}${exclude}`;
}

/**
 * What a `Rule`'s `selector` picks out: a tag (by display name) or a path (a
 * `Scope`, formatted the same way a grant's is).
 *
 * Takes the whole `RuleView` rather than its `selector` because both display
 * names — `tag_name` for a tag selector, `catalog_name` for a path one — ride
 * on the payload beside the selector, for the reason `formatScope` documents.
 * `tags.key` (`pii`) is what the rule stores; `tags.name` (`PII`) is what a
 * steward reads, and `GET /tags` needs `tag:read`, which `policy:read` does
 * not imply.
 */
export function formatSelector(r: RuleView): string {
  if (r.selector.type === "tag") return `tag ${r.tag_name || r.selector.key}`;
  return formatScope(r.selector, r.catalog_name);
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
