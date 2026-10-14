//! The closed grammar for masking, row-filter and tag-addressed policies
//! (SP4d-3 Task 4) — the rule-side twin of `grammar_access.rs`.
//!
//! Every blocker this grammar adds is asserted by NAME, with its accept cell,
//! and every accepted shape is compiled BACK through the real compiler
//! (`compile_rule`) and compared with the body it came from: the grammar is
//! only correct if it is the inverse of that compiler. Accepting what the
//! compiler would render differently is fail-OPEN here — data shown unmasked,
//! rows unfiltered.
//!
//! Pure (no DB).

use std::collections::BTreeSet;

use egret_perm::{EngineType, PrincipalKind, RuleKind, Selector};
use egret_perm_compile::grammar::{
    grants_from_access_policy, rules_from_mask_or_filter_policy, Blocker, CandidateRules,
    ServiceKind,
};
use egret_perm_compile::{compile_rule, ResolvedNames};
use serde_json::{json, Value};
use uuid::Uuid;

const CAT: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
const CAT2: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

fn catalogs(name: &str) -> Option<Uuid> {
    match name {
        "wh" => Some(CAT),
        "lake" => Some(CAT2),
        _ => None,
    }
}

fn catalog_name(id: Uuid) -> &'static str {
    match id {
        CAT => "wh",
        CAT2 => "lake",
        _ => panic!("no catalog name for {id}"),
    }
}

/// The tag registry, looked up by `tags.key` and answering that tag's
/// `tags.name`. Key and name are independent columns, and this registry
/// makes them differ on purpose:
///
/// | key            | name           |
/// |----------------|----------------|
/// | `pii`          | `Sensitive`    | ← key of A …
/// | `b-key`        | `pii`          | ← … is the NAME of B (the crossing case)
/// | `confidential` | `Confidential` | ← same word, different case
/// | `phi`          | `phi`          | ← key == name: the one migratable shape
fn tags(key: &str) -> Option<String> {
    match key {
        "pii" => Some("Sensitive".into()),
        "b-key" => Some("pii".into()),
        "confidential" => Some("Confidential".into()),
        "phi" => Some("phi".into()),
        _ => None,
    }
}

/// The service a body belongs in, read off its own resources — so every
/// helper call below is CORRECTLY placed, and the placement tests pass the
/// wrong one explicitly.
fn home(body: &Value) -> ServiceKind {
    if body["resources"].get("tag").is_some() {
        ServiceKind::Tag
    } else {
        ServiceKind::Resource
    }
}

fn level(values: &[&str]) -> Value {
    json!({"values": values, "isExcludes": false, "isRecursive": false})
}

fn select() -> Value {
    json!([{"type": "select", "isAllowed": true}])
}

fn mask_item(users: &[&str], groups: &[&str], mask_type: &str) -> Value {
    json!({
        "users": users, "groups": groups, "roles": [],
        "accesses": select(),
        "dataMaskInfo": {"dataMaskType": mask_type}
    })
}

fn everyone_mask(mask_type: &str) -> Value {
    mask_item(&[], &["public"], mask_type)
}

fn filter_item(users: &[&str], groups: &[&str], expr: &str) -> Value {
    json!({
        "users": users, "groups": groups, "roles": [],
        "accesses": select(),
        "rowFilterInfo": {"filterExpr": expr}
    })
}

/// A path-addressed masking policy on `wh.sales.orders.email` (Trino names).
fn mask_policy(items: Vec<Value>) -> Value {
    json!({
        "name": "mask emails",
        "policyType": 1,
        "isEnabled": true,
        "resources": {
            "catalog": level(&["wh"]),
            "schema":  level(&["sales"]),
            "table":   level(&["orders"]),
            "column":  level(&["email"])
        },
        "dataMaskPolicyItems": items,
        "policyItems": [],
        "denyPolicyItems": []
    })
}

/// A path-addressed row-filter policy on `wh.sales.orders` (Trino names).
fn filter_policy(items: Vec<Value>) -> Value {
    json!({
        "name": "only EU rows",
        "policyType": 2,
        "isEnabled": true,
        "resources": {
            "catalog": level(&["wh"]),
            "schema":  level(&["sales"]),
            "table":   level(&["orders"])
        },
        "rowFilterPolicyItems": items,
        "policyItems": [],
        "denyPolicyItems": []
    })
}

/// A tag-addressed masking policy (what `tag_masking_to_ranger_json` emits).
fn tag_mask_policy(tag: &str, items: Vec<Value>) -> Value {
    json!({
        "name": "mask pii",
        "service": "tags",
        "serviceType": "tag",
        "policyType": 1,
        "isEnabled": true,
        "resources": {"tag": level(&[tag])},
        "dataMaskPolicyItems": items,
        "policyItems": [],
        "denyPolicyItems": []
    })
}

fn accept(body: &Value, engine: EngineType) -> CandidateRules {
    rules_from_mask_or_filter_policy(body, engine, &catalogs, &tags, home(body))
        .unwrap_or_else(|b| panic!("expected an accept, got {b:?}"))
}

fn refused(body: &Value, engine: EngineType) -> Vec<Blocker> {
    rules_from_mask_or_filter_policy(body, engine, &catalogs, &tags, home(body))
        .expect_err("expected a refusal")
}

fn has(blockers: &[Blocker], want: &Blocker) -> bool {
    blockers.contains(want)
}

/// Insert (or replace) the key at the end of `pointer`.
fn set(body: &mut Value, pointer: &str, v: Value) {
    let (parent, key) = pointer.rsplit_once('/').expect(pointer);
    let target = if parent.is_empty() {
        body
    } else {
        body.pointer_mut(parent).expect(parent)
    };
    target
        .as_object_mut()
        .expect("an object")
        .insert(key.to_string(), v);
}

// ── Brief item 1 ────────────────────────────────────────────────────────────

