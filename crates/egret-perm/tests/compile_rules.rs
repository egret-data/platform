//! D5: the same rule compiles compactly on Trino and expansively on StarRocks,
//! because StarRocks' Ranger plugin has no tag service (upstream
//! starrocks#67458). The asymmetry is the design; the ceiling is the guard.
//!
//! Round-1 review pinned two Criticals that count-only assertions let
//! through: a tag policy naming `tags.key` (the plugin matches `tags.name`)
//! and a path policy naming a catalog UUID (engines match by name). Both
//! deploy clean, are listed by Ranger, and match nothing — "right count,
//! wrong value". Every shape below that produces a policy body now asserts
//! into that body (`resources`, `policyType`, `serviceType`, the item
//! contents), not just `.len()`.
//!
//! Pure (no DB) — `compile_rule` is a function of its arguments only.

use std::collections::HashSet;

use uuid::Uuid;

use egret_perm::{
    compile_rule, CompileError, EngineType, Principal, PrincipalKind, PrincipalSet, ResolvedNames,
    Rule, RuleKind, Scope, Selector, TaggedColumn,
};

// Dumb helpers only — a helper that computes anything is a second
// implementation of the thing under test.

fn mask_rule_on_tag(tag: &str, mask_type: &str) -> Rule {
    Rule {
        id: Uuid::nil(),
        name: format!("{tag}-hash"),
        kind: RuleKind::Mask {
            mask_type: mask_type.to_string(),
            value_expr: None,
        },
        selector: Selector::Tag {
            key: tag.to_string(),
        },
        exempt: PrincipalSet::default(),
    }
}

fn mask_rule_custom(tag: &str, expr: &str) -> Rule {
    Rule {
        id: Uuid::nil(),
        name: format!("{tag}-custom"),
        kind: RuleKind::Mask {
            mask_type: "CUSTOM".to_string(),
            value_expr: Some(expr.to_string()),
        },
        selector: Selector::Tag {
            key: tag.to_string(),
        },
        exempt: PrincipalSet::default(),
    }
}

fn mask_rule_on_path(scope: Scope, mask_type: &str) -> Rule {
    Rule {
        id: Uuid::nil(),
        name: "path-mask".to_string(),
        kind: RuleKind::Mask {
            mask_type: mask_type.to_string(),
            value_expr: None,
        },
        selector: Selector::Path(scope),
        exempt: PrincipalSet::default(),
    }
}

fn rowfilter_rule_on_tag(tag: &str, filter_expr: &str) -> Rule {
    Rule {
        id: Uuid::nil(),
        name: format!("{tag}-rowfilter"),
        kind: RuleKind::RowFilter {
            filter_expr: filter_expr.to_string(),
        },
        selector: Selector::Tag {
            key: tag.to_string(),
        },
        exempt: PrincipalSet::default(),
    }
}

fn col(catalog: &str, schema: &str, table: &str, column: &str) -> TaggedColumn {
    TaggedColumn {
        catalog: catalog.to_string(),
        schema: schema.to_string(),
        table: table.to_string(),
        column: column.to_string(),
    }
}

fn user(r#ref: &str) -> Principal {
    Principal {
        kind: PrincipalKind::User,
        r#ref: r#ref.to_string(),
    }
}

fn group(r#ref: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Group,
        r#ref: r#ref.to_string(),
    }
}

fn resolved<'a>(catalog_name: &'a str, tag_name: Option<&'a str>) -> ResolvedNames<'a> {
    ResolvedNames {
        catalog_name,
        tag_name,
    }
}

#[test]
fn a_tag_rule_is_one_policy_on_trino_however_many_columns_carry_the_tag() {
    let r = mask_rule_on_tag("pii", "MASK_HASH");
    let tagged = (0..50)
        .map(|i| col("wh", "sales", "orders", &format!("c{i}")))
        .collect::<Vec<_>>();
    let out = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("wh", Some("pii")),
        &tagged,
        100,
    )
    .unwrap();
    assert_eq!(
        out.len(),
        1,
        "Trino's plugin resolves the tag itself — one policy covers all 50"
    );
}

#[test]
fn the_same_rule_becomes_one_policy_per_column_on_starrocks() {
    let r = mask_rule_on_tag("pii", "MASK_HASH");
    let tagged = (0..50)
        .map(|i| col("wh", "sales", "orders", &format!("c{i}")))
        .collect::<Vec<_>>();
    let out = compile_rule(
        &r,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("wh", Some("pii")),
        &tagged,
        100,
    )
    .unwrap();
    assert_eq!(
        out.len(),
        50,
        "no tag service — the tag must be enumerated at compile time"
    );
}

