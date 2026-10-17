import type { Grant, ResourceTagDto, RuleView, Scope } from "@/api/generated/model";
import { isPattern } from "@/routes/governance/permFormat";
import type { ObjectPath } from "@/routes/governance/data/objectPath";

/** How a grant reaches an object: set on it, set on an ancestor, or set on one of its columns. */
export type GrantCoverage = "here" | { inheritedFrom: "catalog" | "schema" | "table" } | "child" | null;

const ANCESTORS = ["catalog", "schema", "table"] as const;

/**
 * How grant `g` reaches `p`, an object of the catalog whose id is `catalogId`.
 *
 * A grant reaches `p` when its catalog is `p`'s and each level it sets
 * matches `p`'s at that level (an unset level is every value). It is "here"
 * when it is set at `p`'s own level, inherited when it is set higher up, and
 * "child" when it is set on a column of the table `p` (a table, view or
 * materialized view). A grant set deeper than that (a table, for a schema's
 * page) does not reach `p` and is `null`.
 *
 * A level matches as egret's own evaluator matches it: by glob (`*` any run of
 * characters, `?` one), not by equality (`crates/egret-perm/src/eval.rs`,
 * `covers` and `level_covers`), because the engines enforce a stored `ord*`
 * or `*` that way. A grant with `table = "*"` is therefore "here" on every
 * table of its schema; {@link patternLevels} tells such a grant apart from one
 * set on this object by name.
 *
 * The scope's levels narrow without gaps (a column implies a table implies a
 * schema; `perm.grants` CHECKs it), so the set levels are a prefix.
 */
export function grantCoverage(g: Grant, p: ObjectPath, catalogId: string): GrantCoverage {
  return scopeCoverage(g.scope, p, catalogId);
}

/**
 * How a scope reaches `p`: {@link grantCoverage}'s rule, for any scope. A
 * rule's path selector is a `Scope` (`Selector`'s `path` variant), and
 * egret-perm checks it with the same `covers` a grant's scope gets
 * (`crates/egret-perm/src/eval.rs`, `selector_hits`).
 */
export function scopeCoverage(scope: Scope, p: ObjectPath, catalogId: string): GrantCoverage {
  if (scope.catalog_id !== catalogId) return null;
  const granted = prefix([scope.schema, scope.table, scope.column]);
  const target = prefix([p.schema, p.schema === undefined ? undefined : p.object, p.object === undefined ? undefined : p.column]);

  const shared = Math.min(granted.length, target.length);
  for (let i = 0; i < shared; i++) {
    if (!globMatch(granted[i], target[i])) return null;
  }
  if (granted.length === target.length) return "here";
  if (granted.length < target.length) return { inheritedFrom: ANCESTORS[granted.length] };
  // Deeper than `p`: only a column of the table `p` is listed.
  return target.length === 2 && granted.length === 3 ? "child" : null;
}

/**
 * Whether rule `r` selects `p` (an object of the catalog `catalogId`, or one
 * of its columns), and by what: its path, or a tag `p` carries. F10: this
 * lists the rules that select something, it never says who sees what; only
 * explain evaluates.
 *
 * A path selector selects `p` when it is set at `p`'s level or above and each
 * level it sets matches (by glob), as egret-perm's `selector_hits` does
 * (`crates/egret-perm/src/eval.rs:413-418`, `Selector::Path` → `covers`). A
 * selector set below `p` (a column, for a table) does not select `p`.
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

/** Whether `r` selects by a tag whose name did not resolve (see {@link ruleSelects}). */
export function tagUnresolved(r: RuleView): boolean {
  return r.selector.type === "tag" && !r.tag_name;
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

export type PatternLevel = { level: "schema" | "table" | "column"; value: string };

/** The levels of `scope` set to a pattern (`*`, `ord*`, `order?`): each names every value it matches. */
export function patternLevels(scope: Scope): PatternLevel[] {
  const levels: PatternLevel[] = [];
  for (const level of ["schema", "table", "column"] as const) {
    const value = scope[level];
    if (value && isPattern(value)) levels.push({ level, value });
  }
  return levels;
}

/** The levels up to the first unset one. */
function prefix(levels: (string | null | undefined)[]): string[] {
  const end = levels.findIndex((l) => l === null || l === undefined);
  return (end === -1 ? levels : levels.slice(0, end)) as string[];
}

/**
 * Ranger's resource glob: `*` matches any run of characters (none included),
 * `?` exactly one. A port of `glob_match` in `crates/egret-perm/src/eval.rs`,
 * over code points as Rust's `chars()` walks them.
 */
export function globMatch(patternText: string, valueText: string): boolean {
  const pattern = Array.from(patternText);
  const value = Array.from(valueText);
  let pi = 0;
  let vi = 0;
  let star = -1;
  let starVi = 0;
  while (vi < value.length) {
    if (pi < pattern.length && (pattern[pi] === "?" || pattern[pi] === value[vi])) {
      pi++;
      vi++;
    } else if (pi < pattern.length && pattern[pi] === "*") {
      star = pi;
      starVi = vi;
      pi++;
    } else if (star !== -1) {
      pi = star + 1;
      starVi++;
      vi = starVi;
    } else {
      return false;
    }
  }
  while (pi < pattern.length && pattern[pi] === "*") pi++;
  return pi === pattern.length;
}
