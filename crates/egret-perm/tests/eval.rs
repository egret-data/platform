//! D2: you can see it if some grant allows it, unless some grant denies it;
//! masks and row filters always apply, however the access was granted.
//!
//! Pure (no DB) — the evaluator is a function of its arguments only.

// SP4d-2a Task 5: `evaluate` is no longer re-exported at the crate root — it
// is an ORACLE now, not what `/perm/explain` answers with, so reaching it
// takes naming the module on purpose. See `egret_perm`'s own lib.rs note.
use egret_perm::eval::evaluate;
use egret_perm::{
    Action, ClearanceGate, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet,
    Reason, Rule, RuleKind, Scope, Selector, Subject,
};
use uuid::Uuid;

// Dumb helpers only — a helper that computes anything is a second
// implementation of the thing under test. All scopes share one fixed
// catalog id, except the two tests that deliberately cross catalogs.
const CATALOG: Uuid = Uuid::nil();
const OTHER_CATALOG: Uuid = Uuid::from_u128(1);

fn grant(effect: Effect, include: &[&str], exclude: &[&str], scope: Scope) -> Grant {
    Grant {
        id: Uuid::new_v4(),
        effect,
        actions: vec![Action::Read],
        scope,
        principals: PrincipalSet {
            include: include
                .iter()
                .map(|g| Principal {
                    kind: PrincipalKind::Group,
                    r#ref: (*g).into(),
                })
                .collect(),
            exclude: exclude
                .iter()
                .map(|g| Principal {
                    kind: PrincipalKind::Group,
                    r#ref: (*g).into(),
                })
                .collect(),
        },
        origin: GrantOrigin::Explicit,
    }
}

fn scope_schema(schema: &str) -> Scope {
    Scope {
        catalog_id: CATALOG,
        schema: Some(schema.into()),
        table: None,
        column: None,
    }
}

fn scope_table(schema: &str, table: &str) -> Scope {
    Scope {
        catalog_id: CATALOG,
        schema: Some(schema.into()),
        table: Some(table.into()),
        column: None,
    }
}

fn scope_column(schema: &str, table: &str, column: &str) -> Scope {
    Scope {
        catalog_id: CATALOG,
        schema: Some(schema.into()),
        table: Some(table.into()),
        column: Some(column.into()),
    }
}

fn subject_in_group(group: &str) -> Subject {
    Subject {
        user: "u".into(),
        groups: vec![group.into()],
        roles: vec![],
        workspaces: vec![],
    }
}

fn mask_rule_on_tag(tag_key: &str, mask_type: &str, exempt_groups: &[&str]) -> Rule {
    Rule {
        id: Uuid::new_v4(),
        name: format!("mask_{tag_key}"),
        kind: RuleKind::Mask {
            mask_type: mask_type.into(),
            value_expr: None,
        },
        selector: Selector::Tag {
            key: tag_key.into(),
        },
        exempt: PrincipalSet {
            include: exempt_groups
                .iter()
                .map(|g| Principal {
                    kind: PrincipalKind::Group,
                    r#ref: (*g).into(),
                })
                .collect(),
            exclude: vec![],
        },
    }
}

fn row_filter_rule_on_path(name: &str, scope: Scope, filter_expr: &str) -> Rule {
    Rule {
        id: Uuid::new_v4(),
        name: name.into(),
        kind: RuleKind::RowFilter {
            filter_expr: filter_expr.into(),
        },
        selector: Selector::Path(scope),
        exempt: PrincipalSet::default(),
    }
}

