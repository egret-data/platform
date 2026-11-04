//! Task 4: row filters and column masks (plan D2 "Row filters", "Masks"; D1 mask rendering).

use egret_perm_eval::EvalError;
use egret_perm_eval::{ClassifiedColumn, Snapshot, SnapshotInput};
use egret_perm_ir::audience::{Audience, Member, MemberName, Via};
use egret_perm_ir::statement::*;
use uuid::Uuid;

fn who(users: &[&str], except: &[&str]) -> Audience {
    let m = |n: &str| Member {
        name: if n == "*" {
            MemberName::Everyone
        } else {
            MemberName::User(n.into())
        },
        via: Via::Direct,
    };
    Audience {
        include: users.iter().map(|u| m(u)).collect(),
        except: except.iter().map(|u| m(u)).collect(),
    }
}
fn path(c: &str, s: Option<&str>, t: Option<&str>, col: Option<&str>) -> Target {
    let l = |v: Option<&str>| v.map_or(Level::Unnamed, |v| Level::Named(v.into()));
    Target::Path(Path {
        catalog_id: None,
        catalog: c.into(),
        schema: l(s),
        table: l(t),
        column: l(col),
    })
}
fn st(source_n: u128, audience: Audience, target: Target, effect: Effect) -> Statement {
    Statement {
        source: Source::Grant {
            grant_id: Uuid::from_u128(source_n),
        },
        audience,
        target,
        effect,
        derivation: Derivation::AsDeclared,
    }
}
fn snap(statements: Vec<Statement>, classified: Vec<ClassifiedColumn>) -> Snapshot {
    Snapshot::build(SnapshotInput {
        service_id: 1,
        key: (1, 1),
        statements,
        classified,
    })
    .unwrap()
}

// ---- The brief's tests, verbatim. ----

#[test]
fn a_legacy_row_filter_with_an_unnamed_table_filters_every_table_of_the_schema() {
    let s = snap(
        vec![st(
            1,
            who(&["*"], &[]),
            path("lake", Some("s"), None, None),
            Effect::RowFilter {
                expression: "region = 'us'".into(),
            },
        )],
        vec![],
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "any_table").unwrap(),
        vec!["(region = 'us')".to_string()]
    );
    assert!(s
        .row_filters("ann", "lake", "other", "t")
        .unwrap()
        .is_empty());
}

#[test]
fn an_agreeing_tie_is_one_mask_and_one_filter() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), Some("c")),
                Effect::Mask {
                    mask_type: "MASK".into(),
                    expression: None,
                },
            ),
            st(
                2,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), Some("c")),
                Effect::Mask {
                    mask_type: "MASK".into(),
                    expression: None,
                },
            ),
            st(
                3,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), None),
                Effect::RowFilter {
                    expression: "x = 1".into(),
                },
            ),
            st(
                4,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), None),
                Effect::RowFilter {
                    expression: "x = 1".into(),
                },
            ),
        ],
        vec![],
    );
    assert_eq!(
        s.column_masks("ann", "lake", "s", "t", &[("c".into(), "varchar".into())])
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "t").unwrap(),
        vec!["(x = 1)".to_string()]
    );
}

#[test]
fn row_filters_combine_with_and_in_statement_order() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), None),
                Effect::RowFilter {
                    expression: "region = 'us'".into(),
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), None),
                Effect::RowFilter {
                    expression: "tier < 3".into(),
                },
            ),
        ],
        vec![],
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "t").unwrap(),
        vec!["(region = 'us') AND (tier < 3)".to_string()]
    );
    assert_eq!(
        s.row_filters("bob", "lake", "s", "t").unwrap(),
        vec!["(region = 'us')".to_string()]
    );
    assert!(s
        .row_filters("bob", "lake", "s", "other")
        .unwrap()
        .is_empty());
}

