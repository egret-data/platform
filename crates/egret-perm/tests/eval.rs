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
// catalog id; no test here compares across catalogs.
const CATALOG: Uuid = Uuid::nil();

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