#[test]
fn deny_beats_allow_regardless_of_order() {
    let allow = grant(
        Effect::Allow,
        &["analysts"],
        &[],
        scope_table("sales", "orders"),
    );
    let deny = grant(
        Effect::Deny,
        &["analysts"],
        &[],
        scope_table("sales", "orders"),
    );
    let subj = subject_in_group("analysts");

    // Both orders, because an evaluator that depends on grant order is a
    // Ranger first-match-wins in disguise, and D2 promises it is not.
    for grants in [
        vec![allow.clone(), deny.clone()],
        vec![deny.clone(), allow.clone()],
    ] {
        let d = evaluate(
            &subj,
            Action::Read,
            &scope_table("sales", "orders"),
            &grants,
            &[],
            &[],
            &[],
        );
        assert!(!d.allowed, "deny must win in either order");
        assert!(matches!(d.reasons[0], Reason::ExplicitDeny { .. }));
    }
}

#[test]
fn the_subtract_removes_a_member_of_an_included_group() {
    let g = grant(
        Effect::Allow,
        &["finance"],
        &["contractors"],
        scope_table("sales", "orders"),
    );
    let insider = Subject {
        user: "a".into(),
        groups: vec!["finance".into()],
        roles: vec![],
        workspaces: vec![],
    };
    let outsider = Subject {
        user: "b".into(),
        groups: vec!["finance".into(), "contractors".into()],
        roles: vec![],
        workspaces: vec![],
    };

    assert!(
        evaluate(
            &insider,
            Action::Read,
            &scope_table("sales", "orders"),
            std::slice::from_ref(&g),
            &[],
            &[],
            &[],
        )
        .allowed
    );
    let d = evaluate(
        &outsider,
        Action::Read,
        &scope_table("sales", "orders"),
        &[g],
        &[],
        &[],
        &[],
    );
    assert!(!d.allowed, "exclude must subtract from include");
    assert!(
        matches!(d.reasons[0], Reason::NoGrantMatched),
        "a subtracted principal matches NO grant — it is not an explicit deny, \
         and telling those apart is the whole point of the reason class"
    );
}

#[test]
fn no_grant_at_all_is_a_distinct_reason_from_an_explicit_deny() {
    let d = evaluate(
        &subject_in_group("nobody"),
        Action::Read,
        &scope_table("sales", "orders"),
        &[],
        &[],
        &[],
        &[],
    );
    assert!(!d.allowed);
    assert!(matches!(d.reasons[0], Reason::NoGrantMatched));
}

#[test]
fn a_mask_applies_even_when_access_came_from_a_wider_grant() {
    let g = grant(Effect::Allow, &["analysts"], &[], scope_schema("sales"));
    let r = mask_rule_on_tag("pii", "MASK_HASH", &[]);
    let d = evaluate(
        &subject_in_group("analysts"),
        Action::Read,
        &scope_column("sales", "orders", "email"),
        &[g],
        &[r],
        &["pii".into()],
        &[],
    );
    assert!(d.allowed, "the grant still allows the read");
    assert!(
        d.reasons
            .iter()
            .any(|x| matches!(x, Reason::MaskedByRule { .. })),
        "the mask must apply regardless of how access was granted (D2)"
    );
}

#[test]
fn an_exempt_principal_escapes_the_mask() {
    let g = grant(Effect::Allow, &["analysts"], &[], scope_schema("sales"));
    let r = mask_rule_on_tag("pii", "MASK_HASH", &["stewards"]);
    let subj = Subject {
        user: "s".into(),
        groups: vec!["analysts".into(), "stewards".into()],
        roles: vec![],
        workspaces: vec![],
    };
    let d = evaluate(
        &subj,
        Action::Read,
        &scope_column("sales", "orders", "email"),
        &[g],
        &[r],
        &["pii".into()],
        &[],
    );
    assert!(d.allowed);
    assert!(!d
        .reasons
        .iter()
        .any(|x| matches!(x, Reason::MaskedByRule { .. })));
}

#[test]
fn a_grant_on_a_schema_covers_a_table_beneath_it_but_not_a_sibling_schema() {
    let g = grant(Effect::Allow, &["analysts"], &[], scope_schema("sales"));
    let subj = subject_in_group("analysts");
    assert!(
        evaluate(
            &subj,
            Action::Read,
            &scope_table("sales", "orders"),
            std::slice::from_ref(&g),
            &[],
            &[],
            &[],
        )
        .allowed
    );
    assert!(
        !evaluate(
            &subj,
            Action::Read,
            &scope_table("hr", "salaries"),
            &[g],
            &[],
            &[],
            &[],
        )
        .allowed
    );
}