#[test]
fn a_hash_mask_renders_the_service_def_transformer() {
    let s = snap(
        vec![st(
            1,
            who(&["*"], &["boss"]),
            path("lake", Some("s"), Some("t"), Some("email")),
            Effect::Mask {
                mask_type: "MASK_HASH".into(),
                expression: None,
            },
        )],
        vec![],
    );
    let m = s
        .column_masks(
            "ann",
            "lake",
            "s",
            "t",
            &[
                ("email".into(), "varchar".into()),
                ("id".into(), "bigint".into()),
            ],
        )
        .unwrap();
    assert_eq!(
        m.get("email").map(String::as_str),
        Some("cast(to_hex(sha256(to_utf8(email))) as varchar)")
    );
    assert!(!m.contains_key("id"));
    assert!(s
        .column_masks(
            "boss",
            "lake",
            "s",
            "t",
            &[("email".into(), "varchar".into())]
        )
        .unwrap()
        .is_empty());
}

#[test]
fn null_custom_and_none_render_like_the_ranger_plugin() {
    assert_eq!(
        egret_perm_eval::render_mask("MASK_NULL", None, "c", "varchar").as_deref(),
        Some("NULL")
    );
    assert_eq!(
        egret_perm_eval::render_mask("CUSTOM", Some("substr(c, 1, 2)"), "c", "varchar").as_deref(),
        Some("substr(c, 1, 2)")
    );
    assert_eq!(
        egret_perm_eval::render_mask("CUSTOM", None, "c", "varchar").as_deref(),
        Some("NULL")
    );
    assert_eq!(
        egret_perm_eval::render_mask("MASK_NONE", None, "c", "varchar"),
        None
    );
}

#[test]
fn the_transformers_equal_the_trino_service_def() {
    let def: serde_json::Value = serde_json::from_str(include_str!(
        "../../egret-policy/src/service-defs/trino.json"
    ))
    .unwrap();
    for m in def["dataMaskDef"]["maskTypes"].as_array().unwrap() {
        let name = m["name"].as_str().unwrap();
        let want = m.get("transformer").and_then(|t| t.as_str());
        let got = egret_perm_eval::TRINO_MASK_TRANSFORMERS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| *t);
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn a_classification_mask_and_a_withhold_mask_reach_the_tagged_column() {
    let pii = ClassifiedColumn {
        catalog: "lake".into(),
        schema: "s".into(),
        table: "t".into(),
        column: Some("ssn".into()),
        classification: "pii".into(),
    };
    let s = snap(
        vec![Statement {
            source: Source::Rule {
                rule_id: Uuid::nil(),
                rule_name: "r".into(),
            },
            audience: who(&["*"], &[]),
            target: Target::Classification { name: "pii".into() },
            effect: Effect::Mask {
                mask_type: "MASK_SHOW_LAST_4".into(),
                expression: None,
            },
            derivation: Derivation::AsDeclared,
        }],
        vec![pii],
    );
    assert!(s
        .column_masks("ann", "lake", "s", "t", &[("ssn".into(), "varchar".into())])
        .unwrap()["ssn"]
        .starts_with("cast(regexp_replace(ssn,"));
}

#[test]
fn two_masks_on_one_column_is_an_error_not_a_choice() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), Some("c")),
                Effect::Mask {
                    mask_type: "MASK_NULL".into(),
                    expression: None,
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), Some("c")),
                Effect::Mask {
                    mask_type: "MASK_HASH".into(),
                    expression: None,
                },
            ),
        ],
        vec![],
    );
    assert!(matches!(
        s.column_masks("ann", "lake", "s", "t", &[("c".into(), "varchar".into())]),
        Err(EvalError::TwoMasks { .. })
    ));
    assert!(s
        .column_masks("bob", "lake", "s", "t", &[("c".into(), "varchar".into())])
        .is_ok());
}

// ---- Added for the mutation checks of the binding decisions. ----

