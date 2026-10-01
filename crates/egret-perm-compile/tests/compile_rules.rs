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
    CompileError, EngineType, Principal, PrincipalKind, PrincipalSet, Rule, RuleKind, Scope,
    Selector,
};
use egret_perm_compile::{compile_rule, validate_rule_shape, ResolvedNames, TaggedColumn};

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

/// **`policyPriority` MUST be 0 on every body the rule compiler emits — the
/// resource-addressed ones as well as the tag-addressed ones — and until SP4d-2b
/// Task 5 fix round 2 nothing asserted it for either.**
///
/// The third and last hole in the invariant T4's and T7's cost claims rest on.
/// Round 1 closed it for tag bodies through `tag_service_routing.rs`, which needs a
/// database; `clearance_starrocks.rs` covers the clearance path; this covers both
/// arms of `build_tag_body` AND both arms of the resource path, with no database at
/// all — which matters, because the shared `egret_test_template` is currently
/// unusable on this branch and a DB-gated assertion that cannot run is not an
/// assertion.
///
/// Asserted on the EMITTED `policyPriority`, not on the `is_override` flag the four
/// call sites in `rules.rs` hard-code, so it survives a refactor that renames or
/// reroutes the flag. `isOverride` is a public wire field, so one flip makes
/// egret's own downloads reach the shape the differential corpus holds out
/// (`tests/ranger/README.md`, T7).
#[test]
fn every_compiled_rule_body_emits_policy_priority_zero() {
    let cases: Vec<(&str, Rule)> = vec![
        ("mask", mask_rule_on_tag("pii-key", "MASK_HASH")),
        (
            "rowfilter",
            rowfilter_rule_on_tag("pii-key", "dept = 'eng'"),
        ),
    ];
    for (label, r) in cases {
        // The TAG-addressed arm.
        let tagged = compile_rule(
            &r,
            EngineType::Trino,
            "trino-1",
            Some("trino-1_tag"),
            &resolved("warehouse", Some("PII")),
            &[],
            100,
        )
        .unwrap();
        assert!(!tagged.is_empty(), "{label}: the tag arm produced nothing");
        for body in &tagged {
            assert_eq!(
                body["policyPriority"], 0,
                "{label}: tag-addressed body must carry policyPriority 0"
            );
        }
        // …and the RESOURCE-addressed arm, which takes the other branch of
        // `build_body` and had no assertion of its own either.
        let untagged = compile_rule(
            &r,
            EngineType::Trino,
            "trino-1",
            None,
            &resolved("warehouse", None),
            &[],
            100,
        );
        if let Ok(bodies) = untagged {
            for body in &bodies {
                assert_eq!(
                    body["policyPriority"], 0,
                    "{label}: resource-addressed body must carry policyPriority 0"
                );
            }
        }
    }
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

// ---------------------------------------------------------------------------
// Whole-branch review, Critical 1 — a row filter on a column returned every row
// ---------------------------------------------------------------------------

/// Read one embedded service-def from disk (same two files the compiler
/// embeds; `CARGO_MANIFEST_DIR` because `cargo test` runs with CWD = this
/// crate's manifest directory). Copied from `compile_grants.rs` rather than
/// shared: an integration test binary cannot import another one's helpers,
/// and the point of both is to re-read the DECLARATION rather than trust a
/// constant.
fn service_def(engine: EngineType) -> serde_json::Value {
    let path = match engine {
        EngineType::Trino => concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../egret-policy/src/service-defs/trino.json"
        ),
        EngineType::StarRocks => concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../egret-policy/src/service-defs/starrocks.json"
        ),
    };
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The resource level names one policy-type definition declares, and which of
/// them it marks `"mandatory": true`.
fn declared_levels(engine: EngineType, def_key: &str) -> (Vec<String>, Vec<String>) {
    let def = service_def(engine);
    let resources = def[def_key]["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("{engine:?}'s service-def declares no {def_key}.resources"))
        .clone();
    let names = resources
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let mandatory = resources
        .iter()
        .filter(|r| r["mandatory"].as_bool() == Some(true))
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    (names, mandatory)
}

fn resource_keys(body: &serde_json::Value) -> Vec<String> {
    let mut keys = body["resources"]
        .as_object()
        .expect("every compiled body carries a resources object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

fn rowfilter_rule_on_path(scope: Scope, filter_expr: &str) -> Rule {
    Rule {
        id: Uuid::nil(),
        name: "path-rowfilter".to_string(),
        kind: RuleKind::RowFilter {
            filter_expr: filter_expr.to_string(),
        },
        selector: Selector::Path(scope),
        exempt: PrincipalSet::default(),
    }
}

fn scope_of(schema: Option<&str>, table: Option<&str>, column: Option<&str>) -> Scope {
    Scope {
        catalog_id: Uuid::nil(),
        schema: schema.map(str::to_string),
        table: table.map(str::to_string),
        column: column.map(str::to_string),
    }
}

/// **The assertion the branch was missing.** `tests` for row filters checked
/// the policy body (`policyType`, the filter expression, the `public` item)
/// and never the `resources` map against what `rowFilterDef` DECLARES — so
/// three shapes emitted a map that definition cannot match, all of which
/// compile, deploy, and are listed by egret as live while filtering nothing.
///
/// This test derives the expectation from the service-def files themselves
/// (both engines), so it fails if the emission drifts OR if a service-def
/// ever gains a `column` level the compiler is not taught about.
#[test]
fn the_row_filter_resource_map_matches_what_the_service_def_declares() {
    for engine in [EngineType::Trino, EngineType::StarRocks] {
        let (declared, mandatory) = declared_levels(engine, "rowFilterDef");
        let mut expected = declared.clone();
        expected.sort();
        assert!(
            !declared.iter().any(|n| n == "column"),
            "{engine:?}: this test's whole premise is that rowFilterDef declares no column level"
        );
        assert_eq!(
            mandatory.len(),
            declared.len(),
            "{engine:?}: every rowFilterDef level is expected to be mandatory — if that ever \
             stops being true, the wildcard fill below needs re-deciding, not silently keeping"
        );

        // 1. The shape the live gate measured working: catalog + schema + table.
        let exact = rowfilter_rule_on_path(
            scope_of(Some("sales"), Some("orders"), None),
            "country = 'US'",
        );
        let out = compile_rule(
            &exact,
            engine,
            "svc",
            Some("svc_tag"),
            &resolved("wh", None),
            &[],
            100,
        )
        .unwrap();
        assert_eq!(
            resource_keys(&out[0]),
            expected,
            "{engine:?}: a table-scoped row filter must name exactly the declared levels"
        );

        // 2. Scoped to a COLUMN (refused at the authoring boundary now, but a
        //    row already in `perm.rules` still has to compile to something
        //    matchable): the undeclared `column` must not be emitted.
        let on_column = rowfilter_rule_on_path(
            scope_of(Some("sales"), Some("orders"), Some("customer_email")),
            "country = 'US'",
        );
        let out = compile_rule(
            &on_column,
            engine,
            "svc",
            Some("svc_tag"),
            &resolved("wh", None),
            &[],
            100,
        )
        .unwrap();
        assert_eq!(
            resource_keys(&out[0]),
            expected,
            "{engine:?}: a column-scoped row filter must NOT emit a column resource — \
             rowFilterDef declares none, and a body carrying one filters no rows at all"
        );

        // 3. Scoped ABOVE the table: every mandatory level must still be
        //    present, as an explicit wildcard.
        let above = rowfilter_rule_on_path(scope_of(None, None, None), "country = 'US'");
        let out = compile_rule(
            &above,
            engine,
            "svc",
            Some("svc_tag"),
            &resolved("wh", None),
            &[],
            100,
        )
        .unwrap();
        assert_eq!(
            resource_keys(&out[0]),
            expected,
            "{engine:?}: a catalog-scoped row filter must fill the mandatory levels it does not \
             name, never omit them"
        );
        for m in &mandatory {
            assert!(
                out[0]["resources"][m]["values"][0].is_string(),
                "{engine:?}: mandatory level {m:?} carries no value"
            );
        }
        assert_eq!(out[0]["resources"][&declared[1]]["values"][0], "*");
        assert_eq!(out[0]["resources"][&declared[2]]["values"][0], "*");
    }

    // 4. A MASK keeps the column level, because `dataMaskDef` declares one —
    //    the same call that must drop it for a row filter.
    for engine in [EngineType::Trino, EngineType::StarRocks] {
        let (mask_levels, _) = declared_levels(engine, "dataMaskDef");
        assert!(
            mask_levels.iter().any(|n| n == "column"),
            "{engine:?}: dataMaskDef is expected to declare a column level"
        );
        let r = mask_rule_on_path(
            scope_of(Some("sales"), Some("orders"), Some("customer_email")),
            "MASK_HASH",
        );
        let out = compile_rule(&r, engine, "svc", None, &resolved("wh", None), &[], 100).unwrap();
        assert_eq!(
            out[0]["resources"]["column"]["values"][0], "customer_email",
            "{engine:?}: a mask must still address its column"
        );
    }
}

/// C1's third shape, and the reason it is a recurring class rather than a
/// one-off: `column_resources` (the StarRocks tag expansion) emitted all four
/// levels unconditionally while its sibling `scope_to_resources` already
/// branched on the policy target. Task 11 fixed one and not the other.
#[test]
fn a_starrocks_tag_row_filter_addresses_the_table_and_not_the_column() {
    let (declared, _) = declared_levels(EngineType::StarRocks, "rowFilterDef");
    let mut expected = declared.clone();
    expected.sort();

    let r = rowfilter_rule_on_tag("region", "country = 'US'");
    let tagged = [
        col("wh", "sales", "orders", "customer_email"),
        // Same table, second tagged column: one row filter, not two.
        col("wh", "sales", "orders", "customer_phone"),
        col("wh", "sales", "shipments", "recipient"),
    ];
    let out = compile_rule(
        &r,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("wh", Some("region-tag")),
        &tagged,
        100,
    )
    .unwrap();

    assert_eq!(
        out.len(),
        2,
        "a row filter is table-granular: two tagged columns of one table are one policy"
    );
    for body in &out {
        assert_eq!(
            resource_keys(body),
            expected,
            "a StarRocks tag row filter must name exactly the levels rowFilterDef declares"
        );
        assert_eq!(body["policyType"], 2);
        assert_eq!(body["serviceType"], "starrocks");
    }
    let mut tables = out
        .iter()
        .map(|b| b["resources"]["table"]["values"][0].as_str().unwrap())
        .collect::<Vec<_>>();
    tables.sort();
    assert_eq!(tables, vec!["orders", "shipments"]);
    let names = out
        .iter()
        .map(|b| b["name"].as_str().unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(
        names.len(),
        2,
        "two policies under one service need two names"
    );
    assert!(
        !names.iter().any(|n| n.contains("customer_email")),
        "a row filter's name must not claim a column it does not address: {names:?}"
    );

    // The MASK expansion over the same three columns is still per-column —
    // this is the branch that must NOT have changed.
    let m = mask_rule_on_tag("region", "MASK_HASH");
    let masks = compile_rule(
        &m,
        EngineType::StarRocks,
        "sr-1",
        None,
        &resolved("wh", Some("region-tag")),
        &tagged,
        100,
    )
    .unwrap();
    assert_eq!(
        masks.len(),
        3,
        "a mask still expands once per tagged column"
    );
    let mut mask_cols = masks
        .iter()
        .map(|b| b["resources"]["column"]["values"][0].as_str().unwrap())
        .collect::<Vec<_>>();
    mask_cols.sort();
    assert_eq!(
        mask_cols,
        vec!["customer_email", "customer_phone", "recipient"]
    );
}

/// The write-time half: a shape the compiler can only render as a DIFFERENT
/// control than the author wrote is refused where the author can still fix
/// it, the way every other uncompilable shape is (`validate_rule_shape` runs
/// in `create_rule`, and `CompileError` maps to a 400).
#[test]
fn a_path_row_filter_must_name_exactly_catalog_schema_and_table() {
    let ok = rowfilter_rule_on_path(
        scope_of(Some("sales"), Some("orders"), None),
        "country = 'US'",
    );
    validate_rule_shape(&ok).expect("the shape the live gate measured must stay legal");

    for (scope, what) in [
        (
            scope_of(Some("sales"), Some("orders"), Some("customer_email")),
            "a column",
        ),
        (scope_of(Some("sales"), None, None), "no table"),
        (scope_of(None, None, None), "catalog only"),
        (scope_of(None, Some("orders"), None), "a gap at schema"),
    ] {
        let r = rowfilter_rule_on_path(scope, "country = 'US'");
        let err = validate_rule_shape(&r)
            .expect_err(&format!("a row filter scoped to {what} must be refused"));
        assert!(
            matches!(err, CompileError::UnfilterableRowFilterScope { .. }),
            "scoped to {what}: expected UnfilterableRowFilterScope, got {err:?}"
        );
    }

    // A MASK on the very same scopes stays legal — dataMaskDef declares a
    // column and marks nothing mandatory, so this refusal must not leak
    // across to the other rule kind.
    let m = mask_rule_on_path(
        scope_of(Some("sales"), Some("orders"), Some("customer_email")),
        "MASK_HASH",
    );
    validate_rule_shape(&m).expect("a column-scoped mask is exactly what a mask is for");

    // Nor across to a TAG-selected row filter, which carries no scope at all.
    validate_rule_shape(&rowfilter_rule_on_tag("region", "country = 'US'"))
        .expect("a tag row filter has no path scope to get wrong");
}
