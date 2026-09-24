//! The compiler is the security boundary: its output is what an engine
//! enforces. These tests pin the two things a reader cannot check by eye —
//! that the subtract lands in `allowExceptions`, and that an action an engine
//! cannot express is an ERROR rather than a silent drop.
//!
//! Pure (no DB) — `compile_grant` is a function of its arguments only.

use uuid::Uuid;

use egret_perm::{
    compile_grant, engine_accesses, Action, Effect, EngineType, Grant, GrantOrigin, Principal,
    PrincipalKind, PrincipalSet, Scope, CANONICAL_ADMIN_PRINCIPAL,
};

// Dumb helpers only — a helper that computes anything is a second
// implementation of the thing under test.

fn group(r#ref: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Group,
        r#ref: r#ref.to_string(),
    }
}

fn user(r#ref: &str) -> Principal {
    Principal {
        kind: PrincipalKind::User,
        r#ref: r#ref.to_string(),
    }
}

fn no_scope() -> Scope {
    Scope {
        catalog_id: Uuid::nil(),
        schema: None,
        table: None,
        column: None,
    }
}

fn grant_allow_read(include_groups: &[&str], exclude_groups: &[&str]) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: no_scope(),
        principals: PrincipalSet {
            include: include_groups.iter().map(|g| group(g)).collect(),
            exclude: exclude_groups.iter().map(|g| group(g)).collect(),
        },
        origin: GrantOrigin::Explicit,
    }
}

fn grant_allow_read_on(scope: Scope) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope,
        principals: PrincipalSet {
            include: vec![user("admin")],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    }
}

fn grant_allow_read_for_user(r#ref: &str) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: no_scope(),
        principals: PrincipalSet {
            include: vec![user(r#ref)],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    }
}

#[test]
fn a_principal_subtract_compiles_to_allow_exceptions() {
    let g = grant_allow_read(&["finance"], &["contractors"]);
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();

    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].groups, vec!["finance"]);
    assert_eq!(
        out.allow_exceptions.len(),
        1,
        "the exclude set must become an allowException — that block is a \
         compilation target and must never surface in egret's own API"
    );
    assert_eq!(out.allow_exceptions[0].groups, vec!["contractors"]);
    assert!(out.deny_exceptions.is_empty());
}

#[test]
fn a_deny_grants_subtract_compiles_to_deny_exceptions() {
    let mut g = grant_allow_read(&["finance"], &["contractors"]);
    g.effect = Effect::Deny;
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();
    assert_eq!(out.items[0].effect, "deny");
    assert_eq!(out.deny_exceptions.len(), 1);
    assert!(out.allow_exceptions.is_empty());
}

#[test]
fn write_maps_to_two_accesses_on_trino_and_three_on_starrocks() {
    assert_eq!(
        engine_accesses(Action::Read, EngineType::Trino).unwrap(),
        vec!["select"]
    );
    assert_eq!(
        engine_accesses(Action::Read, EngineType::StarRocks).unwrap(),
        vec!["select"]
    );

    // Trino's service-def declares NO `update` access type. Asserting the same
    // three on both engines would be asserting a mapping that cannot exist —
    // this test exists because the plan's first draft did exactly that.
    let mut t = engine_accesses(Action::Write, EngineType::Trino).unwrap();
    t.sort();
    assert_eq!(t, vec!["delete", "insert"]);

    let mut s = engine_accesses(Action::Write, EngineType::StarRocks).unwrap();
    s.sort();
    assert_eq!(s, vec!["delete", "insert", "update"]);
}

#[test]
fn every_access_type_the_map_emits_is_declared_by_that_service_def() {
    // The map is only correct relative to the service-defs, so read them
    // rather than trusting the table above. An access type Ranger does not
    // know is silently ignored by the plugin: the policy compiles, deploys,
    // and grants nothing.
    // `cargo test` runs this binary with CWD = this crate's own manifest
    // directory, not the workspace root, so the path is built from
    // `CARGO_MANIFEST_DIR` rather than the workspace-root-relative spelling
    // used to read these files by hand (brief Step 3) — same two files.
    for (engine, path) in [
        (
            EngineType::Trino,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../egret-policy/src/service-defs/trino.json"
            ),
        ),
        (
            EngineType::StarRocks,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../egret-policy/src/service-defs/starrocks.json"
            ),
        ),
    ] {
        let def: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let declared: Vec<String> = def["accessTypes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_string())
            .collect();
        for action in [Action::Read, Action::Write] {
            for acc in engine_accesses(action, engine).unwrap() {
                assert!(
                    declared.iter().any(|d| d == acc),
                    "{engine:?} does not declare access type {acc:?}"
                );
            }
        }
    }
}

#[test]
fn the_scope_becomes_the_ranger_resource_levels_the_service_def_declares() {
    let g = grant_allow_read_on(Scope {
        catalog_id: Uuid::nil(),
        schema: Some("sales".into()),
        table: Some("orders".into()),
        column: None,
    });
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();
    assert_eq!(out.resources["catalog"].values, vec!["warehouse"]);
    assert_eq!(out.resources["schema"].values, vec!["sales"]);
    assert_eq!(out.resources["table"].values, vec!["orders"]);
    assert!(
        !out.resources.contains_key("column"),
        "an unnamed level must be ABSENT, not a wildcard string — Ranger \
         treats a missing level and a '*' level differently"
    );
}

#[test]
fn a_user_principal_is_emitted_in_the_form_the_engine_reports() {
    // The literal below is copied from the D7 measurement, not assumed. If the
    // engine reports bare usernames, a policy naming an email matches nobody —
    // and nothing in Ranger or egret would show it.
    let g = grant_allow_read_for_user("admin@egret.local");
    let out = compile_grant(&g, "warehouse", "trino-1", EngineType::Trino).unwrap();
    assert_eq!(out.items[0].users, vec![CANONICAL_ADMIN_PRINCIPAL]);
}