fn mask(mask_type: &str, expression: Option<&str>) -> Effect {
    Effect::Mask {
        mask_type: mask_type.into(),
        expression: expression.map(str::to_string),
    }
}
fn filter(e: &str) -> Effect {
    Effect::RowFilter {
        expression: e.into(),
    }
}
fn withhold_mask(audience: Audience, classification: &str) -> Statement {
    Statement {
        source: Source::Withhold {
            classification_id: Uuid::nil(),
            classification_name: classification.into(),
        },
        audience,
        target: Target::Classification {
            name: classification.into(),
        },
        effect: Effect::WithholdMask,
        derivation: Derivation::AsDeclared,
    }
}
fn tag(table: &str, column: Option<&str>, classification: &str) -> ClassifiedColumn {
    ClassifiedColumn {
        catalog: "lake".into(),
        schema: "s".into(),
        table: table.into(),
        column: column.map(str::to_string),
        classification: classification.into(),
    }
}
fn one(c: &str, ty: &str) -> Vec<(String, String)> {
    vec![(c.to_string(), ty.to_string())]
}

#[test]
fn the_and_keeps_statement_order_not_audience_order() {
    // The user-specific filter comes first in statement order here.
    let s = snap(
        vec![
            st(
                1,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), None),
                filter("b = 2"),
            ),
            st(
                2,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), None),
                filter("a = 1"),
            ),
        ],
        vec![],
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "t").unwrap(),
        vec!["(b = 2) AND (a = 1)".to_string()]
    );
}

#[test]
fn a_merged_filter_keeps_its_first_position() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), None),
                filter("a = 1"),
            ),
            st(
                2,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), None),
                filter("b = 2"),
            ),
            st(
                3,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), None),
                filter("a = 1"),
            ),
        ],
        vec![],
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "t").unwrap(),
        vec!["(a = 1) AND (b = 2)".to_string()]
    );
}

#[test]
fn a_row_filter_except_and_a_stranger_get_no_filter() {
    let s = snap(
        vec![st(
            1,
            who(&["ann", "bob"], &["bob"]),
            path("lake", Some("s"), Some("t"), None),
            filter("a = 1"),
        )],
        vec![],
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "t").unwrap(),
        vec!["(a = 1)".to_string()]
    );
    assert!(s.row_filters("bob", "lake", "s", "t").unwrap().is_empty());
    assert!(s.row_filters("cid", "lake", "s", "t").unwrap().is_empty());
}

#[test]
fn row_filter_paths_match_ignoring_case_and_by_wildcard() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("Lake", Some("S"), Some("T"), None),
                filter("a = 1"),
            ),
            st(
                2,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("sales_*"), None),
                filter("b = 2"),
            ),
        ],
        vec![],
    );
    assert_eq!(
        s.row_filters("ann", "LAKE", "s", "t").unwrap(),
        vec!["(a = 1)".to_string()]
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "Sales_EU").unwrap(),
        vec!["(b = 2)".to_string()]
    );
    assert!(s.row_filters("ann", "lake", "s", "hr").unwrap().is_empty());
}

#[test]
fn a_legacy_row_filter_with_an_unnamed_schema_filters_every_table_of_the_catalog() {
    let s = snap(
        vec![st(
            1,
            who(&["*"], &[]),
            path("lake", None, None, None),
            filter("a = 1"),
        )],
        vec![],
    );
    assert_eq!(
        s.row_filters("ann", "lake", "x", "y").unwrap(),
        vec!["(a = 1)".to_string()]
    );
    assert!(s.row_filters("ann", "sea", "x", "y").unwrap().is_empty());
}

#[test]
fn a_wildcard_run_row_filter_level_matches_every_table() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("**"), None),
                filter("a = 1"),
            ),
            st(
                2,
                who(&["*"], &[]),
                path("lake", Some("**"), Some("**"), None),
                filter("b = 2"),
            ),
        ],
        vec![],
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "t").unwrap(),
        vec!["(a = 1) AND (b = 2)".to_string()]
    );
    // A request value of `*` is answered only by a match-any level.
    assert_eq!(
        s.row_filters("ann", "lake", "other", "*").unwrap(),
        vec!["(b = 2)".to_string()]
    );
}