// --- Fix round 1 ---

#[test]
fn a_write_is_refused_by_a_read_only_grant() {
    // Review finding 3: every prior test hardcoded Action::Read on both the
    // grant and the call, so an evaluator that ignored `action` entirely
    // would have passed all six. This pins `applicable`'s
    // `g.actions.contains(&action)` term.
    let g = grant(
        Effect::Allow,
        &["analysts"],
        &[],
        scope_table("sales", "orders"),
    );
    assert_eq!(g.actions, vec![Action::Read]);
    let d = evaluate(
        &subject_in_group("analysts"),
        Action::Write,
        &scope_table("sales", "orders"),
        &[g],
        &[],
        &[],
        &[],
    );
    assert!(!d.allowed, "a read-only grant must not authorize a write");
    assert!(matches!(d.reasons[0], Reason::NoGrantMatched));
}

#[test]
fn a_grant_in_one_catalog_does_not_cover_a_target_in_another() {
    // Review finding 3: every prior test shared one `Uuid::nil()` catalog
    // id, so the `g.catalog_id != t.catalog_id` guard in `covers` was never
    // exercised. This is the pair mutation-checked below.
    let g = grant(
        Effect::Allow,
        &["analysts"],
        &[],
        scope_table("sales", "orders"),
    );
    let target = Scope {
        catalog_id: OTHER_CATALOG,
        ..scope_table("sales", "orders")
    };
    let d = evaluate(
        &subject_in_group("analysts"),
        Action::Read,
        &target,
        &[g],
        &[],
        &[],
        &[],
    );
    assert!(
        !d.allowed,
        "a grant scoped to one catalog must not cover a target in another"
    );
    assert!(matches!(d.reasons[0], Reason::NoGrantMatched));
}

#[test]
fn a_grant_scope_value_globs_like_ranger_not_only_the_literal_star() {
    // Review finding 1: Ranger globs (`*`, `?`), and migration 0154 puts no
    // CHECK restricting a scope column to a closed `*`-or-literal
    // vocabulary, so a stored value like "ord*" is possible today. The
    // evaluator must match it the way the engine does, or it under-matches
    // as an oracle — which silently blesses an over-permissive compiler.
    let g = grant(
        Effect::Allow,
        &["analysts"],
        &[],
        Scope {
            catalog_id: CATALOG,
            schema: Some("sales".into()),
            table: Some("ord*".into()),
            column: None,
        },
    );
    let subj = subject_in_group("analysts");
    assert!(
        evaluate(
            &subj,
            Action::Read,
            &scope_table("sales", "orders"),
            std::slice::from_ref(&g),
            &[],
            &[],
            &[],
        )
        .allowed,
        "\"ord*\" must glob-match \"orders\", the way the engine does"
    );
    assert!(
        !evaluate(
            &subj,
            Action::Read,
            &scope_table("sales", "customers"),
            &[g],
            &[],
            &[],
            &[],
        )
        .allowed,
        "\"ord*\" must not match a table it does not prefix"
    );
}

#[test]
fn a_tag_selector_never_hits_a_schema_level_target() {
    // Review finding 4: tags attach to tables and columns only (there is no
    // catalog- or schema-level tag), so a tag selector must not claim a hit
    // on a target that names no table, even if the caller mistakenly passes
    // tags for it.
    let g = grant(Effect::Allow, &["analysts"], &[], scope_schema("sales"));
    let r = mask_rule_on_tag("pii", "MASK_HASH", &[]);
    let d = evaluate(
        &subject_in_group("analysts"),
        Action::Read,
        &scope_schema("sales"),
        &[g],
        &[r],
        &["pii".into()],
        &[],
    );
    assert!(d.allowed);
    assert!(
        !d.reasons
            .iter()
            .any(|x| matches!(x, Reason::MaskedByRule { .. })),
        "a schema-level target cannot carry a tag, so no mask reason may appear"
    );
}