#[test]
fn the_ceiling_refuses_and_names_the_rule_and_the_count() {
    let r = mask_rule_on_tag("pii", "MASK_HASH");
    let tagged = (0..101)
        .map(|i| col("wh", "sales", "orders", &format!("c{i}")))
        .collect::<Vec<_>>();
    let err = compile_rule(
        &r,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("wh", Some("pii")),
        &tagged,
        100,
    )
    .unwrap_err();
    match err {
        CompileError::ExpansionCeiling {
            rule,
            count,
            ceiling,
        } => {
            assert_eq!(rule, "pii-hash");
            assert_eq!(count, 101);
            assert_eq!(ceiling, 100);
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn the_ceiling_refuses_rather_than_truncating() {
    // The failure this guards is silent partial enforcement: 100 of 101
    // columns masked looks exactly like success from every screen egret has.
    let r = mask_rule_on_tag("pii", "MASK_HASH");
    let tagged = (0..101)
        .map(|i| col("wh", "sales", "orders", &format!("c{i}")))
        .collect::<Vec<_>>();
    assert!(compile_rule(
        &r,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("wh", Some("pii")),
        &tagged,
        100
    )
    .is_err());
    // And one under the ceiling still compiles, so the test above is not
    // passing merely because compilation is broken.
    let ok = compile_rule(
        &r,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("wh", Some("pii")),
        &tagged[..100],
        100,
    )
    .unwrap();
    assert_eq!(ok.len(), 100);
}

#[test]
fn a_custom_mask_expression_that_does_not_parse_is_refused_at_compile_time() {
    // Ranger never parses this string; it fails inside the engine at query
    // time. Refusing here is the difference between an author finding out now
    // and a user finding out from a broken dashboard. Pinned to the SPECIFIC
    // variant (round-1 review, Important 4) — `is_err()` alone would also
    // pass if some unrelated check fired first.
    let r = mask_rule_custom("pii", "initcap(reverse({col})"); // unbalanced paren
    let err = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("wh", Some("pii")),
        &[],
        100,
    )
    .unwrap_err();
    assert_eq!(
        err,
        CompileError::InvalidCustomMask {
            rule: "pii-custom".to_string(),
            expr: "initcap(reverse({col})".to_string(),
        }
    );
}

#[test]
fn the_tag_policy_carries_the_resolved_tag_name_not_the_selectors_key() {
    // Critical 1 from the round-1 review: the first `compile_rule` had no
    // resolved-name parameter at all, so it stamped `Selector::Tag`'s `key`
    // straight into the policy. egret's own tag download pushes
    // `tags.name` — a separate, mutable column — so a policy naming the key
    // deploys clean, is listed by Ranger, and matches no resource.
    let r = mask_rule_on_tag("pii-key", "MASK_HASH");
    let out = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("warehouse", Some("PII")),
        &[],
        100,
    )
    .unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0]["resources"]["tag"]["values"][0], "PII",
        "must stamp the RESOLVED tag name, never the selector's key"
    );
    assert_ne!(out[0]["resources"]["tag"]["values"][0], "pii-key");
    assert_eq!(out[0]["policyType"], 1);
    assert_eq!(out[0]["serviceType"], "tag");
}

#[test]
fn starrocks_expanded_policies_each_name_their_own_column_and_are_all_distinct() {
    // Important 3 from the round-1 review: a count of 3 would also pass if
    // all three policies named the SAME column. Assert the actual paths.
    let r = mask_rule_on_tag("pii", "MASK_HASH");
    let tagged = vec![
        col("wh", "sales", "orders", "email"),
        col("wh", "sales", "orders", "phone"),
        col("wh", "sales", "customers", "email"),
    ];
    let out = compile_rule(
        &r,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("wh", Some("pii")),
        &tagged,
        100,
    )
    .unwrap();
    assert_eq!(out.len(), 3);
    let paths: HashSet<String> = out
        .iter()
        .map(|body| {
            format!(
                "{}.{}.{}.{}",
                body["resources"]["catalog"]["values"][0].as_str().unwrap(),
                body["resources"]["database"]["values"][0].as_str().unwrap(),
                body["resources"]["table"]["values"][0].as_str().unwrap(),
                body["resources"]["column"]["values"][0].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        paths.len(),
        3,
        "each policy must name its OWN column, not all the same one"
    );
    assert!(paths.contains("wh.sales.orders.email"));
    assert!(paths.contains("wh.sales.orders.phone"));
    assert!(paths.contains("wh.sales.customers.email"));
}

#[test]
fn a_path_rule_uses_the_resolved_catalog_name_not_the_uuid() {
    // Critical 2 from the round-1 review: the compiler used to stamp
    // `scope.catalog_id.to_string()` — the raw UUID — as the catalog
    // resource. Engines match catalogs by NAME, never by id.
    let r = mask_rule_on_path(
        Scope {
            catalog_id: Uuid::new_v4(),
            schema: Some("sales".into()),
            table: Some("orders".into()),
            column: Some("email".into()),
        },
        "MASK_HASH",
    );
    let out = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        None,
        &resolved("warehouse", None),
        &[],
        100,
    )
    .unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["resources"]["catalog"]["values"][0], "warehouse");
    assert_eq!(out[0]["resources"]["schema"]["values"][0], "sales");
    assert_eq!(out[0]["resources"]["table"]["values"][0], "orders");
    assert_eq!(out[0]["resources"]["column"]["values"][0], "email");
}