/// A column-level mask whose type the engine declares → one `Mask` rule on
/// that exact path. The refusal cell: a type the ENGINE does not declare
/// (`MASK_SHOW_LAST_4` is Trino-only) — a name the plugin does not know is
/// silently ignored, so the policy would deploy and mask nothing.
#[test]
fn a_column_mask_of_a_declared_type_becomes_one_mask_rule() {
    let got = accept(
        &mask_policy(vec![everyone_mask("MASK_HASH")]),
        EngineType::Trino,
    );
    assert_eq!(got.rules.len(), 1);
    let r = &got.rules[0];
    assert_eq!(r.id, Uuid::nil());
    assert_eq!(r.name, "mask emails");
    assert_eq!(
        r.kind,
        RuleKind::Mask {
            mask_type: "MASK_HASH".into(),
            value_expr: None
        }
    );
    let Selector::Path(scope) = &r.selector else {
        panic!("expected a path selector, got {:?}", r.selector)
    };
    assert_eq!(scope.catalog_id, CAT);
    assert_eq!(scope.schema.as_deref(), Some("sales"));
    assert_eq!(scope.table.as_deref(), Some("orders"));
    assert_eq!(scope.column.as_deref(), Some("email"));
    assert!(r.exempt.include.is_empty() && r.exempt.exclude.is_empty());

    let ok = accept(
        &mask_policy(vec![everyone_mask("MASK_SHOW_LAST_4")]),
        EngineType::Trino,
    );
    assert_eq!(ok.rules.len(), 1, "MASK_SHOW_LAST_4 is declared on Trino");

    let mut sr = mask_policy(vec![everyone_mask("MASK_SHOW_LAST_4")]);
    let res = sr["resources"].as_object_mut().unwrap();
    let schema = res.remove("schema").unwrap();
    res.insert("database".into(), schema);
    let blockers = refused(&sr, EngineType::StarRocks);
    assert_eq!(
        blockers,
        vec![Blocker::UnsupportedMaskType {
            mask_type: "MASK_SHOW_LAST_4".into()
        }]
    );
}

// ── Brief item 2 ────────────────────────────────────────────────────────────

/// A row filter is exactly catalog + schema + table. A `column` level is not
/// in `rowFilterDef.resources` on either engine, so it is
/// [`Blocker::UnknownLevel`] — the level-name rule, not the empty-level one —
/// and an ABSENT table is [`Blocker::MissingLevel`], because the compiler
/// always emits all three (`scope_to_resources`' `RowFilter` arm).
#[test]
fn a_row_filter_is_exactly_three_levels_and_a_column_is_an_unknown_level() {
    let got = accept(
        &filter_policy(vec![filter_item(&[], &["public"], "region = 'EU'")]),
        EngineType::Trino,
    );
    assert_eq!(got.rules.len(), 1);
    let r = &got.rules[0];
    assert_eq!(
        r.kind,
        RuleKind::RowFilter {
            filter_expr: "region = 'EU'".into()
        }
    );
    let Selector::Path(scope) = &r.selector else {
        panic!("expected a path selector")
    };
    assert_eq!(
        (
            scope.catalog_id,
            scope.schema.as_deref(),
            scope.table.as_deref(),
            scope.column.as_deref()
        ),
        (CAT, Some("sales"), Some("orders"), None)
    );

    let mut with_column = filter_policy(vec![filter_item(&[], &["public"], "region = 'EU'")]);
    with_column["resources"]["column"] = level(&["email"]);
    assert_eq!(
        refused(&with_column, EngineType::Trino),
        vec![Blocker::UnknownLevel {
            level: "column".into()
        }]
    );

    let mut no_table = filter_policy(vec![filter_item(&[], &["public"], "region = 'EU'")]);
    no_table["resources"]
        .as_object_mut()
        .unwrap()
        .remove("table");
    assert_eq!(
        refused(&no_table, EngineType::Trino),
        vec![Blocker::MissingLevel {
            level: "table".into()
        }]
    );
}

// ── Brief item 3 ────────────────────────────────────────────────────────────

/// A tag-addressed mask resolves its tag through `tags.key`, and is accepted
/// only when that tag's NAME is the very string the body names: the engine
/// matches tag policies on names, and `compile_rule` stamps the resolved tag's
/// name back (`tagify`). Absent from the registry → [`Blocker::UnknownTag`]
/// (the migrator never creates a tag).
#[test]
fn a_tag_mask_resolves_its_tag_by_key_or_names_the_unknown_tag() {
    // key == name == V: accepted (and compiled back to the same name in the
    // oracle test below).
    let got = accept(
        &tag_mask_policy("phi", vec![everyone_mask("MASK_NULL")]),
        EngineType::Trino,
    );
    assert_eq!(got.rules.len(), 1);
    assert_eq!(got.rules[0].selector, Selector::Tag { key: "phi".into() });
    assert_eq!(
        got.rules[0].kind,
        RuleKind::Mask {
            mask_type: "MASK_NULL".into(),
            value_expr: None
        }
    );

    assert_eq!(
        refused(
            &tag_mask_policy("secret", vec![everyone_mask("MASK_NULL")]),
            EngineType::Trino
        ),
        vec![Blocker::UnknownTag {
            key: "secret".into()
        }]
    );
    // A tag's NAME is not a key: `Sensitive` is A's name, and nothing has
    // that key. Refused, never matched by name.
    assert_eq!(
        refused(
            &tag_mask_policy("Sensitive", vec![everyone_mask("MASK_NULL")]),
            EngineType::Trino
        ),
        vec![Blocker::UnknownTag {
            key: "Sensitive".into()
        }]
    );
}