#[test]
fn a_legacy_row_filter_naming_a_column_filters_its_table() {
    // Ranger drops the column from a row-filter resource (ROW_FILTER_LEVELS).
    let s = snap(
        vec![st(
            1,
            who(&["*"], &[]),
            path("lake", Some("s"), Some("t"), Some("c")),
            filter("a = 1"),
        )],
        vec![],
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "t").unwrap(),
        vec!["(a = 1)".to_string()]
    );
}

#[test]
fn a_classification_row_filter_reaches_a_table_carrying_the_tag() {
    let s = snap(
        vec![Statement {
            source: Source::Rule {
                rule_id: Uuid::nil(),
                rule_name: "r".into(),
            },
            audience: who(&["*"], &[]),
            target: Target::Classification { name: "EU".into() },
            effect: filter("region = 'eu'"),
            derivation: Derivation::AsDeclared,
        }],
        vec![tag("T", None, "eu")],
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "t").unwrap(),
        vec!["(region = 'eu')".to_string()]
    );
    assert!(s.row_filters("ann", "lake", "s", "u").unwrap().is_empty());
}

#[test]
fn an_empty_row_filter_expression_filters_nothing() {
    // Ranger's exempt idiom: an empty filterExpr is no row filter at all.
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), None),
                filter(""),
            ),
            st(
                2,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), None),
                filter("a = 1"),
            ),
        ],
        vec![],
    );
    assert_eq!(
        s.row_filters("ann", "lake", "s", "t").unwrap(),
        vec!["(a = 1)".to_string()]
    );
}

#[test]
fn a_withhold_mask_is_null_except_for_the_exempt() {
    let s = snap(
        vec![withhold_mask(who(&["*"], &["cleared"]), "pii")],
        vec![tag("t", Some("ssn"), "pii")],
    );
    let cs = one("ssn", "varchar");
    assert_eq!(
        s.column_masks("ann", "lake", "s", "t", &cs)
            .unwrap()
            .get("ssn")
            .map(String::as_str),
        Some("NULL")
    );
    assert_eq!(
        s.column_mask_choice("ann", "lake", "s", "t", "ssn")
            .unwrap(),
        Some(("MASK_NULL".to_string(), None))
    );
    assert!(s
        .column_masks("cleared", "lake", "s", "t", &cs)
        .unwrap()
        .is_empty());
    assert!(s
        .column_masks("ann", "lake", "s", "t", &one("id", "bigint"))
        .unwrap()
        .is_empty());
}

#[test]
fn a_withhold_mask_merges_with_a_null_mask_and_conflicts_with_any_other() {
    let null = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), Some("ssn")),
                mask("MASK_NULL", None),
            ),
            withhold_mask(who(&["*"], &[]), "pii"),
        ],
        vec![tag("t", Some("ssn"), "pii")],
    );
    assert_eq!(
        null.column_masks("ann", "lake", "s", "t", &one("ssn", "varchar"))
            .unwrap()["ssn"],
        "NULL"
    );
    let hash = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), Some("ssn")),
                mask("MASK_HASH", None),
            ),
            withhold_mask(who(&["*"], &["cleared"]), "pii"),
        ],
        vec![tag("t", Some("ssn"), "pii")],
    );
    assert_eq!(
        hash.column_masks("ann", "lake", "s", "t", &one("ssn", "varchar")),
        Err(EvalError::TwoMasks {
            user: "ann".into(),
            column: "ssn".into()
        })
    );
    // The exempt user meets only the hash.
    assert_eq!(
        hash.column_masks("cleared", "lake", "s", "t", &one("ssn", "varchar"))
            .unwrap()["ssn"],
        "cast(to_hex(sha256(to_utf8(ssn))) as varchar)"
    );
}

