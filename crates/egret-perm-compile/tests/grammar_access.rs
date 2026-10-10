//! The closed grammar for access policies. Three things are asserted, and the
//! second is the one a field-by-field translator gets wrong.
//!
//! Beyond the three: every blocker the grammar can emit is asserted by NAME,
//! with its accept cell where one is meaningful, and every accepted shape is
//! compiled BACK through the real compiler (`compile_grant` +
//! `access_to_ranger_json`) and compared with the body it came from — the
//! grammar is only correct if it is the inverse of that compiler.
//!
//! Pure (no DB).

use std::collections::BTreeSet;

use egret_perm::{Action, Effect, EngineType, PrincipalKind};
use egret_perm_compile::compile_grant;
use egret_perm_compile::grammar::{grants_from_access_policy, Blocker};
use egret_policy::admin::access_to_ranger_json;
use serde_json::{json, Value};
use uuid::Uuid;

const CAT: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
const CAT2: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);

fn lookup(name: &str) -> Option<Uuid> {
    if name == "wh" {
        Some(CAT)
    } else {
        None
    }
}

fn lookup_two(name: &str) -> Option<Uuid> {
    match name {
        "wh" => Some(CAT),
        "lake" => Some(CAT2),
        _ => None,
    }
}

fn catalog_name(id: Uuid) -> &'static str {
    if id == CAT {
        "wh"
    } else if id == CAT2 {
        "lake"
    } else {
        panic!("no catalog name for {id}")
    }
}

/// ADJUSTED from the brief: the brief's fixture had no `column` level. The
/// compiler always emits all four levels with `*` below the scope
/// (`grants.rs::scope_to_resources`), and an ABSENT `column` is measured to
/// grant nothing on a column `select` (`MatchType::Ancestor`), so a body
/// without it is refused as [`Blocker::MissingLevel`] — see
/// `an_absent_level_is_refused_but_a_star_level_is_accepted`.
fn policy(accesses: &[&str]) -> Value {
    json!({
        "name": "sales can read orders",
        "resources": {
            "catalog": {"values": ["wh"], "isExcludes": false, "isRecursive": false},
            "schema":  {"values": ["sales"], "isExcludes": false, "isRecursive": false},
            "table":   {"values": ["orders"], "isExcludes": false, "isRecursive": false},
            "column":  {"values": ["*"], "isExcludes": false, "isRecursive": false}
        },
        "policyItems": [{
            "users": ["alice"],
            "groups": [],
            "roles": [],
            "accesses": accesses.iter().map(|a| json!({"type": a, "isAllowed": true})).collect::<Vec<_>>()
        }],
        "denyPolicyItems": [],
        "allowExceptions": [],
        "denyExceptions": [],
        "isEnabled": true
    })
}

fn level(values: &[&str]) -> Value {
    json!({"values": values, "isExcludes": false, "isRecursive": false})
}

fn item(users: &[&str], accesses: &[&str]) -> Value {
    json!({
        "users": users, "groups": [], "roles": [],
        "accesses": accesses.iter().map(|a| json!({"type": a, "isAllowed": true})).collect::<Vec<_>>()
    })
}

fn refused(body: &Value, engine: EngineType) -> Vec<Blocker> {
    grants_from_access_policy(body, engine, &lookup).expect_err("expected a refusal")
}

/// A full expansion is accepted and produces exactly the grant it should.
#[test]
fn a_full_action_expansion_becomes_one_grant() {
    let got = grants_from_access_policy(&policy(&["select"]), EngineType::Trino, &lookup)
        .expect("select is Action::Read on Trino");
    assert_eq!(got.grants.len(), 1);
    assert_eq!(got.grants[0].actions, vec![Action::Read]);
    assert_eq!(got.grants[0].scope.catalog_id, CAT);
    assert_eq!(got.grants[0].scope.schema.as_deref(), Some("sales"));
    assert_eq!(got.grants[0].scope.table.as_deref(), Some("orders"));
    // Added: the `*` column is the compiler's rendering of "no column".
    assert_eq!(got.grants[0].scope.column, None);

    let got = grants_from_access_policy(&policy(&["insert", "delete"]), EngineType::Trino, &lookup)
        .expect("insert+delete is exactly Action::Write on Trino");
    assert_eq!(got.grants[0].actions, vec![Action::Write]);
}