/// THE CROSSING CASE (review, Critical 1). The body says `pii`. To the
/// engine that is tag B (NAMED `pii`); to a key lookup it is tag A (KEY
/// `pii`, named `Sensitive`). Accepting it would compile back to a policy on
/// `Sensitive` — A's columns masked, B's columns UNMASKED once the original
/// is disabled. Fail-open, so refused, naming both strings.
#[test]
fn a_tag_whose_key_is_another_tags_name_is_refused() {
    assert_eq!(
        refused(
            &tag_mask_policy("pii", vec![everyone_mask("MASK_NULL")]),
            EngineType::Trino
        ),
        vec![Blocker::TagNameDiffersFromKey {
            key: "pii".into(),
            name: "Sensitive".into()
        }]
    );
}

/// The key exists but its name is a different string (here only by case —
/// Ranger's tag match is exact). Without the crossing tag this fails closed
/// rather than open, but the compiled-back body still names another tag.
#[test]
fn a_tag_whose_name_differs_from_its_key_is_refused() {
    let mut filter = tag_mask_policy("confidential", vec![]);
    filter
        .as_object_mut()
        .unwrap()
        .remove("dataMaskPolicyItems");
    set(&mut filter, "/policyType", json!(2));
    set(
        &mut filter,
        "/rowFilterPolicyItems",
        json!([filter_item(&[], &["public"], "x = 1")]),
    );
    for body in [
        tag_mask_policy("confidential", vec![everyone_mask("MASK_NULL")]),
        filter,
    ] {
        assert_eq!(
            refused(&body, EngineType::Trino),
            vec![Blocker::TagNameDiffersFromKey {
                key: "confidential".into(),
                name: "Confidential".into()
            }]
        );
    }
}

// ── Brief item 4 ────────────────────────────────────────────────────────────

/// A mask item with no `dataMaskInfo` is refused. On the TAG repository such
/// an item NPEs the plugin's constructor and the engine serves NOTHING, so it
/// must never be reproduced into a compiled artifact. Absent, `null` and `{}`
/// are the same thing to Ranger (Jackson's setter substitutes an empty info
/// whose type is null), so all three are the one blocker.
#[test]
fn a_mask_item_without_data_mask_info_is_refused() {
    let mut bodies = Vec::new();
    for info in [None, Some(Value::Null), Some(json!({}))] {
        let mut it = everyone_mask("MASK_HASH");
        let obj = it.as_object_mut().unwrap();
        match info {
            None => {
                obj.remove("dataMaskInfo");
            }
            Some(v) => {
                obj.insert("dataMaskInfo".into(), v);
            }
        }
        bodies.push(tag_mask_policy("phi", vec![it.clone()]));
        bodies.push(mask_policy(vec![it]));
    }
    for body in &bodies {
        assert_eq!(
            refused(body, EngineType::Trino),
            vec![Blocker::MaskWithoutType { index: 0 }],
            "body: {body}"
        );
    }

    // The accept cell: the same item WITH a type.
    accept(
        &tag_mask_policy("phi", vec![everyone_mask("MASK_HASH")]),
        EngineType::Trino,
    );
}

// ── Brief item 5 ────────────────────────────────────────────────────────────

/// `*` above a named level: REFUSED on a grant (Task 3, 0179), ACCEPTED on a
/// rule — product decision 2026-10-10 (`0179`'s closing section): on a rule
/// it widens restriction, not authority, and both plugins enforce it. The `*`
/// is carried verbatim, because a mask renders exactly the levels it names
/// (`scope_to_resources`' `Mask` arm), so `Some("*")` is what compiles back to
/// `*`. Both sides of the asymmetry are asserted here.
#[test]
fn a_wildcard_above_a_named_level_is_accepted_on_a_rule_and_refused_on_a_grant() {
    let mut body = mask_policy(vec![everyone_mask("MASK_HASH")]);
    body["resources"]["schema"] = level(&["*"]);
    let got = accept(&body, EngineType::Trino);
    let Selector::Path(scope) = &got.rules[0].selector else {
        panic!("expected a path selector")
    };
    assert_eq!(scope.schema.as_deref(), Some("*"));
    assert_eq!(scope.table.as_deref(), Some("orders"));
    assert_eq!(scope.column.as_deref(), Some("email"));
    // ...and it compiles back to the very `*` it came from. Checked HERE,
    // not in the oracle test below, so that pinning the asymmetry is one
    // test's job (a mutant flipping it fails this test alone).
    let back = compile_rule(
        &got.rules[0],
        EngineType::Trino,
        "svc",
        None,
        &ResolvedNames {
            catalog_name: "wh",
            tag_name: None,
        },
        &[],
        100,
    )
    .expect("the compiler accepts a match-any level above a named one on a rule");
    assert_eq!(back[0]["resources"], body["resources"]);

    let grant = json!({
        "name": "read email anywhere",
        "resources": body["resources"].clone(),
        "policyItems": [{"users": ["alice"], "groups": [], "roles": [], "accesses": select()}]
    });
    let blockers =
        grants_from_access_policy(&grant, EngineType::Trino, &catalogs, ServiceKind::Resource)
            .expect_err("the grant side refuses the same resources");
    assert!(
        blockers.contains(&Blocker::WildcardOverNamedLevel {
            level: "schema".into()
        }),
        "{blockers:?}"
    );
}

// ── The principal shape: `public`, optionally preceded by one exemption ────

/// `compile_rule` emits exactly `[exempt (MASK_NONE / empty filter)?,
/// public (the rule)]`. That shape is accepted and becomes `exempt.include`;
/// everything else is a per-principal control the model has no shape for.
#[test]
fn an_exemption_ahead_of_the_everyone_item_becomes_exempt_include() {
    let got = accept(
        &mask_policy(vec![
            mask_item(&["alice"], &["auditors"], "MASK_NONE"),
            everyone_mask("MASK_HASH"),
        ]),
        EngineType::Trino,
    );
    let exempt = &got.rules[0].exempt;
    assert!(exempt.exclude.is_empty());
    let got: Vec<(PrincipalKind, &str)> = exempt
        .include
        .iter()
        .map(|p| (p.kind, p.r#ref.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (PrincipalKind::User, "alice"),
            (PrincipalKind::Group, "auditors")
        ]
    );

    let got = accept(
        &filter_policy(vec![
            filter_item(&["bob"], &[], ""),
            filter_item(&[], &["public"], "region = 'EU'"),
        ]),
        EngineType::Trino,
    );
    assert_eq!(got.rules[0].exempt.include.len(), 1);
}

