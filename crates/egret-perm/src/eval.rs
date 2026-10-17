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
//! Five bounds on what this function can prove, recorded rather than fixed:
//!
//! - **Leaf granularity.** `covers` is strictly grant-scope ⊇ target-scope
//!   (see its doc). A deny scoped to `sales.orders.email` does not deny a
//!   target scoped only to `sales.orders` — the two `Scope`s are not
//!   comparable by `covers` in that direction. Callers must evaluate at the
//!   leaf level actually being read (the column, if a column is being read)
//!   rather than at a coarser level and expect a finer-grained deny to apply.
//! - **Group principals were structurally invisible to the real engine, and
//!   are NOT any more (J-F3 Task 1b, design R8).** Task 1 measured, on the
//!   live cluster, that Trino has no group provider and egret never pushes one
//!   (no usersync), so a Ranger policy item naming a group matches no one on
//!   any engine egret provisions — see
//!   `docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`. Until R8
//!   the compiler named the group anyway, so this function reported an allow
//!   the engine refused (it grew a separate `AllowedByGroupOnly` class to say
//!   so) and a deny the engine did not enforce. A group's membership IS
//!   knowable from `user_groups`, so the compiler now expands a non-`public`
//!   group into its members' usernames (`egret_perm_compile::members`) and
//!   every writer of `user_groups` recompiles. This function is unchanged —
//!   matching `Subject::groups` by name is the right oracle semantics — and the
//!   `AllowedByGroupOnly` class is gone, because the access it warned about is
//!   now the enforceable kind.
//! - **`Role` principals were in that same bound, and are NOT any more (SP4c
//!   Task 2b).** SP4c's D0 measured that a Ranger item naming a role matches
//!   nobody either, for its own reason (the plugin downloads roles from a route
//!   egret does not serve, and Trino's plugin passes `userRoles` as `null`) —
//!   so `explain` was reporting an access the engine did not grant, exactly as
//!   for a group. Unlike a group, a role's membership IS knowable from
//!   `access_grants`, so the compiler now expands a `role` principal into its
//!   holders' usernames (`egret_perm_compile::members`) instead of naming the
//!   role. This function is unchanged — matching `Subject::roles` by name is
//!   still the right oracle semantics — but the divergence it could not see is
//!   closed on the compiler's side, and
//!   `egret-perm-compile/tests/role_grants.rs` asserts the two now AGREE about
//!   a role-granted subject.
//! - **A tag's clearance (SP4c Task 4) is the fourth thing this function can
//!   see that the model above did not.** `egret_perm_compile::clearance`
//!   compiles a gated tag's clearance to a `{public}` deny excepting the
//!   required role's members, enumerated the same way a `Role` grant is
//!   (D0/D0b) — so, exactly like a `Role` grant, `Subject::roles` is the right
//!   oracle semantics and no DB read belongs in this function. Unlike a
//!   grant, a clearance is not in `grants` at all (D1 amends SP4a's D2 that
//!   tags drive rules only, never access) and it wins over every grant rather
//!   than competing with one — so `evaluate` checks it first, before any
//!   grant, matching the tag-first short-circuit D1 relies on. Only a tag
//!   whose `required_roles` is non-empty is passed in [`ClearanceGate`]: a
//!   `required_groups`-only gate compiles to nothing (D2/D7 — a group name
//!   matches nobody in the engine, so a deny for it would withhold the column
//!   from everybody, not from the intended few), and reporting a denial for
//!   it here would make `explain` disagree with the engine in the opposite
//!   direction from the bug this task fixes.
//! - **The resource-level fold is a full-string lowercase, and the engine's is
//!   not (design R8).** The schema, table and column levels fold case like the
//!   Ranger service-defs egret serves (see [`resource_level_matches`]), but
//!   with Rust's `str::to_lowercase`, the same full Unicode mapping as
//!   Postgres `lower()` and JS `toLowerCase`. The engine folds per character
//!   (`String.equalsIgnoreCase`), and the two can disagree on 25 code points:
//!   U+00B5, U+0130, U+0131, U+017F, U+0345, U+03A3, U+03C2, U+03D0, U+03D1,
//!   U+03D5, U+03D6, U+03F0, U+03F1, U+03F5, U+1C80–U+1C88 and U+1E9B,
//!   U+1FBE. That set, its derivation and its measurement are on
//!   `egret_perm_compile::routes::FOLD_DIVERGENT`, where tag resolution
//!   DECLINES on it. `evaluate` has no decline channel, and no production
//!   caller (it is the test oracle), so here it is a bound and not a gate: a
//!   name carrying one of those characters may get an answer the engine does
//!   not give. The constant is not copied into this crate — egret-perm does
//!   not depend on egret-perm-compile, and a second copy would drift. Tag keys
//!   ([`Selector::Tag`], [`ClearanceGate::tag`]) and principal refs are NOT
//!   folded at all (design R7): the `tag` service-def sets
//!   `ignoreCase: false`, so `PII` and `pii` are two tags on the engine too.

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