/// THE SUBSET TRAP. `Action::Write` expands to insert+delete on Trino, so a
/// policy granting only `insert` is NOT expressible: migrating it as `Write`
/// would silently add `delete`. A grammar that only rejected UNKNOWN access
/// types would accept this and widen access.
#[test]
fn a_strict_subset_of_an_action_expansion_is_refused_and_names_what_is_missing() {
    let blockers = grants_from_access_policy(&policy(&["insert"]), EngineType::Trino, &lookup)
        .expect_err("insert alone is a strict subset of Action::Write on Trino");
    assert!(
        blockers.iter().any(|b| matches!(
            b,
            Blocker::PartialActionExpansion { action: Action::Write, missing }
                if missing == &vec!["delete".to_string()]
        )),
        "expected a PartialActionExpansion naming `delete`, got {blockers:?}"
    );

    // The same body IS expressible on StarRocks? No — there Write is
    // insert+update+delete, so `insert` alone is still a subset, and the
    // missing list is longer. Asserting both engines keeps the expansion table
    // from being read as engine-independent.
    let blockers = grants_from_access_policy(&policy(&["insert"]), EngineType::StarRocks, &lookup)
        .expect_err("insert alone is a strict subset of Action::Write on StarRocks too");
    assert!(blockers.iter().any(|b| matches!(
        b,
        Blocker::PartialActionExpansion { action: Action::Write, missing }
            if missing.len() == 2
    )));
}

/// Every other closed-grammar refusal, each naming its own blocker. A refusal
/// that does not say what blocked it is unusable by an operator and
/// unmeasurable by a test — and a classifier that refused everything would
/// pass a test that only checked `is_err()`.
#[test]
fn each_inexpressible_shape_names_its_own_blocker() {
    let unknown_access = grants_from_access_policy(&policy(&["alter"]), EngineType::Trino, &lookup)
        .expect_err("alter has no Action");
    assert!(unknown_access.iter().any(|b| matches!(
        b,
        Blocker::UnknownAccessType { access_type } if access_type == "alter"
    )));

    let mut unregistered = policy(&["select"]);
    unregistered["resources"]["catalog"]["values"] = json!(["not_in_registry"]);
    let blockers = grants_from_access_policy(&unregistered, EngineType::Trino, &lookup)
        .expect_err("an unregistered catalog has no Scope.catalog_id to point at");
    assert!(blockers.iter().any(|b| matches!(
        b,
        Blocker::UnregisteredCatalog { catalog } if catalog == "not_in_registry"
    )));

    let mut recursive = policy(&["select"]);
    recursive["resources"]["schema"]["isRecursive"] = json!(true);
    let blockers = grants_from_access_policy(&recursive, EngineType::Trino, &lookup)
        .expect_err("grants.rs::level hard-codes is_recursive: false");
    assert!(blockers.iter().any(|b| matches!(
        b,
        Blocker::ResourceFlag { level, flag } if level == "schema" && *flag == "isRecursive"
    )));

    let mut excludes = policy(&["select"]);
    excludes["resources"]["schema"]["isExcludes"] = json!(true);
    let blockers = grants_from_access_policy(&excludes, EngineType::Trino, &lookup)
        .expect_err("grants.rs::level hard-codes is_excludes: false");
    assert!(blockers.iter().any(|b| matches!(
        b,
        Blocker::ResourceFlag { level, flag } if level == "schema" && *flag == "isExcludes"
    )));

    let mut scheduled = policy(&["select"]);
    scheduled["validitySchedules"] = json!([{"startTime": "2026/01/01 00:00:00"}]);
    assert!(
        grants_from_access_policy(&scheduled, EngineType::Trino, &lookup)
            .expect_err("nothing in the model carries a schedule")
            .iter()
            .any(|b| matches!(b, Blocker::ValiditySchedules))
    );

    let mut prioritised = policy(&["select"]);
    prioritised["policyPriority"] = json!(1);
    assert!(
        grants_from_access_policy(&prioritised, EngineType::Trino, &lookup)
            .expect_err("a non-default priority changes arbitration and has no model field")
            .iter()
            .any(|b| matches!(b, Blocker::NonDefaultPriority { priority: 1 }))
    );

    let mut two_exceptions = policy(&["select"]);
    two_exceptions["allowExceptions"] = json!([
        {"users": ["bob"], "groups": [], "roles": [], "accesses": [{"type": "select", "isAllowed": true}]},
        {"users": ["carol"], "groups": [], "roles": [], "accesses": [{"type": "select", "isAllowed": true}]}
    ]);
    assert!(
        grants_from_access_policy(&two_exceptions, EngineType::Trino, &lookup)
            .expect_err("principals.exclude emits exactly one exception item")
            .iter()
            .any(|b| matches!(b, Blocker::TooManyExceptions { count: 2 }))
    );
}

/// One exception with the SAME access set is expressible, and lands in
/// `principals.exclude`. This is the allow cell for the exception rule — the
/// refusal above proves nothing on its own.
#[test]
fn one_exception_with_the_same_accesses_becomes_principals_exclude() {
    let mut one = policy(&["select"]);
    one["allowExceptions"] = json!([
        {"users": ["bob"], "groups": [], "roles": [], "accesses": [{"type": "select", "isAllowed": true}]}
    ]);
    let got = grants_from_access_policy(&one, EngineType::Trino, &lookup)
        .expect("one exception, same accesses, is expressible");
    assert_eq!(got.grants.len(), 1);
    assert_eq!(got.grants[0].principals.exclude.len(), 1);
}