#[test]
fn a_per_principal_mask_is_not_an_exemption() {
    // Analysts get a hash, everyone else a null: two different masks.
    assert_eq!(
        refused(
            &mask_policy(vec![
                mask_item(&[], &["analysts"], "MASK_HASH"),
                everyone_mask("MASK_NULL"),
            ]),
            EngineType::Trino
        ),
        vec![Blocker::NotAnExemption { index: 0 }]
    );
    // A row-filter item ahead of `public` that still filters.
    assert_eq!(
        refused(
            &filter_policy(vec![
                filter_item(&["bob"], &[], "1 = 1"),
                filter_item(&[], &["public"], "region = 'EU'"),
            ]),
            EngineType::Trino
        ),
        vec![Blocker::NotAnExemption { index: 0 }]
    );
}

#[test]
fn a_mask_for_named_principals_only_is_not_for_everyone() {
    // A Rule applies to EVERYONE; migrating "mask for alice" would mask every
    // user's view of the column.
    assert_eq!(
        refused(
            &mask_policy(vec![mask_item(&["alice"], &[], "MASK_HASH")]),
            EngineType::Trino
        ),
        vec![Blocker::NotForEveryone { index: 0 }]
    );
    // `public` plus someone else is not the compiler's item either.
    assert_eq!(
        refused(
            &mask_policy(vec![mask_item(&["alice"], &["public"], "MASK_HASH")]),
            EngineType::Trino
        ),
        vec![Blocker::NotForEveryone { index: 0 }]
    );
    // The everyone item FIRST makes a later exemption dead in Ranger (first
    // match wins); the compiler would put the exemption first and revive it.
    let blockers = refused(
        &mask_policy(vec![
            everyone_mask("MASK_HASH"),
            mask_item(&["alice"], &[], "MASK_NONE"),
        ]),
        EngineType::Trino,
    );
    assert!(
        has(&blockers, &Blocker::NotForEveryone { index: 1 }),
        "{blockers:?}"
    );
}

#[test]
fn an_exemption_naming_public_or_a_third_item_is_refused() {
    assert_eq!(
        refused(
            &mask_policy(vec![
                mask_item(&[], &["public"], "MASK_NONE"),
                everyone_mask("MASK_HASH"),
            ]),
            EngineType::Trino
        ),
        vec![Blocker::PublicExemption]
    );
    assert_eq!(
        refused(
            &mask_policy(vec![
                mask_item(&["a"], &[], "MASK_NONE"),
                mask_item(&["b"], &[], "MASK_NONE"),
                everyone_mask("MASK_HASH"),
            ]),
            EngineType::Trino
        ),
        vec![Blocker::TooManyRuleItems { count: 3 }]
    );
    // Task 3's principal blockers apply to the exemption too.
    let mut role = mask_policy(vec![
        mask_item(&[], &["auditors"], "MASK_NONE"),
        everyone_mask("MASK_HASH"),
    ]);
    set(
        &mut role,
        "/dataMaskPolicyItems/0/roles",
        json!(["steward"]),
    );
    assert!(has(
        &refused(&role, EngineType::Trino),
        &Blocker::RolePrincipal {
            role: "steward".into()
        }
    ));
    assert!(has(
        &refused(
            &mask_policy(vec![
                mask_item(&["a@b"], &[], "MASK_NONE"),
                everyone_mask("MASK_HASH"),
            ]),
            EngineType::Trino
        ),
        &Blocker::UnusableUserRef { user: "a@b".into() }
    ));
}

#[test]
fn a_mask_or_filter_item_must_carry_exactly_select() {
    let mut extra = mask_policy(vec![everyone_mask("MASK_HASH")]);
    set(
        &mut extra,
        "/dataMaskPolicyItems/0/accesses",
        json!([{"type": "select"}, {"type": "update"}]),
    );
    assert_eq!(
        refused(&extra, EngineType::Trino),
        vec![Blocker::UnknownAccessType {
            access_type: "update".into()
        }]
    );
    let mut none = filter_policy(vec![filter_item(&[], &["public"], "x = 1")]);
    set(&mut none, "/rowFilterPolicyItems/0/accesses", json!([]));
    assert_eq!(
        refused(&none, EngineType::Trino),
        vec![Blocker::EmptyAccesses]
    );
    let mut denied = mask_policy(vec![everyone_mask("MASK_HASH")]);
    set(
        &mut denied,
        "/dataMaskPolicyItems/0/accesses",
        json!([{"type": "select", "isAllowed": false}]),
    );
    assert!(has(
        &refused(&denied, EngineType::Trino),
        &Blocker::AccessNotAllowed {
            access_type: "select".into()
        }
    ));
}

// ── Mask values and filter expressions ─────────────────────────────────────