/// One tag's clearance gate, as `egret_perm_compile::clearance` compiles it —
/// only ever passed for a tag whose `required_roles` is non-empty, since a
/// `required_groups`-only gate compiles to no engine deny at all (D2/D7) and
/// `evaluate` must not report a denial the engine does not enforce.
/// `required_roles`' membership is knowable the same way a `Role` grant's is
/// (D0/D0b): `Subject::roles` already carries it, so this struct names the
/// tag and the roles that clear it, not any member list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearanceGate {
    /// `tags.key` — the same identifier `tags_on` and `selector_hits` use,
    /// so a caller does not need a second tag lookup to line the two up.
    pub tag: String,
    pub required_roles: Vec<String>,
}

/// Why the decision came out the way it did. `NoGrantMatched` and
/// `ExplicitDeny` are deliberately distinct: a subject a grant's `exclude`
/// subtracted matches no grant at all, and collapsing that into "denied"
/// would say something the model never asserted.
///
/// `ClearanceRequired` is likewise its own class, not a flavour of
/// `ExplicitDeny` or `NoGrantMatched` (SP4c Task 4/D4). Both of those would
/// tell an administrator to look at a grant; a clearance denial has no grant
/// to look at — the wrong grant does not exist to fix, a person needs the
/// clearing role. SP4b's five reason classes are independent for exactly this
/// reason: collapsing this into `ExplicitDeny` (or reporting `NoGrantMatched`,
/// today's status quo) would make two different facts about the model read as
/// the same sentence, "denied," which is the failure this whole mechanism
/// exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    AllowedByGrant {
        grant_id: Uuid,
    },
    ExplicitDeny {
        grant_id: Uuid,
    },
    NoGrantMatched,
    MaskedByRule {
        rule_id: Uuid,
        mask_type: String,
    },
    FilteredByRule {
        rule_id: Uuid,
        filter_expr: String,
    },
    ClearanceRequired {
        tag: String,
        required_roles: Vec<String>,
    },
}

/// The decision. `allowed` is the boolean an enforcer would ask for;
/// `reasons` is the deliverable this function exists for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub reasons: Vec<Reason>,
}

/// D2 in code, amended by SP4c D1/D4 for clearance. Deny beats allow
/// regardless of grant order; a target not reached by any grant is
/// `NoGrantMatched`, never a deny; masks and row filters are evaluated only
/// once access is allowed, and apply "however the access was granted" — they
/// are checked against `target`, not against the grant that let the subject
/// in. A clearance gate on a tag present on `target` is checked FIRST, ahead
/// of every grant: D1 compiles it to a tag policy that wins over any resource
/// grant by evaluating first, and this function must decide in the same
/// order or `explain` and the compiled policy could disagree about the same
/// subject — exactly the disagreement SP4c exists to close.
pub fn evaluate(
    subject: &Subject,
    action: Action,
    target: &Scope,
    grants: &[Grant],
    rules: &[Rule],
    tags_on: &[String],
    clearances: &[ClearanceGate],
) -> Decision {
    // Cleared = holds AT LEAST ONE of the gate's required roles — the same
    // "OR across roles" semantics `egret_perm_compile::clearance` compiles
    // (its `cleared` exception list is the union of each role's members).
    if let Some(gate) = clearances.iter().find(|g| {
        tags_on.iter().any(|t| t == &g.tag)
            && !g.required_roles.iter().any(|r| subject.roles.contains(r))
    }) {
        return Decision {
            allowed: false,
            reasons: vec![Reason::ClearanceRequired {
                tag: gate.tag.clone(),
                required_roles: gate.required_roles.clone(),
            }],
        };
    }

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

    // One allow class, however the subject matched: since J-F3 Task 1b a
    // `group` compiles to its members' usernames exactly as a `role` does, so a
    // group-matched allow is as enforceable as any other (module doc).
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

/// Ranger's built-in "everybody" group, the ONE group name that resolves with
/// no group provider and no usersync.
///
/// It is the one group the compiler does NOT expand into members
/// (`egret_perm_compile::members`, J-F3 Task 1b): SP4c's own clearance compiler
/// puts this literal on the deny side of a tag policy to mean everybody,
/// because D0 measured that it is the half of Ranger's deny/deny-exception pair
/// that works. `egret_perm_compile::rules` aliases this constant rather than
/// re-spelling it; two copies would drift, and a drift here would expand
/// "everyone" into whoever happens to have a row in a group of that name.
pub const PUBLIC_GROUP: &str = "public";

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
/// resolve against a named target value is matched by
/// [`resource_level_matches`] — a glob, not plain equality, and folded for
/// case; see its doc and `glob_match`'s for why.
fn level_covers(grant: &Option<String>, target: &Option<String>) -> bool {
    match (grant, target) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(g), Some(t)) => resource_level_matches(g, t),
    }
}