#[test]
fn a_path_selected_row_filter_applies_like_a_tag_selected_mask() {
    // Minor finding 5: RuleKind::RowFilter and Selector::Path were both
    // untested, so `FilteredByRule` and the path half of `selector_hits`
    // had zero coverage.
    let g = grant(Effect::Allow, &["analysts"], &[], scope_schema("sales"));
    let r = row_filter_rule_on_path(
        "orders_region_filter",
        scope_table("sales", "orders"),
        "region = current_user_region()",
    );
    let d = evaluate(
        &subject_in_group("analysts"),
        Action::Read,
        &scope_table("sales", "orders"),
        &[g],
        &[r],
        &[],
        &[],
    );
    assert!(d.allowed);
    assert!(
        d.reasons
            .iter()
            .any(|x| matches!(x, Reason::FilteredByRule { .. })),
        "a path-selected row filter covering the target must apply"
    );
}

/// SP4c Task 4/D4: a subject blocked by clearance must get its OWN reason,
/// not `no_grant_matched` (today's wrong answer — nobody needs to add a
/// grant, somebody needs clearance) and not `explicit_deny` either
/// (collapsing the two is how both would end up reading "denied", which
/// SP4b's five independent reason classes exist to prevent). An `allow`
/// grant that would otherwise let the subject through proves the clearance
/// check runs BEFORE grants, matching D1's tag-first short-circuit — a
/// clearance must win over a resource grant, not merely apply where none
/// exists.
#[test]
fn clearance_blocks_even_when_a_grant_would_otherwise_allow() {
    let g = grant(
        Effect::Allow,
        &["everyone"],
        &[],
        scope_table("sales", "orders"),
    );
    let gate = ClearanceGate {
        tag: "pii".into(),
        required_roles: vec!["analyst".into()],
    };
    let subject = subject_in_group("everyone");

    let d = evaluate(
        &subject,
        Action::Read,
        &scope_column("sales", "orders", "ssn"),
        &[g],
        &[],
        &["pii".into()],
        &[gate],
    );
    assert!(!d.allowed, "an uncleared subject must be refused: {d:?}");
    match d.reasons.as_slice() {
        [Reason::ClearanceRequired {
            tag,
            required_roles,
        }] => {
            assert_eq!(tag, "pii", "the reason must name the gating tag");
            assert_eq!(required_roles, &vec!["analyst".to_string()]);
        }
        other => panic!("expected ClearanceRequired alone, got {other:?}"),
    }
}

/// A subject holding ONE of several required roles is cleared — the "OR
/// across roles" semantics `egret_perm_compile::clearance` compiles (its
/// exception list is the union of every required role's members) — and, once
/// cleared, evaluation proceeds to the grants exactly as if no gate existed.
#[test]
fn a_subject_holding_any_required_role_is_cleared_and_grants_still_apply() {
    let g = grant(
        Effect::Allow,
        &["everyone"],
        &[],
        scope_table("sales", "orders"),
    );
    let gate = ClearanceGate {
        tag: "pii".into(),
        required_roles: vec!["analyst".into(), "auditor".into()],
    };
    let mut subject = subject_in_group("everyone");
    subject.roles = vec!["auditor".into()];

    let d = evaluate(
        &subject,
        Action::Read,
        &scope_column("sales", "orders", "ssn"),
        &[g],
        &[],
        &["pii".into()],
        &[gate],
    );
    assert!(
        d.allowed,
        "a subject holding any required role is cleared: {d:?}"
    );
    // What must not appear is `ClearanceRequired`.
    assert!(
        matches!(d.reasons.as_slice(), [Reason::AllowedByGrant { .. }]),
        "the clearance must not be the reason for a subject who holds a required role: {d:?}"
    );
}