#[test]
fn mask_values_are_accepted_only_where_the_compiler_emits_them() {
    // CUSTOM with `{col}`: Ranger reports the string verbatim (its request
    // resolver only fires on `${{…}}`), and the compiler emits it verbatim.
    let mut custom = mask_policy(vec![everyone_mask("CUSTOM")]);
    set(
        &mut custom,
        "/dataMaskPolicyItems/0/dataMaskInfo/valueExpr",
        json!("concat('x', {col})"),
    );
    assert_eq!(
        accept(&custom, EngineType::Trino).rules[0].kind,
        RuleKind::Mask {
            mask_type: "CUSTOM".into(),
            value_expr: Some("concat('x', {col})".into())
        }
    );

    // A request-time expression is resolved per request by Ranger.
    let mut request = custom.clone();
    set(
        &mut request,
        "/dataMaskPolicyItems/0/dataMaskInfo/valueExpr",
        json!("${{USER.dept}}"),
    );
    assert_eq!(
        refused(&request, EngineType::Trino),
        vec![Blocker::MaskExpression {
            expr: "${{USER.dept}}".into()
        }]
    );

    // A CUSTOM the compiler itself refuses (no `{col}`) — the backstop.
    let mut unparseable = custom.clone();
    set(
        &mut unparseable,
        "/dataMaskPolicyItems/0/dataMaskInfo/valueExpr",
        json!("upper(x)"),
    );
    let blockers = refused(&unparseable, EngineType::Trino);
    assert!(
        matches!(blockers.as_slice(), [Blocker::CompilerRefuses { error }] if error.contains("upper(x)")),
        "{blockers:?}"
    );

    // A value on a non-CUSTOM type: the compiler would drop it.
    let mut stray = mask_policy(vec![everyone_mask("MASK_HASH")]);
    set(
        &mut stray,
        "/dataMaskPolicyItems/0/dataMaskInfo/valueExpr",
        json!("x"),
    );
    assert_eq!(
        refused(&stray, EngineType::Trino),
        vec![Blocker::ValueExprWithoutCustom {
            mask_type: "MASK_HASH".into()
        }]
    );

    // A conditional mask.
    let mut cond = mask_policy(vec![everyone_mask("MASK_HASH")]);
    set(
        &mut cond,
        "/dataMaskPolicyItems/0/dataMaskInfo/conditionExpr",
        json!("x > 1"),
    );
    assert_eq!(refused(&cond, EngineType::Trino), vec![Blocker::Conditions]);
}

#[test]
fn filter_expressions_must_be_present_and_static() {
    for info in [json!({}), json!({"filterExpr": null}), Value::Null] {
        let mut body = filter_policy(vec![filter_item(&[], &["public"], "x = 1")]);
        set(&mut body, "/rowFilterPolicyItems/0/rowFilterInfo", info);
        assert_eq!(
            refused(&body, EngineType::Trino),
            vec![Blocker::MissingFilterExpr { index: 0 }]
        );
    }
    for expr in ["dept = '${{USER.dept}}'", "id IN [[x]]"] {
        assert_eq!(
            refused(
                &filter_policy(vec![filter_item(&[], &["public"], expr)]),
                EngineType::Trino
            ),
            vec![Blocker::FilterExpression { expr: expr.into() }]
        );
    }
}

// ── Policy kinds, tags and engines ─────────────────────────────────────────

#[test]
fn policy_kinds_the_rule_model_cannot_hold_are_refused() {
    let mut access = mask_policy(vec![]);
    set(&mut access, "/policyType", json!(0));
    set(
        &mut access,
        "/policyItems",
        json!([{"users": ["a"], "accesses": select()}]),
    );
    access
        .as_object_mut()
        .unwrap()
        .remove("dataMaskPolicyItems");
    assert!(has(
        &refused(&access, EngineType::Trino),
        &Blocker::NotAMaskOrRowFilter { policy_type: 0 }
    ));

    // A tag ACCESS policy: a clearance's cleared set is role MEMBERSHIP
    // enumerated at compile time, so enumerated users cannot be inverted
    // into it.
    let mut tag_deny = tag_mask_policy("phi", vec![]);
    set(&mut tag_deny, "/policyType", json!(0));
    tag_deny
        .as_object_mut()
        .unwrap()
        .remove("dataMaskPolicyItems");
    set(
        &mut tag_deny,
        "/denyPolicyItems",
        json!([{"groups": ["public"], "accesses": select()}]),
    );
    let blockers = refused(&tag_deny, EngineType::Trino);
    assert!(
        has(&blockers, &Blocker::ClearanceNotExpressible),
        "{blockers:?}"
    );
    assert!(!has(
        &blockers,
        &Blocker::NotAMaskOrRowFilter { policy_type: 0 }
    ));

    // Items in the other kind's bucket, or in an access bucket.
    let mut both = mask_policy(vec![everyone_mask("MASK_HASH")]);
    set(
        &mut both,
        "/rowFilterPolicyItems",
        json!([filter_item(&[], &["public"], "x = 1")]),
    );
    set(
        &mut both,
        "/policyItems",
        json!([{"users": ["a"], "accesses": select()}]),
    );
    let blockers = refused(&both, EngineType::Trino);
    assert!(has(
        &blockers,
        &Blocker::ForeignItems {
            bucket: "rowFilterPolicyItems"
        }
    ));
    assert!(has(
        &blockers,
        &Blocker::ForeignItems {
            bucket: "policyItems"
        }
    ));

    assert_eq!(
        refused(&mask_policy(vec![]), EngineType::Trino),
        vec![Blocker::NothingToMigrate]
    );
}

/// StarRocks has no tag service: a tag rule compiles there to one resource
/// policy PER COLUMN, a different artifact from the tag body it came from.
#[test]
fn a_tag_selector_is_refused_on_starrocks() {
    assert_eq!(
        refused(
            &tag_mask_policy("phi", vec![everyone_mask("MASK_HASH")]),
            EngineType::StarRocks
        ),
        vec![Blocker::TagSelectorOnStarRocks]
    );
    accept(
        &tag_mask_policy("phi", vec![everyone_mask("MASK_HASH")]),
        EngineType::Trino,
    );
}

