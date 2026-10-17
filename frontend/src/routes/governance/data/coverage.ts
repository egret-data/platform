import type { Grant, ResourceTagDto, RuleView, Scope } from "@/api/generated/model";
import { isPattern } from "@/routes/governance/permFormat";
import { globMatch } from "@/routes/governance/ruleMatch";
import type { ObjectPath } from "@/routes/governance/data/objectPath";

/** How a grant reaches an object: set on it, set on an ancestor, or set on something inside it (a column of a table; any table or column of a schema or catalog). */
export type GrantCoverage = "here" | { inheritedFrom: "catalog" | "schema" | "table" } | "child" | null;

const ANCESTORS = ["catalog", "schema", "table"] as const;

/**
 * How grant `g` reaches `p`, an object of the catalog whose id is `catalogId`.
 *
 * A grant reaches `p` when its catalog is `p`'s and each level it sets
 * matches `p`'s at that level (an unset level is every value). It is "here"
 * when it is set at `p`'s own level, inherited when it is set higher up, and
 * "child" when it is set deeper, on something inside `p`: a column of the
 * table `p` (a table, view or materialized view), or a table or column of the
 * schema or catalog `p`. A "child" grant does not reach `p` itself; a page
 * lists it so that a grant made on something inside `p`, or on a pattern
 * below it, is seen where it was made and where "Open in Data" lands.
 *
 * A level matches as the engines match it: by glob (`*` any run of
 * characters, `?` one), not by equality, and without regard to case. egret's
 * served Ranger service-defs set `ignoreCase: true` on catalog, schema, table
 * and column for both Trino and StarRocks
 * (`crates/egret-policy/src/service-defs/trino.json`, `starrocks.json`), and
 * Ranger folds both the policy value and the requested name
 * (`fold` in `crates/egret-ranger-eval/src/resource.rs`), so a deny grant
 * typed on `sales.Orders` reaches the table `orders`. The Data surface follows
 * the engines, and so does egret-perm's own evaluator: `level_covers` matches
 * through `resource_level_matches` (`crates/egret-perm/src/eval.rs`), the
 * same whole-string lower-case on both sides, then a glob. A grant with
 * `table = "*"` is therefore
 * "here" on every table of its schema; {@link patternLevels} tells such a
 * grant apart from one set on this object by name.
 *
 * The scope's levels narrow without gaps (a column implies a table implies a
 * schema; `perm.grants` CHECKs it), so the set levels are a prefix.
 */
export function grantCoverage(g: Grant, p: ObjectPath, catalogId: string): GrantCoverage {
  return scopeCoverage(g.scope, p, catalogId);
}

/**
 * How a scope reaches `p`: {@link grantCoverage}'s rule, for any scope. A
 * rule's path selector is a `Scope` (`Selector`'s `path` variant), compiled to
 * the same engine resources a grant's scope is, so it matches the same way:
 * by glob, each level lower-cased on both sides, as Ranger does.
 */
export function scopeCoverage(scope: Scope, p: ObjectPath, catalogId: string): GrantCoverage {
  if (scope.catalog_id !== catalogId) return null;
  const granted = prefix([scope.schema, scope.table, scope.column]);
  const target = prefix([p.schema, p.schema === undefined ? undefined : p.object, p.object === undefined ? undefined : p.column]);

  const shared = Math.min(granted.length, target.length);
  for (let i = 0; i < shared; i++) {
    if (!globMatch(granted[i].toLowerCase(), target[i].toLowerCase())) return null;
  }
  if (granted.length === target.length) return "here";
  if (granted.length < target.length) return { inheritedFrom: ANCESTORS[granted.length] };
  // Deeper than `p`: on something inside it.
  return "child";
}

/**
 * Whether rule `r` selects `p` (an object of the catalog `catalogId`, or one
 * of its columns), and by what: its path, or a tag `p` carries. F10: this
 * lists the rules that select something, it never says who sees what; only
 * explain evaluates.
 *
 * A path selector selects `p` when it is set at `p`'s level or above and each
 * level it sets matches (by glob, without regard to case, as the engines match
 * it; see {@link grantCoverage}). A selector set below `p` (a column, for a
 * table) does not select `p`.
 *
 * A tag selector selects `p` when one of `tagNamesOnPath`, the tag NAMES
 * resolved for `p` by {@link tagNamesOn}, matches the rule's resolved
 * `tag_name`. Names, not keys: the rule stores `tags.key`, but the compiler
 * stamps the resolved `tags.name` into the tag policy
 * (`crates/egret-perm/src/actions.rs:300-310`, `MissingTagName`), egret's tag
 * download serves `tags.name` as each tag's type
 * (`crates/egret-policy-api/src/dto.rs:160,185`), and explain resolves names
 * (`crates/egret-policy/src/store.rs:1638-1643`). `tags.name` is not unique
 * (`migrations/0080_tags.sql:8-9`), so a rule on one tag reaches every tag
 * that shares its name, and this says so. The match is Ranger's tag matcher:
 * case-sensitive (`ignoreCase: false`) with `wildCard: true`, so a name with
 * `*` or `?` globs (`crates/egret-policy/src/service-defs/tag.json:21`), done by
 * {@link globMatch}. A rule whose `tag_name` did not resolve (`null`, its tag
 * gone) selects nothing: the compiler refuses to deploy it (`MissingTagName`);
 * the Masks & filters tab names such rules rather than dropping them silently.
 *
 * Only a table or a column can carry a tag, so a tag rule never selects a
 * catalog or a schema (`selector_hits`' `target.table.is_some()` guard). A tag
 * rule is catalog-unscoped: which catalog it reaches is which catalog's tags
 * it finds.
 */
