//! The evaluator: decides whether a subject may take an action on a target,
//! and says why.
//!
//! This is not the enforcer — Trino and StarRocks enforce, via Ranger
//! policies a later task compiles from this same model. This function exists
//! so egret can explain a decision with provenance, which Ranger's own audit
//! cannot do (its Access tab shows `-` for the most common denial: nothing
//! matched). A later task also uses this evaluator as the compiler's oracle,
//! so its semantics must be exactly D2, not approximately.
//!
//! D2, verbatim: *you can see it if some grant allows it, unless some grant
//! denies it; masks and row filters always apply, however the access was
//! granted.* A rule's `exempt` set is how an individual escapes a mask —
//! never a competing grant.
//!
//! Two bounds on what this function can prove, recorded rather than fixed:
//!
//! - **Leaf granularity.** `covers` is strictly grant-scope ⊇ target-scope
//!   (see its doc). A deny scoped to `sales.orders.email` does not deny a
//!   target scoped only to `sales.orders` — the two `Scope`s are not
//!   comparable by `covers` in that direction. Callers must evaluate at the
//!   leaf level actually being read (the column, if a column is being read)
//!   rather than at a coarser level and expect a finer-grained deny to apply.
//! - **Group principals are structurally invisible to the real engine.**
//!   Task 1 measured, on the live cluster, that Trino has no group provider
//!   and egret never pushes one (no usersync), so a Ranger policy item
//!   naming a group matches no one on any engine egret provisions today —
//!   see `docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`. This
//!   evaluator has no way to know that: a `Group` principal is matched
//!   against `Subject.groups` exactly like `User`/`Role`, so `evaluate` will
//!   report `AllowedByGrant` for a group grant that the real engine denies.
//!   Because a later task checks the compiler against this same evaluator as
//!   its oracle, the compiler will agree — the oracle is blind to this whole
//!   class of dead grant, and nothing downstream of `evaluate` alone can
//!   catch it.

use uuid::Uuid;

use crate::model::{
    Action, Effect, Grant, Principal, PrincipalKind, PrincipalSet, Rule, RuleKind, Scope, Selector,
};

/// The caller of `evaluate`. `groups`/`roles` are the bare identifiers D7
/// settled on; `workspaces` is the set of workspace ids the subject belongs
/// to, used to match a `PrincipalKind::WorkspaceMembers` principal (Task 1
/// measured that groups themselves never reach the engine, so this
/// expansion happens here and, at compile time, in the Ranger compiler).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    pub user: String,
    pub groups: Vec<String>,
    pub roles: Vec<String>,
    pub workspaces: Vec<Uuid>,
}

/// Why the decision came out the way it did. `NoGrantMatched` and
/// `ExplicitDeny` are deliberately distinct: a subject a grant's `exclude`
/// subtracted matches no grant at all, and collapsing that into "denied"
/// would say something the model never asserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    AllowedByGrant { grant_id: Uuid },
    ExplicitDeny { grant_id: Uuid },
    NoGrantMatched,
    MaskedByRule { rule_id: Uuid, mask_type: String },
    FilteredByRule { rule_id: Uuid, filter_expr: String },
}

/// The decision. `allowed` is the boolean an enforcer would ask for;
/// `reasons` is the deliverable this function exists for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub reasons: Vec<Reason>,
}

/// D2 in code. Deny beats allow regardless of grant order; a target not
/// reached by any grant is `NoGrantMatched`, never a deny; masks and row
/// filters are evaluated only once access is allowed, and apply "however
/// the access was granted" — they are checked against `target`, not against
/// the grant that let the subject in.
pub fn evaluate(
    subject: &Subject,
    action: Action,
    target: &Scope,
    grants: &[Grant],
    rules: &[Rule],
    tags_on: &[String],
) -> Decision {
    let applicable = |g: &Grant| {
        g.actions.contains(&action)
            && covers(&g.scope, target)
            && matches_set(subject, &g.principals)
    };

    // Deny wins: a single matching deny ends it whatever else allows. D2
    // makes no promise about which deny is reported when several match, only
    // that a match wins, so `find`'s first hit is enough.
    if let Some(d) = grants
        .iter()
        .find(|g| g.effect == Effect::Deny && applicable(g))
    {
        return Decision {
            allowed: false,
            reasons: vec![Reason::ExplicitDeny { grant_id: d.id }],
        };
    }

    let allow = grants
        .iter()
        .find(|g| g.effect == Effect::Allow && applicable(g));
    let Some(a) = allow else {
        return Decision {
            allowed: false,
            reasons: vec![Reason::NoGrantMatched],
        };
    };

    let mut reasons = vec![Reason::AllowedByGrant { grant_id: a.id }];
    for r in rules {
        if !selector_hits(&r.selector, target, tags_on) {
            continue;
        }
        // The exempt set is a PrincipalSet exactly like a grant's principals
        // (include minus exclude) — "finance except contractors" as an
        // exemption still masks a contractor in finance, same as it would
        // fail to match as a grant's principals.
        if matches_set(subject, &r.exempt) {
            continue;
        }
        reasons.push(match &r.kind {
            RuleKind::Mask { mask_type, .. } => Reason::MaskedByRule {
                rule_id: r.id,
                mask_type: mask_type.clone(),
            },
            RuleKind::RowFilter { filter_expr } => Reason::FilteredByRule {
                rule_id: r.id,
                filter_expr: filter_expr.clone(),
            },
        });
    }
    Decision {
        allowed: true,
        reasons,
    }
}

/// `include` minus `exclude` — the operator that lets egret drop Ranger's
/// allow-exception and deny-exception blocks from the authoring surface
/// (D1). Used identically for a grant's `principals` and a rule's `exempt`.
fn matches_set(s: &Subject, set: &PrincipalSet) -> bool {
    set.include.iter().any(|p| matches_principal(s, p))
        && !set.exclude.iter().any(|p| matches_principal(s, p))
}