#[test]
fn a_tag_resource_names_one_tag_and_nothing_else() {
    let mut two = tag_mask_policy("phi", vec![everyone_mask("MASK_HASH")]);
    set(&mut two, "/resources/tag", level(&["phi", "pci"]));
    assert!(has(
        &refused(&two, EngineType::Trino),
        &Blocker::MultipleValues {
            level: "tag".into(),
            count: 2
        }
    ));
    let mut mixed = tag_mask_policy("phi", vec![everyone_mask("MASK_HASH")]);
    set(&mut mixed, "/resources/catalog", level(&["wh"]));
    assert!(has(
        &refused(&mixed, EngineType::Trino),
        &Blocker::UnknownLevel {
            level: "catalog".into()
        }
    ));
    let mut flagged = tag_mask_policy("phi", vec![everyone_mask("MASK_HASH")]);
    set(&mut flagged, "/resources/tag/isExcludes", json!(true));
    assert!(has(
        &refused(&flagged, EngineType::Trino),
        &Blocker::ResourceFlag {
            level: "tag".into(),
            flag: "isExcludes"
        }
    ));
    let mut empty = tag_mask_policy("phi", vec![everyone_mask("MASK_HASH")]);
    set(&mut empty, "/resources/tag", level(&[]));
    assert!(has(
        &refused(&empty, EngineType::Trino),
        &Blocker::EmptyLevel {
            level: "tag".into()
        }
    ));
}

// ── Levels ─────────────────────────────────────────────────────────────────

/// The missing-level answer for rules: a MASK renders only the levels its
/// scope names, so an absent TRAILING level is exactly `None` and is accepted
/// (unlike a grant, whose compiler always emits all four). A gap above a
/// named level is still refused — `Scope` cannot skip a level.
#[test]
fn a_mask_may_stop_above_the_column_but_may_not_skip_a_level() {
    let mut table_wide = mask_policy(vec![everyone_mask("MASK_HASH")]);
    table_wide["resources"]
        .as_object_mut()
        .unwrap()
        .remove("column");
    let got = accept(&table_wide, EngineType::Trino);
    let Selector::Path(scope) = &got.rules[0].selector else {
        panic!()
    };
    assert_eq!(scope.table.as_deref(), Some("orders"));
    assert_eq!(scope.column, None);

    let mut gap = mask_policy(vec![everyone_mask("MASK_HASH")]);
    gap["resources"].as_object_mut().unwrap().remove("schema");
    assert_eq!(
        refused(&gap, EngineType::Trino),
        vec![Blocker::MissingLevel {
            level: "schema".into()
        }]
    );
    let mut no_catalog = mask_policy(vec![everyone_mask("MASK_HASH")]);
    no_catalog["resources"]
        .as_object_mut()
        .unwrap()
        .remove("catalog");
    assert!(has(
        &refused(&no_catalog, EngineType::Trino),
        &Blocker::MissingLevel {
            level: "catalog".into()
        }
    ));
}

#[test]
fn several_catalogs_become_n_rules_but_several_columns_are_refused() {
    let mut body = mask_policy(vec![everyone_mask("MASK_HASH")]);
    body["resources"]["catalog"] = level(&["wh", "lake"]);
    let got = accept(&body, EngineType::Trino);
    let ids: Vec<Uuid> = got
        .rules
        .iter()
        .map(|r| match &r.selector {
            Selector::Path(s) => s.catalog_id,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, vec![CAT, CAT2]);
    let names: BTreeSet<&str> = got.rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names.len(), 2, "perm.rules.name is UNIQUE: {names:?}");

    let mut cols = mask_policy(vec![everyone_mask("MASK_HASH")]);
    cols["resources"]["column"] = level(&["email", "phone"]);
    assert_eq!(
        refused(&cols, EngineType::Trino),
        vec![Blocker::MultipleValues {
            level: "column".into(),
            count: 2
        }]
    );
    let mut unregistered = mask_policy(vec![everyone_mask("MASK_HASH")]);
    unregistered["resources"]["catalog"] = level(&["wh", "ext"]);
    assert_eq!(
        refused(&unregistered, EngineType::Trino),
        vec![Blocker::UnregisteredCatalog {
            catalog: "ext".into()
        }]
    );
}

// ── Header and unknown fields ──────────────────────────────────────────────

#[test]
fn decision_relevant_header_fields_are_refused_on_rules_too() {
    let cases: Vec<(&str, Value, Blocker)> = vec![
        ("/isEnabled", json!(false), Blocker::Disabled),
        (
            "/policyPriority",
            json!(1),
            Blocker::NonDefaultPriority { priority: 1 },
        ),
        ("/isDenyAllElse", json!(true), Blocker::DenyAllElse),
        (
            "/additionalResources",
            json!([{"catalog": level(&["wh"])}]),
            Blocker::AdditionalResources,
        ),
        (
            "/validitySchedules",
            json!([{"startTime": "2026/01/01 00:00:00"}]),
            Blocker::ValiditySchedules,
        ),
        (
            "/zoneName",
            json!("z1"),
            Blocker::SecurityZone { zone: "z1".into() },
        ),
    ];
    for (pointer, value, want) in cases {
        let mut body = mask_policy(vec![everyone_mask("MASK_HASH")]);
        body.as_object_mut()
            .unwrap()
            .insert(pointer[1..].to_string(), value);
        let blockers = refused(&body, EngineType::Trino);
        assert!(has(&blockers, &want), "{pointer}: {blockers:?}");
    }
    // Inert metadata is not a blocker.
    let mut meta = mask_policy(vec![everyone_mask("MASK_HASH")]);
    for (k, v) in [
        ("id", json!(7)),
        ("guid", json!("g")),
        ("isAuditEnabled", json!(true)),
        ("serviceType", json!("trino")),
    ] {
        meta.as_object_mut().unwrap().insert(k.into(), v);
    }
    accept(&meta, EngineType::Trino);
}