#[test]
fn a_table_classification_masks_every_column_of_the_table() {
    let s = snap(
        vec![Statement {
            source: Source::Rule {
                rule_id: Uuid::nil(),
                rule_name: "r".into(),
            },
            audience: who(&["*"], &[]),
            target: Target::Classification { name: "pii".into() },
            effect: mask("MASK_NULL", None),
            derivation: Derivation::AsDeclared,
        }],
        vec![tag("t", None, "pii")],
    );
    let m = s
        .column_masks(
            "ann",
            "lake",
            "s",
            "t",
            &[("a".into(), "int".into()), ("b".into(), "int".into())],
        )
        .unwrap();
    assert_eq!(m.len(), 2);
    assert!(s
        .column_masks("ann", "lake", "s", "u", &one("a", "int"))
        .unwrap()
        .is_empty());
}

#[test]
fn an_unnamed_mask_column_covers_every_column_of_the_table() {
    let s = snap(
        vec![st(
            1,
            who(&["*"], &[]),
            path("lake", Some("s"), Some("t"), None),
            mask("MASK_NULL", None),
        )],
        vec![],
    );
    let m = s
        .column_masks(
            "ann",
            "lake",
            "s",
            "t",
            &[("a".into(), "int".into()), ("b".into(), "int".into())],
        )
        .unwrap();
    assert_eq!(m.len(), 2);
    assert!(s
        .column_masks("ann", "lake", "s", "u", &one("a", "int"))
        .unwrap()
        .is_empty());
}

#[test]
fn mask_paths_match_ignoring_case_and_by_wildcard() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("Lake", Some("S"), Some("T"), Some("SSN")),
                mask("MASK_NULL", None),
            ),
            st(
                2,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), Some("e*")),
                mask("MASK_HASH", None),
            ),
        ],
        vec![],
    );
    let m = s
        .column_masks(
            "ann",
            "lake",
            "s",
            "t",
            &[
                ("ssn".into(), "varchar".into()),
                ("Email".into(), "varchar".into()),
                ("id".into(), "int".into()),
            ],
        )
        .unwrap();
    assert_eq!(m.get("ssn").map(String::as_str), Some("NULL"));
    assert_eq!(
        m.get("Email").map(String::as_str),
        Some("cast(to_hex(sha256(to_utf8(Email))) as varchar)")
    );
    assert!(!m.contains_key("id"));
}

#[test]
fn a_mask_none_applies_renders_nothing_and_still_conflicts() {
    let s = snap(
        vec![st(
            1,
            who(&["*"], &[]),
            path("lake", Some("s"), Some("t"), Some("c")),
            mask("MASK_NONE", None),
        )],
        vec![],
    );
    assert!(s
        .column_masks("ann", "lake", "s", "t", &one("c", "int"))
        .unwrap()
        .is_empty());
    assert_eq!(
        s.column_mask_choice("ann", "lake", "s", "t", "c").unwrap(),
        Some(("MASK_NONE".to_string(), None))
    );
    let two = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), Some("c")),
                mask("MASK_NONE", None),
            ),
            st(
                2,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), Some("c")),
                mask("MASK_HASH", None),
            ),
        ],
        vec![],
    );
    assert!(matches!(
        two.column_mask_choice("ann", "lake", "s", "t", "c"),
        Err(EvalError::TwoMasks { .. })
    ));
}

#[test]
fn masks_with_the_same_type_but_different_expressions_conflict() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), Some("c")),
                mask("CUSTOM", Some("left({col}, 1)")),
            ),
            st(
                2,
                who(&["*"], &[]),
                path("lake", Some("s"), Some("t"), Some("c")),
                mask("CUSTOM", Some("left({col}, 2)")),
            ),
        ],
        vec![],
    );
    assert!(matches!(
        s.column_masks("ann", "lake", "s", "t", &one("c", "varchar")),
        Err(EvalError::TwoMasks { .. })
    ));
}

#[test]
fn the_mask_choice_is_none_when_nothing_applies() {
    let s = snap(
        vec![st(
            1,
            who(&["ann"], &[]),
            path("lake", Some("s"), Some("t"), Some("c")),
            mask("MASK_HASH", None),
        )],
        vec![],
    );
    assert_eq!(
        s.column_mask_choice("bob", "lake", "s", "t", "c").unwrap(),
        None
    );
    assert_eq!(
        s.column_mask_choice("ann", "lake", "s", "t", "c").unwrap(),
        Some(("MASK_HASH".to_string(), None))
    );
}

