//! D2: you can see it if some grant allows it, unless some grant denies it;
//! masks and row filters always apply, however the access was granted.
//!
//! Pure (no DB) — the evaluator is a function of its arguments only.

use egret_perm::{
    evaluate, Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Reason,
    Rule, RuleKind, Scope, Selector, Subject,
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
            &[g.clone()],
            &[],
            &[]
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
            &[g.clone()],
            &[],
            &[]
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
            &[]
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
            &[g.clone()],
            &[],
            &[]
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
            &[]
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
    );
    assert!(d.allowed);
    assert!(
        d.reasons
            .iter()
            .any(|x| matches!(x, Reason::FilteredByRule { .. })),
        "a path-selected row filter covering the target must apply"
    );
}
