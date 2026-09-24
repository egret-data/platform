//! D5: the same rule compiles compactly on Trino and expansively on StarRocks,
//! because StarRocks' Ranger plugin has no tag service (upstream
//! starrocks#67458). The asymmetry is the design; the ceiling is the guard.
//!
//! Pure (no DB) — `compile_rule` is a function of its arguments only.

use uuid::Uuid;

use egret_perm::{
    compile_rule, CompileError, EngineType, PrincipalSet, Rule, RuleKind, Selector, TaggedColumn,
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

fn col(catalog: &str, schema: &str, table: &str, column: &str) -> TaggedColumn {
    TaggedColumn {
        catalog: catalog.to_string(),
        schema: schema.to_string(),
        table: table.to_string(),
        column: column.to_string(),
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
    let out = compile_rule(&r, EngineType::StarRocks, "sr-1", None, &tagged, 100).unwrap();
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
    let err = compile_rule(&r, EngineType::StarRocks, "sr-1", None, &tagged, 100).unwrap_err();
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
    assert!(compile_rule(&r, EngineType::StarRocks, "sr-1", None, &tagged, 100).is_err());
    // And one under the ceiling still compiles, so the test above is not
    // passing merely because compilation is broken.
    let ok = compile_rule(&r, EngineType::StarRocks, "sr-1", None, &tagged[..100], 100).unwrap();
    assert_eq!(ok.len(), 100);
}

#[test]
fn a_custom_mask_expression_that_does_not_parse_is_refused_at_compile_time() {
    // Ranger never parses this string; it fails inside the engine at query
    // time. Refusing here is the difference between an author finding out now
    // and a user finding out from a broken dashboard.
    let r = mask_rule_custom("pii", "initcap(reverse({col})"); // unbalanced paren
    let err = compile_rule(
        &r,
        EngineType::Trino,
        "trino-1",
        Some("trino-1_tag"),
        &[],
        100,
    );
    assert!(err.is_err(), "an unparseable custom mask must not compile");
}
