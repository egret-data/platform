import type { Grant } from "@/api/generated/model";
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
 * table of its schema.
 *
 * The scope's levels narrow without gaps (a column implies a table implies a
 * schema; `perm.grants` CHECKs it), so the set levels are a prefix.
 */
export function grantCoverage(g: Grant, p: ObjectPath, catalogId: string): GrantCoverage {
  if (g.scope.catalog_id !== catalogId) return null;
  const granted = prefix([g.scope.schema, g.scope.table, g.scope.column]);
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
