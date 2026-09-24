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
/// level the grant names matches. A level left `None` means "everything
/// below"; `"*"` at a named level means the same thing at that level — the
/// spec records scope as "nullable, wildcards allowed".
fn covers(g: &Scope, t: &Scope) -> bool {
    if g.catalog_id != t.catalog_id {
        return false;
    }
    level_covers(&g.schema, &t.schema)
        && level_covers(&g.table, &t.table)
        && level_covers(&g.column, &t.column)
}

fn level_covers(grant: &Option<String>, target: &Option<String>) -> bool {
    match (grant, target) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(g), Some(t)) => g == "*" || g == t,
    }
}

/// Whether a rule's selector hits `target`. A tag selector is checked
/// against `tags_on` — the tags the caller has already resolved as present
/// on `target` (tags attach to tables/columns only; there is no catalog- or
/// schema-level tag, per the spec, so resolving that is the caller's job,
/// not this function's). A path selector is checked the same way a grant's
/// scope is: `covers`.
fn selector_hits(selector: &Selector, target: &Scope, tags_on: &[String]) -> bool {
    match selector {
        Selector::Tag { key } => tags_on.iter().any(|t| t == key),
        Selector::Path(scope) => covers(scope, target),
    }
}