/// A gate on a tag NOT present on the target must not fire — `evaluate` only
/// consults `tags_on`, exactly as a rule's tag selector does, never a gate's
/// mere presence in the caller-supplied list.
#[test]
fn a_clearance_gate_for_an_absent_tag_does_not_block() {
    let g = grant(
        Effect::Allow,
        &["everyone"],
        &[],
        scope_table("sales", "orders"),
    );
    let gate = ClearanceGate {
        tag: "financial".into(),
        required_roles: vec!["analyst".into()],
    };
    let d = evaluate(
        &subject_in_group("everyone"),
        Action::Read,
        &scope_table("sales", "orders"),
        &[g],
        &[],
        &[], // no tags on this target at all
        &[gate],
    );
    assert!(
        d.allowed,
        "a clearance gate for a tag the target does not carry must not block: {d:?}"
    );
}

// ---------------------------------------------------------------------------
// A group-matched allow is an ordinary allow (J-F3 Task 1b)
// ---------------------------------------------------------------------------

/// **SP4c review, retired by J-F3 Task 1b.** A Ranger item naming a group
/// matches nobody on any engine egret provisions (D7), so a subject who
/// reached a grant only through a group used to be allowed here and refused by
/// both engines, and this evaluator reported it under its own class
/// (`allowed_by_group_only`). Design R8 closed the divergence on the
/// compiler's side: a non-`public` group now compiles to its members'
/// usernames, so the engine allows exactly whom this function allows, and the
/// warning class would now be the lie. `egret-perm-compile/tests/routes.rs`
/// (`a_group_grant_reaches_its_members_in_the_artifact_and_in_explain`) asserts
/// the engine side.
#[test]
fn a_grant_matched_only_through_a_group_is_an_ordinary_allow() {
    let g = grant(
        Effect::Allow,
        &["contractors"],
        &[],
        scope_table("sales", "orders"),
    );
    let d = evaluate(
        &subject_in_group("contractors"),
        Action::Read,
        &scope_table("sales", "orders"),
        &[g],
        &[],
        &[],
        &[],
    );
    assert!(d.allowed, "{d:?}");
    assert!(
        matches!(d.reasons.as_slice(), [Reason::AllowedByGrant { .. }]),
        "{d:?}"
    );
}

/// `public` — Ranger's built-in "everybody", the one group the compiler never
/// expands — is matched like any group and is an ordinary allow. (Written as
/// half of the discriminating pair for the retired `allowed_by_group_only`
/// class; kept because it still pins how `public` is matched.)
#[test]
fn a_grant_naming_the_public_group_is_enforceable_and_not_group_only() {
    let g = grant(
        Effect::Allow,
        &[egret_perm::PUBLIC_GROUP],
        &[],
        scope_table("sales", "orders"),
    );
    let d = evaluate(
        &subject_in_group(egret_perm::PUBLIC_GROUP),
        Action::Read,
        &scope_table("sales", "orders"),
        &[g],
        &[],
        &[],
        &[],
    );
    assert!(d.allowed, "{d:?}");
    assert!(
        matches!(d.reasons.as_slice(), [Reason::AllowedByGrant { .. }]),
        "{d:?}"
    );
}

/// A grant naming the subject's own USERNAME as well as a group is an ordinary
/// allow. (The other half of the retired class's discriminating pair.)
#[test]
fn a_grant_naming_the_user_as_well_as_a_group_is_not_group_only() {
    let mut g = grant(
        Effect::Allow,
        &["contractors"],
        &[],
        scope_table("sales", "orders"),
    );
    g.principals.include.push(Principal {
        kind: PrincipalKind::User,
        r#ref: "alice".into(),
    });
    let mut subject = subject_in_group("contractors");
    subject.user = "alice".into();

    let d = evaluate(
        &subject,
        Action::Read,
        &scope_table("sales", "orders"),
        &[g],
        &[],
        &[],
        &[],
    );
    assert!(
        matches!(d.reasons.as_slice(), [Reason::AllowedByGrant { .. }]),
        "{d:?}"
    );
}