#[test]
fn a_starrocks_path_rule_scopes_by_database_not_schema() {
    let r = mask_rule_on_path(
        Scope {
            catalog_id: Uuid::new_v4(),
            schema: Some("sales".into()),
            table: Some("orders".into()),
            column: None,
        },
        "MASK_HASH",
    );
    let out = compile_rule(
        &r,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("warehouse", None),
        &[],
        100,
    )
    .unwrap();
    assert_eq!(
        out[0]["resources"]["database"]["values"][0], "sales",
        "StarRocks's service-def calls the schema-level resource `database`"
    );
    assert!(!out[0]["resources"]
        .as_object()
        .unwrap()
        .contains_key("schema"));
}

#[test]
fn a_row_filter_tag_rule_renders_policy_type_2_with_the_filter_expression() {
    let r = rowfilter_rule_on_tag("region", "country = 'US'");
    let out = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("warehouse", Some("region-tag")),
        &[],
        100,
    )
    .unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["policyType"], 2);
    assert_eq!(out[0]["resources"]["tag"]["values"][0], "region-tag");
    let items = out[0]["rowFilterPolicyItems"].as_array().unwrap();
    let last = items.last().unwrap();
    assert_eq!(last["rowFilterInfo"]["filterExpr"], "country = 'US'");
    assert_eq!(last["groups"][0], "public");
}

#[test]
fn service_type_defaults_to_trino_and_is_stamped_starrocks_for_starrocks() {
    let r = mask_rule_on_path(
        Scope {
            catalog_id: Uuid::new_v4(),
            schema: None,
            table: None,
            column: None,
        },
        "MASK_HASH",
    );
    let trino_out = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        None,
        &resolved("warehouse", None),
        &[],
        100,
    )
    .unwrap();
    assert_eq!(trino_out[0]["serviceType"], "trino");

    let sr_out = compile_rule(
        &r,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("warehouse", None),
        &[],
        100,
    )
    .unwrap();
    assert_eq!(
        sr_out[0]["serviceType"], "starrocks",
        "masking_to_ranger_json hardcodes \"trino\" — the compiler must overwrite it for StarRocks"
    );
}

#[test]
fn an_unsupported_mask_type_is_refused_on_starrocks_but_not_trino() {
    // Trino's service-def declares MASK_SHOW_LAST_4; StarRocks's does not.
    let r = Rule {
        id: Uuid::nil(),
        name: "pii-show-last-4".to_string(),
        kind: RuleKind::Mask {
            mask_type: "MASK_SHOW_LAST_4".to_string(),
            value_expr: None,
        },
        selector: Selector::Tag {
            key: "pii".to_string(),
        },
        exempt: PrincipalSet::default(),
    };
    assert!(compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("wh", Some("pii")),
        &[],
        100
    )
    .is_ok());

    let err = compile_rule(
        &r,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("wh", Some("pii")),
        &[],
        100,
    )
    .unwrap_err();
    assert_eq!(
        err,
        CompileError::UnsupportedMaskType {
            rule: "pii-show-last-4".to_string(),
            mask_type: "MASK_SHOW_LAST_4".to_string(),
            engine: EngineType::StarRocks,
        }
    );
}