/// A policy carrying both allow and deny items becomes TWO grants — N objects
/// from one policy is allowed, all-or-nothing is about whether ALL of them are
/// expressible.
#[test]
fn allow_and_deny_items_become_two_grants() {
    let mut both = policy(&["select"]);
    both["denyPolicyItems"] = json!([
        {"users": ["dave"], "groups": [], "roles": [], "accesses": [{"type": "select", "isAllowed": true}]}
    ]);
    let got = grants_from_access_policy(&both, EngineType::Trino, &lookup)
        .expect("both item kinds are expressible");
    assert_eq!(got.grants.len(), 2);
    assert_eq!(
        got.grants
            .iter()
            .filter(|g| g.effect == egret_perm::Effect::Deny)
            .count(),
        1
    );
}

// ── Beyond the brief ─────────────────────────────────────────────────────

/// The accept cell for unions: a set that is EXACTLY the union of two full
/// expansions is both actions, on each engine's own expansion. `update` exists
/// on StarRocks only, so on Trino it is an unknown access type, not a part of
/// `Write`.
#[test]
fn a_union_of_full_expansions_becomes_both_actions_and_update_is_starrocks_only() {
    let got = grants_from_access_policy(
        &policy(&["select", "insert", "delete"]),
        EngineType::Trino,
        &lookup,
    )
    .expect("select+insert+delete is Read+Write on Trino");
    assert_eq!(got.grants[0].actions, vec![Action::Read, Action::Write]);

    let mut sr = policy(&["select", "insert", "update", "delete"]);
    sr["resources"] = json!({
        "catalog": level(&["wh"]), "database": level(&["sales"]),
        "table": level(&["orders"]), "column": level(&["*"])
    });
    let got = grants_from_access_policy(&sr, EngineType::StarRocks, &lookup)
        .expect("select+insert+update+delete is Read+Write on StarRocks");
    assert_eq!(got.grants[0].actions, vec![Action::Read, Action::Write]);

    let blockers = refused(&policy(&["insert", "update", "delete"]), EngineType::Trino);
    assert!(
        blockers.iter().any(
            |b| matches!(b, Blocker::UnknownAccessType { access_type } if access_type == "update")
        ),
        "{blockers:?}"
    );

    // An access type's spelling is not normalised: `SELECT` is not in the
    // vocabulary as written, so it is refused rather than guessed at.
    let blockers = refused(&policy(&["SELECT"]), EngineType::Trino);
    assert!(blockers.iter().any(
        |b| matches!(b, Blocker::UnknownAccessType { access_type } if access_type == "SELECT")
    ));
}

/// An ABSENT level is not a `*` level. The compiler always emits all four
/// levels, and a policy without `column` earns `MatchType::Ancestor` on the
/// column `select` Trino asks about — it grants nothing there, while its
/// compiled-back form (`column: *`) would grant it. Refusing is the only
/// inverse.
#[test]
fn an_absent_level_is_refused_but_a_star_level_is_accepted() {
    // Accept cell: `column: *` (the fixture).
    assert!(grants_from_access_policy(&policy(&["select"]), EngineType::Trino, &lookup).is_ok());

    let mut absent = policy(&["select"]);
    absent["resources"]
        .as_object_mut()
        .unwrap()
        .remove("column");
    let blockers = refused(&absent, EngineType::Trino);
    assert!(
        blockers
            .iter()
            .any(|b| matches!(b, Blocker::MissingLevel { level } if level == "column")),
        "{blockers:?}"
    );
}

/// The level NAMES are the engine's: a Trino-shaped body (`schema`) on
/// StarRocks (`database`) is an unknown level plus a missing one, and the
/// same body is accepted on Trino. A level from another hierarchy
/// (`procedure`) is unknown on Trino too.
#[test]
fn level_names_are_the_engines_own() {
    assert!(grants_from_access_policy(&policy(&["select"]), EngineType::Trino, &lookup).is_ok());

    let blockers = refused(&policy(&["select"]), EngineType::StarRocks);
    assert!(blockers
        .iter()
        .any(|b| matches!(b, Blocker::UnknownLevel { level } if level == "schema")));
    assert!(blockers
        .iter()
        .any(|b| matches!(b, Blocker::MissingLevel { level } if level == "database")));

    let mut extra = policy(&["select"]);
    extra["resources"]["procedure"] = level(&["p"]);
    let blockers = refused(&extra, EngineType::Trino);
    assert!(blockers
        .iter()
        .any(|b| matches!(b, Blocker::UnknownLevel { level } if level == "procedure")));
}