#[test]
fn unknown_fields_inside_mask_and_filter_items_are_refused() {
    let mut body = mask_policy(vec![everyone_mask("MASK_HASH")]);
    set(
        &mut body,
        "/dataMaskPolicyItems/0/dataMaskInfo/foo",
        json!(1),
    );
    set(&mut body, "/dataMaskPolicyItems/0/bar", json!(1));
    set(&mut body, "/isOverride", json!(true));
    let blockers = refused(&body, EngineType::Trino);
    for field in [
        "dataMaskPolicyItems[0].dataMaskInfo.foo",
        "dataMaskPolicyItems[0].bar",
        "isOverride",
    ] {
        assert!(
            has(
                &blockers,
                &Blocker::UnrecognisedField {
                    field: field.into()
                }
            ),
            "{field}: {blockers:?}"
        );
    }
    let mut filter = filter_policy(vec![filter_item(&[], &["public"], "x = 1")]);
    set(
        &mut filter,
        "/rowFilterPolicyItems/0/rowFilterInfo/baz",
        json!(1),
    );
    assert_eq!(
        refused(&filter, EngineType::Trino),
        vec![Blocker::UnrecognisedField {
            field: "rowFilterPolicyItems[0].rowFilterInfo.baz".into()
        }]
    );
    // `null` is Ranger's absent and is never a blocker.
    let mut nulls = mask_policy(vec![everyone_mask("MASK_HASH")]);
    set(
        &mut nulls,
        "/dataMaskPolicyItems/0/dataMaskInfo/conditionExpr",
        Value::Null,
    );
    set(
        &mut nulls,
        "/dataMaskPolicyItems/0/delegateAdmin",
        json!(false),
    );
    accept(&nulls, EngineType::Trino);
}

#[test]
fn every_blocker_is_collected_not_just_the_first() {
    let mut body = mask_policy(vec![mask_item(&["alice"], &[], "MASK_SHOW_LAST_4")]);
    body["resources"]["catalog"] = level(&["ext"]);
    set(&mut body, "/policyPriority", json!(2));
    set(
        &mut body,
        "/dataMaskPolicyItems/0/conditions",
        json!([{"type": "ip"}]),
    );
    let blockers = refused(&body, EngineType::StarRocks);
    for want in [
        Blocker::UnregisteredCatalog {
            catalog: "ext".into(),
        },
        Blocker::NonDefaultPriority { priority: 2 },
        Blocker::Conditions,
        Blocker::NotForEveryone { index: 0 },
        Blocker::UnsupportedMaskType {
            mask_type: "MASK_SHOW_LAST_4".into(),
        },
        // Trino's level name on a StarRocks policy.
        Blocker::UnknownLevel {
            level: "schema".into(),
        },
    ] {
        assert!(has(&blockers, &want), "missing {want:?} in {blockers:?}");
    }
}

// ── The design oracle ──────────────────────────────────────────────────────

/// Every accepted shape, compiled back by the REAL compiler, has the same
/// policy type, priority, resources (catalog value swapped for the rule's own
/// catalog), and items — principals, accesses and the mask/filter payload.
#[test]
fn every_accepted_body_compiles_back_to_what_it_came_from() {
    // (The `*`-above-a-named-level shape is compiled back in
    // `a_wildcard_above_a_named_level_is_accepted_on_a_rule_and_refused_on_a_grant`.)
    let mut table_wide = mask_policy(vec![everyone_mask("MASK_NULL")]);
    table_wide["resources"]
        .as_object_mut()
        .unwrap()
        .remove("column");
    let mut custom = mask_policy(vec![everyone_mask("CUSTOM")]);
    set(
        &mut custom,
        "/dataMaskPolicyItems/0/dataMaskInfo/valueExpr",
        json!("concat('x', {col})"),
    );
    let mut sr = mask_policy(vec![
        mask_item(&["alice"], &[], "MASK_NONE"),
        everyone_mask("MASK_HASH"),
    ]);
    let schema = sr["resources"].as_object_mut().unwrap().remove("schema");
    sr["resources"]["database"] = schema.unwrap();
    let mut sr_filter = filter_policy(vec![filter_item(&[], &["public"], "x = 1")]);
    let schema = sr_filter["resources"]
        .as_object_mut()
        .unwrap()
        .remove("schema");
    sr_filter["resources"]["database"] = schema.unwrap();
    let mut two_catalogs = filter_policy(vec![filter_item(&[], &["public"], "x = 1")]);
    two_catalogs["resources"]["catalog"] = level(&["wh", "lake"]);

    let shapes: Vec<(&str, Value, EngineType)> = vec![
        (
            "path mask",
            mask_policy(vec![everyone_mask("MASK_HASH")]),
            EngineType::Trino,
        ),
        (
            "mask with exemption",
            mask_policy(vec![
                mask_item(&["alice"], &["auditors"], "MASK_NONE"),
                everyone_mask("MASK_SHOW_LAST_4"),
            ]),
            EngineType::Trino,
        ),
        ("table-wide mask", table_wide, EngineType::Trino),
        ("custom mask", custom, EngineType::Trino),
        ("starrocks mask", sr, EngineType::StarRocks),
        (
            "row filter with exemption",
            filter_policy(vec![
                filter_item(&["bob"], &[], ""),
                filter_item(&[], &["public"], "region = 'EU'"),
            ]),
            EngineType::Trino,
        ),
        ("starrocks row filter", sr_filter, EngineType::StarRocks),
        ("two catalogs", two_catalogs, EngineType::Trino),
        (
            "tag mask",
            tag_mask_policy("phi", vec![everyone_mask("MASK_NULL")]),
            EngineType::Trino,
        ),
        (
            "tag row filter",
            json!({
                "name": "tag filter", "service": "tags", "serviceType": "tag",
                "policyType": 2,
                "resources": {"tag": level(&["phi"])},
                "rowFilterPolicyItems": [
                    filter_item(&["bob"], &[], ""),
                    filter_item(&[], &["public"], "x = 1")
                ]
            }),
            EngineType::Trino,
        ),
    ];

    for (what, body, engine) in shapes {
        let got = accept(&body, engine);
        for rule in &got.rules {
            let (catalog, tag_name) = match &rule.selector {
                Selector::Path(s) => (catalog_name(s.catalog_id), None),
                // The REAL resolution the compiler is given: key → name,
                // from a registry where key ≠ name for most tags. Only an
                // accepted key can get here, and its name must be the body's.
                Selector::Tag { key } => ("unused", tags(key)),
            };
            let compiled = compile_rule(
                rule,
                engine,
                "svc",
                Some("tags"),
                &ResolvedNames {
                    catalog_name: catalog,
                    tag_name: tag_name.as_deref(),
                },
                &[],
                100,
            )
            .unwrap_or_else(|e| panic!("{what}: compile_rule refused an accepted rule: {e:?}"));
            assert_eq!(compiled.len(), 1, "{what}");
            let back = &compiled[0];

            let mut want_resources = body["resources"].clone();
            if want_resources.get("catalog").is_some() {
                want_resources["catalog"] = level(&[catalog]);
            }
            assert_eq!(back["resources"], want_resources, "{what}: resources");
            assert_eq!(back["policyType"], body["policyType"], "{what}: type");
            assert_eq!(
                back["policyPriority"].as_i64().unwrap_or(0),
                body["policyPriority"].as_i64().unwrap_or(0),
                "{what}: priority"
            );
            for bucket in ["dataMaskPolicyItems", "rowFilterPolicyItems"] {
                let want = body[bucket].as_array().cloned().unwrap_or_default();
                let mut have = back[bucket].as_array().cloned().unwrap_or_default();
                // View governance R5: on Trino the compiler adds the lineage
                // principal's no-transform item FIRST; the rest is the body's.
                if engine == EngineType::Trino && !have.is_empty() {
                    let lineage = have.remove(0);
                    assert_eq!(
                        lineage["users"],
                        serde_json::json!(["egret-lineage"]),
                        "{what}: {bucket} starts with the lineage item"
                    );
                }
                assert_eq!(want.len(), have.len(), "{what}: {bucket} length");
                for (w, h) in want.iter().zip(&have) {
                    assert!(same_item(w, h), "{what}: {bucket}\n want {w}\n have {h}");
                }
            }
        }
    }
}