#[test]
fn an_exempt_user_gets_a_mask_none_item_ahead_of_the_public_item() {
    let mut r = mask_rule_on_tag("pii", "MASK_HASH");
    r.exempt = PrincipalSet {
        include: vec![user("alice")],
        exclude: vec![],
    };
    let out = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("wh", Some("pii")),
        &[],
        100,
    )
    .unwrap();
    let items = out[0]["dataMaskPolicyItems"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["users"][0], "alice");
    assert_eq!(items[0]["dataMaskInfo"]["dataMaskType"], "MASK_NONE");
    assert_eq!(items[1]["groups"][0], "public");
    assert_eq!(items[1]["dataMaskInfo"]["dataMaskType"], "MASK_HASH");
}

#[test]
fn an_exempt_group_named_public_is_refused() {
    // The one fail-open the exempt design would otherwise have: an exempt
    // group literally named "public" would exempt everyone, since that's
    // the same name the compiler's own "applies to everyone" item uses.
    let mut r = mask_rule_on_tag("pii", "MASK_HASH");
    r.exempt = PrincipalSet {
        include: vec![group("public")],
        exclude: vec![],
    };
    let err = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("wh", Some("pii")),
        &[],
        100,
    )
    .unwrap_err();
    match err {
        CompileError::UnexpressibleExemption { rule, .. } => assert_eq!(rule, "pii-hash"),
        other => panic!("expected UnexpressibleExemption, got {other:?}"),
    }
}

#[test]
fn exempt_exclude_is_refused_not_silently_dropped() {
    // Rule.exempt is a full PrincipalSet (model.rs, spec D1), but a
    // masking/row-filter Ranger item has no exception block to subtract
    // from — unlike compile_grant, which puts `exclude` in allowExceptions.
    let mut r = mask_rule_on_tag("pii", "MASK_HASH");
    r.exempt = PrincipalSet {
        include: vec![group("finance")],
        exclude: vec![user("bob")],
    };
    let err = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("wh", Some("pii")),
        &[],
        100,
    )
    .unwrap_err();
    assert!(matches!(err, CompileError::UnexpressibleExemption { .. }));
}

#[test]
fn a_role_principal_in_exempt_is_refused() {
    // MaskItemInput/RowFilterItemInput have no `roles` bucket at all,
    // unlike AccessItemInput.
    let mut r = mask_rule_on_tag("pii", "MASK_HASH");
    r.exempt = PrincipalSet {
        include: vec![Principal {
            kind: PrincipalKind::Role,
            r#ref: "analyst_role".to_string(),
        }],
        exclude: vec![],
    };
    let err = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("wh", Some("pii")),
        &[],
        100,
    )
    .unwrap_err();
    assert!(matches!(err, CompileError::UnexpressibleExemption { .. }));
}

#[test]
fn a_workspace_members_principal_in_exempt_is_refused() {
    let mut r = mask_rule_on_tag("pii", "MASK_HASH");
    r.exempt = PrincipalSet {
        include: vec![Principal {
            kind: PrincipalKind::WorkspaceMembers,
            r#ref: Uuid::nil().to_string(),
        }],
        exclude: vec![],
    };
    let err = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("wh", Some("pii")),
        &[],
        100,
    )
    .unwrap_err();
    assert_eq!(
        err,
        CompileError::UnexpandedPrincipalKind {
            kind: PrincipalKind::WorkspaceMembers
        }
    );
}

#[test]
fn a_tag_rule_with_no_resolved_tag_name_is_refused() {
    // Critical 1's guard: a caller that didn't resolve the tag name must
    // refuse, never fall back to the selector's key.
    let r = mask_rule_on_tag("pii", "MASK_HASH");

    let err = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &resolved("wh", None),
        &[],
        100,
    )
    .unwrap_err();
    assert_eq!(
        err,
        CompileError::MissingTagName {
            rule: "pii-hash".to_string()
        }
    );

    // StarRocks refuses too, even though its own compiled body never uses
    // the tag name directly — an unresolved name is a caller-invariant
    // violation regardless of which engine's output would have needed it.
    let err = compile_rule(
        &r,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("wh", None),
        &[],
        100,
    )
    .unwrap_err();
    assert_eq!(
        err,
        CompileError::MissingTagName {
            rule: "pii-hash".to_string()
        }
    );
}

#[test]
fn a_trino_tag_rule_with_no_tag_service_is_refused() {
    let r = mask_rule_on_tag("pii", "MASK_HASH");
    let err = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        None,
        &resolved("wh", Some("pii")),
        &[],
        100,
    )
    .unwrap_err();
    assert_eq!(
        err,
        CompileError::MissingTagService {
            rule: "pii-hash".to_string()
        }
    );
}