/// Migrations 0178/0179 on the GRANT side: a `*` above a named level is the
/// level skip `perm.grants` forbids, and is refused. The accept cells: `*`
/// with nothing named beneath it, and a `**` run, which Ranger classifies as
/// the same match-any as `*`. (Task 4 asserts the opposite ruling on rules.)
#[test]
fn a_wildcard_above_a_named_level_is_refused_on_a_grant() {
    let mut skip = policy(&["select"]);
    skip["resources"]["schema"] = level(&["*"]);
    let blockers = refused(&skip, EngineType::Trino);
    assert!(
        blockers
            .iter()
            .any(|b| matches!(b, Blocker::WildcardOverNamedLevel { level } if level == "schema")),
        "{blockers:?}"
    );

    let mut run = policy(&["select"]);
    run["resources"]["schema"] = level(&["**"]);
    let blockers = refused(&run, EngineType::Trino);
    assert!(blockers
        .iter()
        .any(|b| matches!(b, Blocker::WildcardOverNamedLevel { level } if level == "schema")));

    // Accept cells.
    let mut schema_wide = policy(&["select"]);
    schema_wide["resources"]["table"] = level(&["*"]);
    let got = grants_from_access_policy(&schema_wide, EngineType::Trino, &lookup)
        .expect("a * with nothing named beneath it is a schema-wide grant");
    assert_eq!(got.grants[0].scope.schema.as_deref(), Some("sales"));
    assert_eq!(got.grants[0].scope.table, None);
    assert_eq!(got.grants[0].scope.column, None);

    let mut catalog_wide = policy(&["select"]);
    catalog_wide["resources"]["schema"] = level(&["**"]);
    catalog_wide["resources"]["table"] = level(&["*"]);
    let got = grants_from_access_policy(&catalog_wide, EngineType::Trino, &lookup)
        .expect("a wildcard run at every level below the catalog is catalog-wide");
    assert_eq!(got.grants[0].scope.schema, None);

    // A glob is a NAMED set, not a wildcard run, and is carried verbatim.
    let mut glob = policy(&["select"]);
    glob["resources"]["table"] = level(&["ord*"]);
    let got = grants_from_access_policy(&glob, EngineType::Trino, &lookup).expect("a glob");
    assert_eq!(got.grants[0].scope.table.as_deref(), Some("ord*"));
}

/// Migration 0178: an empty level names nothing and Ranger enforces it as
/// match-any. Both spellings — `values: []` and `values: [""]`.
#[test]
fn an_empty_level_is_refused() {
    for empty in [json!([]), json!([""]), json!(["", "orders"])] {
        let mut body = policy(&["select"]);
        body["resources"]["table"]["values"] = empty.clone();
        let blockers = refused(&body, EngineType::Trino);
        assert!(
            blockers
                .iter()
                .any(|b| matches!(b, Blocker::EmptyLevel { level } if level == "table")),
            "{empty}: {blockers:?}"
        );
    }
}

/// `Scope` holds ONE value per non-catalog level. Several values there are
/// refused rather than split into a cartesian product (decision recorded in
/// the module doc); several CATALOGS become N grants, per the spec.
#[test]
fn several_catalogs_become_n_grants_but_several_tables_are_refused() {
    let mut two = policy(&["select"]);
    two["resources"]["catalog"]["values"] = json!(["wh", "lake", "wh"]);
    let got = grants_from_access_policy(&two, EngineType::Trino, &lookup_two)
        .expect("two registered catalogs are two grants");
    let cats: BTreeSet<Uuid> = got.grants.iter().map(|g| g.scope.catalog_id).collect();
    assert_eq!(got.grants.len(), 2, "a repeated catalog value is one grant");
    assert_eq!(cats, BTreeSet::from([CAT, CAT2]));

    // All-or-nothing: one unregistered catalog among them refuses the policy.
    let mut partly = policy(&["select"]);
    partly["resources"]["catalog"]["values"] = json!(["wh", "nope"]);
    let blockers = grants_from_access_policy(&partly, EngineType::Trino, &lookup_two)
        .expect_err("one unregistered catalog refuses all of them");
    assert_eq!(
        blockers,
        vec![Blocker::UnregisteredCatalog {
            catalog: "nope".into()
        }]
    );

    let mut tables = policy(&["select"]);
    tables["resources"]["table"]["values"] = json!(["orders", "items"]);
    let blockers = refused(&tables, EngineType::Trino);
    assert!(
        blockers.iter().any(|b| matches!(
            b,
            Blocker::MultipleValues { level, count: 2 } if level == "table"
        )),
        "{blockers:?}"
    );
}