fn matches_principal(s: &Subject, p: &Principal) -> bool {
    match p.kind {
        PrincipalKind::User => s.user == p.r#ref,
        PrincipalKind::Group => s.groups.iter().any(|g| g == &p.r#ref),
        PrincipalKind::Role => s.roles.iter().any(|r| r == &p.r#ref),
        PrincipalKind::WorkspaceMembers => p
            .r#ref
            .parse::<Uuid>()
            .map(|w| s.workspaces.contains(&w))
            .unwrap_or(false),
    }
}

/// A grant's (or a path-selector rule's) scope covers a target when every
/// level the grant names matches, and the grant's catalog is exactly the
/// target's. **This is strictly grant ⊇ target — the reverse never holds.**
/// A grant scoped to `sales.orders` does not cover a target scoped only to
/// `sales.orders.email`; conversely (see `level_covers`) a grant naming a
/// value at a level the target leaves unspecified never matches either,
/// because there is nothing to test the value against. Callers must
/// evaluate at the leaf granularity of the actual read.
fn covers(g: &Scope, t: &Scope) -> bool {
    if g.catalog_id != t.catalog_id {
        return false;
    }
    level_covers(&g.schema, &t.schema)
        && level_covers(&g.table, &t.table)
        && level_covers(&g.column, &t.column)
}

/// `None` on the grant side means "everything below" and matches any target
/// (including a target that itself leaves the level unspecified). Once the
/// grant names a value, the target must name one too — `(Some(_), None)` is
/// `false`, deliberately NOT the same as `(None, None)`: a grant explicitly
/// scoped to a level (even to `"*"`, matching every value there) says
/// nothing about a target that IS the object one level up and does not
/// descend to a specific value at this level. A named grant value that does
/// resolve against a named target value is matched by `glob_match`, not
/// plain equality — see its doc for why.
fn level_covers(grant: &Option<String>, target: &Option<String>) -> bool {
    match (grant, target) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(g), Some(t)) => glob_match(g, t),
    }
}

/// Ranger's resource matcher globs (`*` = any run of characters including
/// none, `?` = exactly one character) rather than testing string equality.
/// Migration `0154` puts no CHECK on `schema_name`/`table_name`/`column_name`
/// restricting them to a closed `*`-or-literal vocabulary, so a stored scope
/// value such as `"ord*"` is possible today and the live engine glob-matches
/// it. An evaluator that only special-cased the literal string `"*"` would
/// then say DENIED/NoGrantMatched for a target the engine actually allows —
/// silent under-matching, which is worse than over-matching here because a
/// later task checks the Ranger compiler against this evaluator as an
/// oracle: an oracle that under-matches would bless a compiler that
/// over-permits. So this implements real glob matching rather than treating
/// `"*"` as a single special case.
fn glob_match(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    // Classic greedy wildcard match with backtracking: `star` remembers the
    // last `*` seen in the pattern and how far into `value` we'd consumed
    // when we saw it, so a later mismatch can retry by having that `*`
    // absorb one more character instead of failing outright.
    let (mut pi, mut vi) = (0usize, 0usize);
    let (mut star, mut star_vi) = (None::<usize>, 0usize);
    while vi < v.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == v[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            star_vi = vi;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            star_vi += 1;
            vi = star_vi;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Whether a rule's selector hits `target`. A tag selector is checked
/// against `tags_on` — the tags the caller has already resolved as present
/// on `target` (tags attach to tables/columns only — `catalog_tag_assignments`
/// is keyed `(tag_id, warehouse_id, schema_name, table_name, column_name)`
/// with `column_name = ''` meaning table-level, and there is no catalog- or
/// schema-level tag — so resolving *which* tags apply is necessarily the
/// caller's job: it needs `catalog_tag_assignments`, which this crate does
/// not read, keeping the evaluator DB-free). What this function DOES own is
/// the boundary: a target that names no table (schema- or catalog-level)
/// cannot carry a tag by construction, so a tag selector can never hit it —
/// guarded here rather than trusted to every caller, so a caller that passes
/// `tags_on` for a schema-level target cannot make this function claim a
/// mask applies where no tag assignment could ever exist. A path selector is
/// checked the same way a grant's scope is: `covers`.
fn selector_hits(selector: &Selector, target: &Scope, tags_on: &[String]) -> bool {
    match selector {
        Selector::Tag { key } => target.table.is_some() && tags_on.iter().any(|t| t == key),
        Selector::Path(scope) => covers(scope, target),
    }
}

#[cfg(test)]
mod glob_match_tests {
    use super::glob_match;

    #[test]
    fn literal_still_requires_exact_equality() {
        assert!(glob_match("orders", "orders"));
        assert!(!glob_match("orders", "order"));
        assert!(!glob_match("orders", "orders2"));
    }

    #[test]
    fn bare_star_matches_anything_including_empty() {
        assert!(glob_match("*", "orders"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn trailing_star_matches_a_prefix() {
        assert!(glob_match("ord*", "orders"));
        assert!(glob_match("ord*", "ord"));
        assert!(!glob_match("ord*", "customers"));
        assert!(!glob_match("ord*", "xord"));
    }

    #[test]
    fn star_in_the_middle_matches_a_run_of_any_length() {
        assert!(glob_match("s*s", "sales"));
        assert!(glob_match("s*s", "ss"));
        assert!(!glob_match("s*s", "sale"));
    }

    #[test]
    fn question_mark_matches_exactly_one_character() {
        assert!(glob_match("s?les", "sales"));
        assert!(!glob_match("s?les", "smiles"));
        assert!(!glob_match("s?les", "sles"));
    }
}