#[test]
fn every_transformer_substitutes_col_and_type() {
    use egret_perm_eval::render_mask;
    assert_eq!(
        render_mask("MASK", None, "name", "varchar(20)").as_deref(),
        Some("cast(regexp_replace(regexp_replace(regexp_replace(name,'([A-Z])', 'X'),'([a-z])','x'),'([0-9])','0') as varchar(20))")
    );
    assert_eq!(
        render_mask("MASK_SHOW_LAST_4", None, "cc", "varchar").as_deref(),
        Some("cast(regexp_replace(cc, '(.*)(.{4}$)', x -> regexp_replace(x[1], '.', 'X') || x[2]) as varchar)")
    );
    assert_eq!(
        render_mask("MASK_SHOW_FIRST_4", None, "cc", "char(16)").as_deref(),
        Some("cast(regexp_replace(cc, '(^.{4})(.*)', x -> x[1] || regexp_replace(x[2], '.', 'X')) as char(16))")
    );
    assert_eq!(
        render_mask("MASK_HASH", None, "e", "varchar").as_deref(),
        Some("cast(to_hex(sha256(to_utf8(e))) as varchar)")
    );
    assert_eq!(
        render_mask("MASK_DATE_SHOW_YEAR", None, "born", "date").as_deref(),
        Some("date_trunc('year', born)")
    );
}

#[test]
fn a_custom_mask_substitutes_col_and_type_like_the_plugin() {
    // The Trino plugin's getColumnMask replaces {col} and {type} in whatever
    // transformer it chose, CUSTOM's maskedValue included; the lowering
    // requires {col} in every CUSTOM expression (InvalidCustomMask).
    use egret_perm_eval::render_mask;
    assert_eq!(
        render_mask("CUSTOM", Some("concat('x-', {col})"), "ssn", "varchar").as_deref(),
        Some("concat('x-', ssn)")
    );
    assert_eq!(
        render_mask(
            "CUSTOM",
            Some("cast(left({col}, 2) as {type})"),
            "c",
            "varchar(9)"
        )
        .as_deref(),
        Some("cast(left(c, 2) as varchar(9))")
    );
    assert_eq!(
        render_mask("CUSTOM", Some("  "), "c", "varchar").as_deref(),
        Some("NULL")
    );
}

#[test]
fn an_unknown_mask_type_fails_closed_to_null() {
    // Ranger would find no transformer and mask nothing; the evaluator never
    // masks less than Ranger, and NULL is the most it can mask.
    assert_eq!(
        egret_perm_eval::render_mask("MASK_SOMETHING_NEW", None, "c", "varchar").as_deref(),
        Some("NULL")
    );
}

fn classification_mask(name: &str) -> Statement {
    Statement {
        source: Source::Rule {
            rule_id: Uuid::nil(),
            rule_name: "r".into(),
        },
        audience: who(&["*"], &[]),
        target: Target::Classification { name: name.into() },
        effect: mask("MASK_NULL", None),
        derivation: Derivation::AsDeclared,
    }
}

#[test]
fn a_mask_selects_its_classification_ignoring_case() {
    let s = snap(
        vec![classification_mask("PII")],
        vec![tag("t", Some("ssn"), "pii")],
    );
    assert_eq!(
        s.column_masks("ann", "lake", "s", "t", &one("ssn", "varchar"))
            .unwrap()
            .get("ssn")
            .map(String::as_str),
        Some("NULL")
    );
}

#[test]
fn a_tagged_column_is_masked_whatever_case_the_request_uses() {
    let s = snap(
        vec![classification_mask("pii")],
        vec![tag("t", Some("ssn"), "pii")],
    );
    assert_eq!(
        s.column_masks("ann", "lake", "s", "t", &one("SSN", "varchar"))
            .unwrap()
            .get("SSN")
            .map(String::as_str),
        Some("NULL")
    );
}