/// One exception whose access set differs from the item's has no
/// representation: `principals.exclude` compiles to an exception carrying the
/// item's own accesses. The same-accesses accept cell is
/// `one_exception_with_the_same_accesses_becomes_principals_exclude`.
#[test]
fn an_exception_with_a_different_access_set_is_refused() {
    let mut narrower = policy(&["select", "insert", "delete"]);
    narrower["allowExceptions"] = json!([item(&["bob"], &["select"])]);
    let blockers = refused(&narrower, EngineType::Trino);
    assert!(
        blockers.contains(&Blocker::ExceptionAccessMismatch),
        "{blockers:?}"
    );

    // The deny side, and the accept cell for it.
    let mut deny = policy(&["select"]);
    deny["policyItems"] = json!([]);
    deny["denyPolicyItems"] = json!([item(&["dave"], &["select"])]);
    deny["denyExceptions"] = json!([item(&["erin"], &["insert", "delete"])]);
    assert!(refused(&deny, EngineType::Trino).contains(&Blocker::ExceptionAccessMismatch));

    deny["denyExceptions"] = json!([item(&["erin"], &["select"])]);
    let got = grants_from_access_policy(&deny, EngineType::Trino, &lookup)
        .expect("a deny with one same-access deny exception");
    assert_eq!(got.grants.len(), 1);
    assert_eq!(got.grants[0].effect, Effect::Deny);
    assert_eq!(got.grants[0].principals.exclude[0].r#ref, "erin");
}

/// An exception with no main item in its bucket cancels nothing in Ranger and
/// has nothing to attach to in the model.
#[test]
fn an_exception_with_no_item_to_except_is_refused() {
    let mut orphan = policy(&["select"]);
    orphan["denyExceptions"] = json!([item(&["erin"], &["select"])]);
    assert!(refused(&orphan, EngineType::Trino).contains(&Blocker::OrphanException));
}

/// Policy-level and item-level conditions both gate the decision and have no
/// model field.
#[test]
fn conditions_are_refused_at_both_levels() {
    let mut policy_level = policy(&["select"]);
    policy_level["conditions"] = json!([{"type": "ip-range", "values": ["10.0.0.0/8"]}]);
    assert!(refused(&policy_level, EngineType::Trino).contains(&Blocker::Conditions));

    let mut item_level = policy(&["select"]);
    item_level["policyItems"][0]["conditions"] = json!([{"type": "ip-range", "values": ["x"]}]);
    assert!(refused(&item_level, EngineType::Trino).contains(&Blocker::Conditions));

    let mut exception_level = policy(&["select"]);
    exception_level["allowExceptions"] = json!([item(&["bob"], &["select"])]);
    exception_level["allowExceptions"][0]["conditions"] = json!([{"type": "x", "values": []}]);
    assert!(refused(&exception_level, EngineType::Trino).contains(&Blocker::Conditions));
}

/// No items at all: nothing to become a grant. The accept cell is every
/// policy above with one item.
#[test]
fn a_policy_without_items_has_nothing_to_migrate() {
    let mut empty = policy(&["select"]);
    empty["policyItems"] = json!([]);
    assert_eq!(
        refused(&empty, EngineType::Trino),
        vec![Blocker::NothingToMigrate]
    );

    let mut absent = policy(&["select"]);
    absent.as_object_mut().unwrap().remove("policyItems");
    assert_eq!(
        refused(&absent, EngineType::Trino),
        vec![Blocker::NothingToMigrate]
    );
}

/// The policy-header fields that change a decision and that a `Grant` cannot
/// carry. Each is refused by name; the inert ones are accepted.
#[test]
fn decision_relevant_header_fields_are_refused_and_inert_ones_are_not() {
    let cases: Vec<(&str, Value, Blocker)> = vec![
        (
            "policyType",
            json!(1),
            Blocker::NotAnAccessPolicy { policy_type: 1 },
        ),
        ("isDenyAllElse", json!(true), Blocker::DenyAllElse),
        (
            "additionalResources",
            json!([{"catalog": level(&["wh"])}]),
            Blocker::AdditionalResources,
        ),
        (
            "zoneName",
            json!("finance"),
            Blocker::SecurityZone {
                zone: "finance".into(),
            },
        ),
        ("isEnabled", json!(false), Blocker::Disabled),
        (
            "dataMaskPolicyItems",
            json!([{"users": ["a"], "accesses": [{"type": "select"}],
                    "dataMaskInfo": {"dataMaskType": "MASK"}}]),
            Blocker::ForeignItems {
                bucket: "dataMaskPolicyItems",
            },
        ),
        (
            "rowFilterPolicyItems",
            json!([{"users": ["a"], "accesses": [{"type": "select"}],
                    "rowFilterInfo": {"filterExpr": "1=1"}}]),
            Blocker::ForeignItems {
                bucket: "rowFilterPolicyItems",
            },
        ),
        (
            "options",
            json!({"x": 1}),
            Blocker::UnrecognisedField {
                field: "options".into(),
            },
        ),
        (
            "isOverride",
            json!(true),
            Blocker::UnrecognisedField {
                field: "isOverride".into(),
            },
        ),
    ];
    for (field, value, want) in cases {
        let mut body = policy(&["select"]);
        body[field] = value;
        let blockers = refused(&body, EngineType::Trino);
        assert!(blockers.contains(&want), "{field}: {blockers:?}");
    }

    // Inert fields, all at once, plus the explicit defaults of the checked
    // ones and Ranger's `null`-as-default.
    let mut inert = policy(&["select"]);
    for (field, value) in [
        ("id", json!(7)),
        ("guid", json!("g")),
        ("version", json!(3)),
        ("service", json!("trino-1")),
        ("serviceType", json!("trino")),
        ("description", json!("d")),
        ("policyLabels", json!(["l"])),
        ("isAuditEnabled", json!(false)),
        ("resourceSignature", json!("sig")),
        ("createdBy", json!("admin")),
        ("updatedBy", json!("admin")),
        ("createTime", json!(1)),
        ("updateTime", json!(2)),
        ("policyType", json!(0)),
        ("policyPriority", json!(0)),
        ("isDenyAllElse", json!(false)),
        ("additionalResources", json!([])),
        ("validitySchedules", json!([])),
        ("conditions", Value::Null),
        ("zoneName", json!("")),
        ("options", json!({})),
        ("dataMaskPolicyItems", json!([])),
        ("rowFilterPolicyItems", Value::Null),
    ] {
        inert[field] = value;
    }
    grants_from_access_policy(&inert, EngineType::Trino, &lookup)
        .expect("inert and default-valued header fields change no decision");
}

/// Unknown keys BELOW the header — on an item, an access entry, a resource
/// level — are refused too: the grammar is closed at every depth.
#[test]
fn unknown_fields_at_every_depth_are_refused() {
    let mut on_item = policy(&["select"]);
    on_item["policyItems"][0]["dataMaskInfo"] = json!({"dataMaskType": "MASK"});
    assert!(
        refused(&on_item, EngineType::Trino).contains(&Blocker::UnrecognisedField {
            field: "policyItems[0].dataMaskInfo".into()
        })
    );

    let mut on_access = policy(&["select"]);
    on_access["policyItems"][0]["accesses"][0]["weight"] = json!(1);
    assert!(
        refused(&on_access, EngineType::Trino).contains(&Blocker::UnrecognisedField {
            field: "policyItems[0].accesses[0].weight".into()
        })
    );

    let mut on_level = policy(&["select"]);
    on_level["resources"]["table"]["isValidLeaf"] = json!(true);
    assert!(
        refused(&on_level, EngineType::Trino).contains(&Blocker::UnrecognisedField {
            field: "resources.table.isValidLeaf".into()
        })
    );
}

/// A body Ranger's own reader would bind differently from ours is refused,
/// not guessed at: Jackson coerces `"true"` to `true`; a reader defaulting a
/// non-bool flag to `false` would silently drop an `isExcludes`.
#[test]
fn a_body_that_does_not_read_as_a_ranger_policy_is_unreadable() {
    let mut stringly = policy(&["select"]);
    stringly["resources"]["schema"]["isExcludes"] = json!("true");
    let blockers = refused(&stringly, EngineType::Trino);
    assert!(
        blockers
            .iter()
            .any(|b| matches!(b, Blocker::Unreadable { .. })),
        "{blockers:?}"
    );
    assert!(matches!(
        refused(&json!("not an object"), EngineType::Trino)[..],
        [Blocker::Unreadable { .. }]
    ));
}

/// Principals: users and groups are carried verbatim (`public` included — the
/// compiler emits it unchanged and Ranger matches everybody by it). A Ranger
/// `role` matched nobody on the engine (D0), while the model expands a role
/// into its holders, so migrating one changes the decision. A `{USER}` macro
/// is not a username; an `@` ref is refused by the compiler itself.
#[test]
fn principals_that_do_not_round_trip_are_refused() {
    let mut ok = policy(&["select"]);
    ok["policyItems"][0]["groups"] = json!(["public", "analysts"]);
    let got = grants_from_access_policy(&ok, EngineType::Trino, &lookup).expect("users + groups");
    let include = &got.grants[0].principals.include;
    assert_eq!(include.len(), 3);
    assert_eq!(
        include
            .iter()
            .filter(|p| p.kind == PrincipalKind::Group)
            .map(|p| p.r#ref.as_str())
            .collect::<Vec<_>>(),
        vec!["public", "analysts"]
    );

    let mut role = policy(&["select"]);
    role["policyItems"][0]["roles"] = json!(["analyst"]);
    assert!(
        refused(&role, EngineType::Trino).contains(&Blocker::RolePrincipal {
            role: "analyst".into()
        })
    );

    let mut role_exc = policy(&["select"]);
    role_exc["allowExceptions"] =
        json!([{"users": [], "groups": [], "roles": ["r"], "accesses": [{"type": "select"}]}]);
    assert!(refused(&role_exc, EngineType::Trino)
        .contains(&Blocker::RolePrincipal { role: "r".into() }));

    let mut macro_user = policy(&["select"]);
    macro_user["policyItems"][0]["users"] = json!(["{USER}"]);
    assert!(
        refused(&macro_user, EngineType::Trino).contains(&Blocker::UserExpression {
            user: "{USER}".into()
        })
    );

    let mut email = policy(&["select"]);
    email["policyItems"][0]["users"] = json!(["alice@example.com"]);
    assert!(
        refused(&email, EngineType::Trino).contains(&Blocker::UnusableUserRef {
            user: "alice@example.com".into()
        })
    );

    let mut nobody = policy(&["select"]);
    nobody["policyItems"][0]["users"] = json!([]);
    assert!(refused(&nobody, EngineType::Trino).contains(&Blocker::NoPrincipals));

    let mut nobody_exc = policy(&["select"]);
    nobody_exc["allowExceptions"] = json!([item(&[], &["select"])]);
    assert!(refused(&nobody_exc, EngineType::Trino).contains(&Blocker::NoPrincipals));
}

/// Access entries: an explicit `isAllowed: false`, an empty access list, and
/// `delegateAdmin` all change what the item means and none has a model field.
/// `isAllowed` absent or `null` is Ranger's `TRUE` and is accepted.
#[test]
fn access_entry_and_item_flags_are_refused() {
    let mut not_allowed = policy(&["select"]);
    not_allowed["policyItems"][0]["accesses"][0]["isAllowed"] = json!(false);
    assert!(
        refused(&not_allowed, EngineType::Trino).contains(&Blocker::AccessNotAllowed {
            access_type: "select".into()
        })
    );

    let mut no_accesses = policy(&["select"]);
    no_accesses["policyItems"][0]["accesses"] = json!([]);
    assert!(refused(&no_accesses, EngineType::Trino).contains(&Blocker::EmptyAccesses));

    let mut delegate = policy(&["select"]);
    delegate["policyItems"][0]["delegateAdmin"] = json!(true);
    assert!(refused(&delegate, EngineType::Trino).contains(&Blocker::DelegateAdmin));

    let mut defaults = policy(&["select"]);
    defaults["policyItems"][0]["accesses"] = json!([{"type": "select"}]);
    defaults["policyItems"][0]["delegateAdmin"] = json!(false);
    defaults["policyItems"][0]["conditions"] = json!([]);
    grants_from_access_policy(&defaults, EngineType::Trino, &lookup)
        .expect("absent isAllowed is Ranger's TRUE; false delegateAdmin and [] conditions");
    defaults["policyItems"][0]["accesses"] = json!([{"type": "select", "isAllowed": null}]);
    grants_from_access_policy(&defaults, EngineType::Trino, &lookup)
        .expect("null isAllowed is Ranger's TRUE");
}

/// A resource value carrying a Ranger runtime expression is resolved per
/// request by the plugin; the model stores literals.
#[test]
fn a_resource_value_expression_is_refused() {
    let mut expr = policy(&["select"]);
    expr["resources"]["table"] = level(&["{USER}"]);
    let blockers = refused(&expr, EngineType::Trino);
    assert!(
        blockers.iter().any(|b| matches!(
            b,
            Blocker::ResourceExpression { level, value } if level == "table" && value == "{USER}"
        )),
        "{blockers:?}"
    );
}

/// The blocker list is COMPLETE, not first-error: a body with several faults
/// reports every one of them, once.
#[test]
fn every_blocker_is_collected_not_just_the_first() {
    // `alter`, not a partial expansion, is the item-level fault here: the
    // subset rule has its own test, and its mutation must fail only that one.
    let mut many = policy(&["select", "alter"]);
    many["resources"]["schema"]["isRecursive"] = json!(true);
    many["policyPriority"] = json!(5);
    many["validitySchedules"] = json!([{"startTime": "x"}]);
    many["conditions"] = json!([{"type": "a", "values": []}]);
    many["policyItems"][0]["conditions"] = json!([{"type": "b", "values": []}]);
    let blockers = refused(&many, EngineType::Trino);
    let want = vec![
        Blocker::ValiditySchedules,
        Blocker::Conditions,
        Blocker::NonDefaultPriority { priority: 5 },
        Blocker::ResourceFlag {
            level: "schema".into(),
            flag: "isRecursive",
        },
        Blocker::UnknownAccessType {
            access_type: "alter".into(),
        },
    ];
    // Order is not part of the contract; completeness and "each once" are.
    assert_eq!(
        blockers.len(),
        want.len(),
        "every fault, each once (Conditions twice in the body, once here): {blockers:?}"
    );
    for w in &want {
        assert!(blockers.contains(w), "missing {w:?} from {blockers:?}");
    }
}

/// THE DESIGN ORACLE. The grammar must be the inverse of the compiler: every
/// accepted body, compiled back grant by grant through `compile_grant` and
/// `access_to_ranger_json`, must reproduce its resources, its items and its
/// exceptions. A grammar that accepted something the compiler renders
/// differently would pass every test above.
#[test]
fn every_accepted_body_compiles_back_to_what_it_came_from() {
    let mut exception = policy(&["select", "insert", "delete"]);
    exception["policyItems"][0]["groups"] = json!(["analysts"]);
    exception["allowExceptions"] = json!([{
        "users": ["bob"], "groups": ["contractors"], "roles": [],
        "accesses": [{"type": "insert", "isAllowed": true}, {"type": "select", "isAllowed": true},
                     {"type": "delete", "isAllowed": true}]
    }]);

    let mut schema_wide = policy(&["select"]);
    schema_wide["resources"]["table"] = level(&["*"]);

    let mut column = policy(&["select"]);
    column["resources"]["column"] = level(&["email"]);

    let mut deny = policy(&["select"]);
    deny["denyPolicyItems"] = json!([item(&["dave"], &["insert", "delete"])]);
    deny["denyExceptions"] = json!([item(&["erin"], &["insert", "delete"])]);

    let mut sr = policy(&["select", "insert", "update", "delete"]);
    sr["resources"] = json!({
        "catalog": level(&["wh"]), "database": level(&["sales"]),
        "table": level(&["orders"]), "column": level(&["*"])
    });

    let mut two_cats = policy(&["select"]);
    two_cats["resources"]["catalog"] = level(&["wh", "lake"]);

    let corpus: Vec<(&str, Value, EngineType)> = vec![
        ("table", policy(&["select"]), EngineType::Trino),
        ("exception", exception, EngineType::Trino),
        ("schema-wide", schema_wide, EngineType::Trino),
        ("column", column, EngineType::Trino),
        ("allow+deny", deny, EngineType::Trino),
        ("starrocks", sr, EngineType::StarRocks),
        ("two catalogs", two_cats, EngineType::Trino),
    ];

    for (label, body, engine) in corpus {
        let got = grants_from_access_policy(&body, engine, &lookup_two)
            .unwrap_or_else(|b| panic!("{label}: refused {b:?}"));

        let n_items = body["policyItems"].as_array().map_or(0, Vec::len)
            + body["denyPolicyItems"].as_array().map_or(0, Vec::len);
        let n_cats = body["resources"]["catalog"]["values"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(got.grants.len(), n_items * n_cats, "{label}");

        for g in &got.grants {
            egret_perm_compile::validate_grant_shape(g)
                .unwrap_or_else(|e| panic!("{label}: the model refuses {g:?}: {e:?}"));
            let compiled = compile_grant(g, catalog_name(g.scope.catalog_id), "svc", engine)
                .unwrap_or_else(|e| panic!("{label}: does not compile back: {e:?}"));
            let back = access_to_ranger_json(&compiled);

            // Resources: identical, except the catalog level names this
            // grant's own catalog.
            let mut want = body["resources"].clone();
            want["catalog"]["values"] = json!([catalog_name(g.scope.catalog_id)]);
            assert_eq!(back["resources"], want, "{label}: resources");

            // The main item: the one in the body this grant came from.
            let (bucket, exc_bucket, back_bucket, back_exc) = match g.effect {
                Effect::Allow => (
                    "policyItems",
                    "allowExceptions",
                    "policyItems",
                    "allowExceptions",
                ),
                Effect::Deny => (
                    "denyPolicyItems",
                    "denyExceptions",
                    "denyPolicyItems",
                    "denyExceptions",
                ),
            };
            let back_items = back[back_bucket].as_array().unwrap();
            assert_eq!(back_items.len(), 1, "{label}");
            let source = body[bucket]
                .as_array()
                .unwrap()
                .iter()
                .find(|it| same_item(it, &back_items[0]))
                .unwrap_or_else(|| panic!("{label}: no source item matches {:?}", back_items[0]));
            assert!(same_item(source, &back_items[0]));

            let src_exc = body[exc_bucket].as_array().cloned().unwrap_or_default();
            let back_exc = back[back_exc].as_array().unwrap();
            assert_eq!(src_exc.len(), back_exc.len(), "{label}: exceptions");
            for (s, b) in src_exc.iter().zip(back_exc) {
                assert!(same_item(s, b), "{label}: exception {s} vs {b}");
            }
        }
    }
}

/// Users, groups, roles and the access-type SET agree (Ranger matches any of a
/// list, so order is not meaning).
fn same_item(a: &Value, b: &Value) -> bool {
    let set = |v: &Value, k: &str| -> BTreeSet<String> {
        v[k].as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    };
    let accesses = |v: &Value| -> BTreeSet<String> {
        v["accesses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["type"].as_str().unwrap().to_string())
            .collect()
    };
    set(a, "users") == set(b, "users")
        && set(a, "groups") == set(b, "groups")
        && set(a, "roles") == set(b, "roles")
        && accesses(a) == accesses(b)
}