export function ruleSelects(r: RuleView, p: ObjectPath, catalogId: string, tagNamesOnPath: string[]): "path" | "tag" | null {
  if (r.selector.type === "tag") {
    if (p.schema === undefined || p.object === undefined) return null;
    const name = r.tag_name;
    if (!name) return null;
    return tagNamesOnPath.some((n) => globMatch(name, n)) ? "tag" : null;
  }
  const reach = scopeCoverage(r.selector, p, catalogId);
  return reach === "here" || (reach !== null && reach !== "child") ? "path" : null;
}

/**
 * The tag names (`tags.name`, what the engine matches; see
 * {@link ruleSelects}) that reach `p`, from its table's tag assignments,
 * mirroring how egret resolves a target's tags for explain and serves them to
 * Ranger:
 *
 * - **A table's tag reaches each of its columns.** egret serves a table-level
 *   assignment as a resource with no `column` element
 *   (`crates/egret-policy-api/src/dto.rs:193-214`, `ServiceTags::from_assignments`),
 *   which Ranger's tag enricher hands a column request as an `ANCESTOR` tag,
 *   and Ranger admits an `ANCESTOR` tag for a tag policy
 *   (`crates/egret-ranger-eval/src/eval.rs:5198-5203`, `tag_match_is_admitted`;
 *   the 2.8.0 disjunct cited at `eval.rs:4781-4783`). Explain resolves it the
 *   same way: `resolved_tags_for_resource_via` returns a table-level row for a
 *   column request as `Ancestor` (`crates/egret-policy/src/store.rs:1689-1717`),
 *   reported as `ExplainResolvedTag.attached_at_column = false`
 *   (`crates/egret-perm-compile/src/routes.rs:686-698`).
 * - **A column's tag does not reach its table.** For a table request it would
 *   be `DESCENDANT`, which the enricher rejects; the same query excludes it
 *   (`store.rs:1656-1658`, point 3; the query at `store.rs:1689-1698`).
 * - **Nothing above a table carries a tag** (migration 0177; `crates/egret-perm/src/eval.rs:400-405`).
 *
 * Schema, table and column compare without regard to case, as that query's
 * `lower(…) = lower(…)` does; the names themselves are kept as stored. Each
 * name appears once.
 */
export function tagNamesOn(assignments: ResourceTagDto[], p: ObjectPath): string[] {
  if (p.schema === undefined || p.object === undefined) return [];
  const names = new Set<string>();
  for (const a of assignments) {
    if (!sameName(a.schema, p.schema) || !sameName(a.table, p.object)) continue;
    if (a.column === "" || (p.column !== undefined && sameName(a.column, p.column))) names.add(a.tag.name);
  }
  return [...names];
}

/** Schema, table and column names compare without regard to case, as egret resolves tags and the engines match columns. */
export function sameName(a: string, b: string): boolean {
  return a.toLowerCase() === b.toLowerCase();
}

/**
 * What a "child" scope ({@link scopeCoverage}) is set on, named from `p`
 * down: `email` on the table `orders`, `orders.email` on the schema `sales`,
 * `sales.orders` on the catalog. A level set to a pattern is quoted, as the
 * grant's sentence quotes it.
 */
export function insideName(scope: Scope, p: ObjectPath): string {
  const depth = p.schema === undefined ? 0 : p.object === undefined ? 1 : 2;
  return prefix([scope.schema, scope.table, scope.column])
    .slice(depth)
    .map((v) => (isPattern(v) ? `“${v}”` : v))
    .join(".");
}

/** The heading and the name column of the group that lists the "child" scopes of `p`'s page. */
export function insideGroup(p: ObjectPath): { title: string; head: string } {
  if (p.schema === undefined) return { title: "Inside this catalog", head: "Object" };
  if (p.object === undefined) return { title: "Inside this schema", head: "Object" };
  return { title: "On columns", head: "Column" };
}

// Defined in `permFormat.ts`, beside `isPattern`, so pages outside this area can use them.
export { patternLevels, type PatternLevel } from "@/routes/governance/permFormat";

/** The levels up to the first unset one. */
function prefix(levels: (string | null | undefined)[]): string[] {
  const end = levels.findIndex((l) => l === null || l === undefined);
  return (end === -1 ? levels : levels.slice(0, end)) as string[];
}
