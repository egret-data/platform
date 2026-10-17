import type { RuleView } from "@/api/generated/model";

/*
 * How a tag-selected rule finds its tag (DG redesign #2, P9). `globMatch` and
 * `tagUnresolved` moved here unchanged from `data/coverage.ts`, which imports
 * them: the Classifications pages are not in the lazy Data area, and nothing
 * under `data/` may be imported statically from the entry graph
 * (`scripts/assert-governance-data-lazy.mjs`).
 */

/**
 * Ranger's resource glob: `*` matches any run of characters (none included),
 * `?` exactly one. A port of `glob_match` in `crates/egret-perm/src/eval.rs`,
 * over code points as Rust's `chars()` walks them. It compares case-sensitively,
 * as Ranger's tag matcher does (`ignoreCase: false`); `scopeCoverage` (`data/coverage.ts`)
 * lower-cases both sides first for resource levels, which ignore case.
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

/** Whether `r` selects by a tag whose name did not resolve (see `ruleSelects` in `data/coverage.ts`): its tag was deleted, since `delete_tag` deletes the tag only. */
export function tagUnresolved(r: RuleView): boolean {
  return r.selector.type === "tag" && !r.tag_name;
}

/**
 * The rules that reach the tag `tag` on engines: a tag rule is matched by its
 * tag's NAME (`tag_name`, what egret serves Ranger), case-sensitively and by
 * glob, so a rule written on another tag of the same name (names are not
 * unique) reaches this tag too, and its key alone never does. A rule whose
 * tag did not resolve reaches none.
 */
export function rulesOnTag(rules: RuleView[], tag: { name: string }): RuleView[] {
  return rules.filter((r) => r.selector.type === "tag" && !!r.tag_name && globMatch(r.tag_name, tag.name));
}

/**
 * The tag rules whose tag is gone: its name did not resolve (`tagUnresolved`),
 * or, when the tag list is known (`tagKeys`), no listed tag has its key.
 * Without the list (a reader without `tag:read`), the server's resolution
 * alone decides.
 */
export function orphanRules(rules: RuleView[], tagKeys: ReadonlySet<string> | null): RuleView[] {
  return rules.filter(
    (r) => r.selector.type === "tag" && (tagUnresolved(r) || (tagKeys !== null && !tagKeys.has(r.selector.key))),
  );
}