/// Whether a stored schema, table or column value matches a target's value
/// at the same level, the way the engine decides it: case-folded, then
/// glob-matched.
///
/// Every resource level egret's Ranger service-defs declare sets
/// `"ignoreCase": true` — Trino's `catalog`/`schema`/`table`/`column` in
/// `egret-policy/src/service-defs/trino.json` and StarRocks'
/// `catalog`/`database`/`table`/`column` in `starrocks.json` — so the engine
/// treats a grant on `sales.Orders` as a grant on `sales.orders`. Comparing
/// exactly would make this oracle miss a deny the engine enforces, which is
/// the fail-open direction: an oracle that misses a deny blesses a compiler
/// that drops one.
///
/// The fold is a whole-string `str::to_lowercase` on both sides — the same
/// function as `egret_ranger_eval::resource::fold`, the server's own fold,
/// and as the Data UI's `toLowerCase` in
/// `frontend/src/routes/governance/data/coverage.ts`, so the three agree.
/// Where that full-string fold and the engine's per-character one disagree is
/// the fifth bound in the module doc. The catalog level is not here: `covers`
/// compares it by UUID. Tag keys are not here either; they stay exact.
pub fn resource_level_matches(pattern: &str, value: &str) -> bool {
    glob_match(&pattern.to_lowercase(), &value.to_lowercase())
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
///
/// It compares characters exactly, which is the `tag` service-def's
/// `ignoreCase: false` and what the Data UI's port of it expects. A resource
/// level must go through [`resource_level_matches`], which folds case first.
pub fn glob_match(pattern: &str, value: &str) -> bool {
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
mod resource_level_matches_tests {
    use super::resource_level_matches;

    #[test]
    fn a_literal_matches_across_case() {
        assert!(resource_level_matches("Orders", "orders"));
        assert!(resource_level_matches("orders", "Orders"));
    }

    #[test]
    fn a_glob_matches_across_case() {
        assert!(resource_level_matches("ORD*", "orders"));
        assert!(resource_level_matches("s?LES", "Sales"));
    }

    #[test]
    fn a_non_ascii_name_folds_too() {
        // An ASCII-only fold (`to_ascii_lowercase`) leaves `Ư`/`Ờ` alone and
        // misses this pair; the engine does not.
        assert!(resource_level_matches("NGƯỜI", "người"));
    }

    /// Pins the fifth bound in the module doc, so a change to it is seen.
    /// U+0130 (`İ`) is in `FOLD_DIVERGENT`: its full lowercase is TWO code
    /// points (`i` + U+0307), so this fold says no, while the engine's
    /// per-character `equalsIgnoreCase` lowers `İ` to `i` and says yes. Here
    /// the oracle is narrower than the engine, and that is the recorded bound,
    /// not a gate.
    #[test]
    fn a_fold_divergent_character_is_the_documented_bound() {
        assert!(!resource_level_matches("İ", "i"));
        assert!(resource_level_matches("İ", "i\u{0307}"));
    }

    #[test]
    fn a_literal_still_needs_the_same_letters() {
        assert!(!resource_level_matches("orders", "order"));
        assert!(!resource_level_matches("ORDERS", "order"));
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