fn names(v: &Value) -> BTreeSet<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn same_item(a: &Value, b: &Value) -> bool {
    let accesses = |v: &Value| -> BTreeSet<String> {
        v["accesses"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x["type"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    // The payload objects are compared as Values (never via `to_string`).
    names(&a["users"]) == names(&b["users"])
        && names(&a["groups"]) == names(&b["groups"])
        && names(&a["roles"]) == names(&b["roles"])
        && accesses(a) == accesses(b)
        && a["dataMaskInfo"] == b["dataMaskInfo"]
        && a["rowFilterInfo"] == b["rowFilterInfo"]
}

// ── Service placement (review, Important 2) ────────────────────────────────

/// A body is only meaningful in the service kind its resources address: a
/// tag body in a RESOURCE service matches nothing, and so does a path body in
/// a TAG service. The compiler files each rule in the matching service, so
/// migrating a misplaced body would start enforcing what enforced nothing.
#[test]
fn a_rule_body_in_the_wrong_kind_of_service_is_refused() {
    let tag = tag_mask_policy("phi", vec![everyone_mask("MASK_NULL")]);
    let path = mask_policy(vec![everyone_mask("MASK_HASH")]);
    let run = |body: &Value, service| {
        rules_from_mask_or_filter_policy(body, EngineType::Trino, &catalogs, &tags, service)
    };

    assert!(run(&tag, ServiceKind::Tag).is_ok());
    assert_eq!(
        run(&tag, ServiceKind::Resource).expect_err("tag body in a resource service"),
        vec![Blocker::TagBodyInResourceService]
    );
    assert!(run(&path, ServiceKind::Resource).is_ok());
    assert_eq!(
        run(&path, ServiceKind::Tag).expect_err("path body in a tag service"),
        vec![Blocker::PathBodyInTagService]
    );
}

/// The same confusion on the grant side: a path ACCESS body in a tag service
/// is refused; a tag-addressed access body is a clearance's shape — refused
/// as such in a tag service, and as misplaced as well in a resource one.
#[test]
fn an_access_body_in_the_wrong_kind_of_service_is_refused() {
    let path = json!({
        "name": "read orders",
        "resources": {
            "catalog": level(&["wh"]), "schema": level(&["sales"]),
            "table": level(&["orders"]), "column": level(&["*"])
        },
        "policyItems": [{"users": ["alice"], "accesses": select()}]
    });
    let run = |body: &Value, service| {
        grants_from_access_policy(body, EngineType::Trino, &catalogs, service)
    };
    assert!(run(&path, ServiceKind::Resource).is_ok());
    assert_eq!(
        run(&path, ServiceKind::Tag).expect_err("path body in a tag service"),
        vec![Blocker::PathBodyInTagService]
    );

    let tag_deny = json!({
        "name": "gate phi",
        "resources": {"tag": level(&["phi"])},
        "denyPolicyItems": [{"groups": ["public"], "accesses": select()}]
    });
    let in_tag = run(&tag_deny, ServiceKind::Tag).expect_err("a clearance shape");
    assert!(
        in_tag.contains(&Blocker::ClearanceNotExpressible),
        "{in_tag:?}"
    );
    assert!(!in_tag.contains(&Blocker::TagBodyInResourceService));
    let in_resource = run(&tag_deny, ServiceKind::Resource).expect_err("misplaced too");
    assert!(
        in_resource.contains(&Blocker::TagBodyInResourceService)
            && in_resource.contains(&Blocker::ClearanceNotExpressible),
        "{in_resource:?}"
    );
}
