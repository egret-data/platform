//! The store round-trips the model. The assertion that matters is the
//! principal SUBTRACT: it is the reason egret can drop Ranger's
//! allow-exception/deny-exception blocks from the authoring surface, so if it
//! does not survive a round trip, nothing above it is trustworthy.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm --test store

use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Rule,
    RuleKind, Scope, Selector,
};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

async fn seed_catalog(pool: &sqlx::PgPool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) \
         VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
    )
    .bind(unique("perm"))
    .fetch_one(pool)
    .await
    .expect("catalog")
}

async fn seed_workspace(pool: &sqlx::PgPool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, 'Perm', 'active') RETURNING id",
    )
    .bind(unique("perm-ws"))
    .fetch_one(pool)
    .await
    .expect("workspace")
}

async fn seed_tag(pool: &sqlx::PgPool) -> String {
    let key = unique("perm_tag");
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, 'Perm tag')")
        .bind(&key)
        .execute(pool)
        .await
        .expect("tag");
    key
}

#[tokio::test]
async fn grant_round_trips_with_its_principal_subtract() {
    let Some(db) = test_db().await else { return };
    let store = PermStore::new(db.pool.clone());
    let catalog_id = seed_catalog(&db.pool).await;

    let g = Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id,
            schema: Some("sales".into()),
            table: None,
            column: None,
        },
        principals: PrincipalSet {
            include: vec![Principal {
                kind: PrincipalKind::Group,
                r#ref: "finance".into(),
            }],
            exclude: vec![Principal {
                kind: PrincipalKind::Group,
                r#ref: "contractors".into(),
            }],
        },
        origin: GrantOrigin::Explicit,
    };
    let id = store.create_grant(&g).await.expect("create");

    let back = store.list_grants().await.expect("list");
    let got = back.iter().find(|x| x.id == id).expect("present");
    assert_eq!(got.principals.include.len(), 1);
    assert_eq!(
        got.principals.exclude.len(),
        1,
        "the subtract must survive the round trip"
    );
    assert_eq!(got.principals.exclude[0].r#ref, "contractors");
    assert_eq!(got.scope.schema.as_deref(), Some("sales"));
    assert!(got.scope.table.is_none());
}

#[tokio::test]
async fn a_column_scope_without_a_table_is_refused_by_the_database() {
    // The CHECK, not the Rust layer, is what makes this unrepresentable — a
    // predicate in one code path is a rule somebody later forgets.
    let Some(db) = test_db().await else { return };
    let catalog_id = seed_catalog(&db.pool).await;
    let err = sqlx::query(
        "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name, column_name) \
         VALUES ('allow', ARRAY['read'], $1, 'sales', 'email')",
    )
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .expect_err("a column with no table must be refused");
    // `grants_check` and `grants_check2` are also unnamed CHECKs on this
    // table (verified via pg_constraint against the migrated template) —
    // asserting only the "grants_check" prefix would pass if any of them
    // fired, not specifically this one. `table_name IS NOT NULL OR
    // column_name IS NULL` is `grants_check1`.
    assert!(
        err.to_string().contains("grants_check1"),
        "expected the grants_check1 CHECK violation, got: {err}"
    );
}

/// SP4d debt Task 12 — **a privilege escalation, and the control is half the
/// test.**
///
/// `perm.grants` has forbidden a level SKIP since `0154_perm_model.sql`, and
/// `a_column_scope_without_a_table_is_refused_by_the_database` above pins that.
/// `schema_name = ''` IS that skip in enforcement terms — the compiled policy
/// carries `"schema": {"values": [""]}`, `RangerAbstractResourceMatcher.init()`
/// drops an empty policy value, and the level becomes `isMatchAny`, so the grant
/// is enforced in every schema of the catalog (measured in
/// `egret-perm-compile/tests/compile_grants.rs`,
/// `an_empty_scope_level_would_be_enforced_in_every_schema`) — and
/// `IS NOT NULL` could not see it.
///
/// **The two controls, both required.** A constraint that rejected every write
/// would pass an empties-only test, so arm 1 writes the legal row the escalation
/// imitates and asserts it is still ACCEPTED. And the NULL skip is asserted to
/// still fail under its OWN constraint name, which is what proves `''` and NULL
/// are two different shapes rather than one guard answering twice: before 0178
/// the NULL insert was rejected by `grants_check` while the `''` insert was
/// accepted, on the same connection. Keeping both arms here is what makes this a
/// measurement rather than a probe that could not fail.
#[tokio::test]
async fn an_empty_scope_level_is_refused_by_the_database_and_so_is_the_null_skip() {
    let Some(db) = test_db().await else { return };
    let catalog_id = seed_catalog(&db.pool).await;

    // CONTROL 1 — the legal row the escalation imitates. If this ever fails,
    // 0178 is over-refusing and every arm below is meaningless.
    sqlx::query(
        "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name, table_name, \
         column_name) VALUES ('allow', ARRAY['read'], $1, 'sales', 'orders', 'email')",
    )
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .expect("a fully named path is the ordinary grant and must stay legal");

    // CONTROL 2 — `*` at a level is a LEGITIMATE policy value and stays legal
    // (migration 0178's own `'' AND NOT '*'` paragraph, and the plan's ruling).
    // A predicate that refused it would break a scope a steward writes on
    // purpose, so this arm is what stops 0178 over-refusing.
    //
    // ⚠ It was `('*', 'orders')` when Task 12 wrote it, and migration 0179
    // (Task 14) now refuses THAT shape — not because `*` is illegitimate but
    // because a NAMED level under a match-any one is the level skip
    // `grants_check` already forbids. The control is kept at the same level with
    // nothing named beneath it, which is the half 0178 is responsible for;
    // `a_wildcard_level_with_a_named_level_beneath_it_is_refused_by_the_database`
    // below owns the other half and keeps `('*','orders')` measured.
    sqlx::query(
        "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name) \
         VALUES ('allow', ARRAY['read'], $1, '*')",
    )
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .expect("`*` is a legitimate Ranger policy value and must stay writable");

    // THE FINDING, at each of the three levels.
    for (schema, table, column) in [
        ("", Some("orders"), None), // the escalation: a table with no schema
        ("", None, None),           // an empty tail level
        ("sales", Some(""), None),  // the same shape one level down
        ("sales", Some("orders"), Some("")),
    ] {
        let err = sqlx::query(
            "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name, table_name, \
             column_name) VALUES ('allow', ARRAY['read'], $1, $2, $3, $4)",
        )
        .bind(catalog_id)
        .bind(schema)
        .bind(table)
        .bind(column)
        .execute(&db.pool)
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("grants_scope_levels_are_named"),
            "({schema:?}, {table:?}, {column:?}) must be refused by \
             grants_scope_levels_are_named, got: {err}"
        );
    }

    // CONTROL 3 — the NULL skip, which was ALREADY refused before 0178 existed.
    // Its constraint name must still be the OLD one: if this row started failing
    // under `grants_scope_levels_are_named` instead, the two shapes would have
    // collapsed into one guard and the pair would no longer measure anything.
    let err = sqlx::query(
        "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name, table_name) \
         VALUES ('allow', ARRAY['read'], $1, NULL, 'orders')",
    )
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("\"grants_check\""),
        "a NULL schema under a named table must still be refused by `grants_check`, the \
         level-skip CHECK 0154 added — got: {err}"
    );
}

/// SP4d debt Task 14 — **the half 0178 left, and the more reachable one.**
///
/// `0178` closed `schema_name = ''`. `schema_name = '*'` earns the IDENTICAL
/// `MatchType::Self_` on `finance.orders.amount` (measured on the compiler's own
/// artifact in
/// `egret-perm-compile/tests/compile_grants.rs::an_empty_scope_level_would_be_enforced_in_every_schema`),
/// and unlike `''` it is reachable from the browser — stated by code path, not by
/// authorship: `ScopePicker`'s levels are free-text `<Input>`s whose `onChange` is
/// `e.target.value || undefined`, so `""` becomes `undefined` while `"*"` passes
/// through, and `canPickTable = !!value.schema` means typing `*` into Schema
/// ENABLES the Table field.
///
/// **The rule is NOT "`*` is illegitimate"** — it is that `grants_check` has
/// forbidden naming a table without a schema since `0154`, and `schema_name = '*'`
/// expresses precisely that, so two guards in this table disagreed about whether
/// that scope exists. Hence the accept-controls below carry as much of the test as
/// the refusals: a catalog-wide grant, a schema-wide grant, `('*','*')`,
/// `('sales','orders','*')`, a double-star tail and `'ord*'` (a PREFIX matcher,
/// which names a set of schemas rather than nothing) must all stay writable. If
/// any of them failed, the predicate would be the over-refusal the plan warned
/// about.
///
/// **`'**'` is in the refused set on purpose.** Ranger's
/// `buildResourceMatchers()` treats any pure run of `*` as match-any, so keying
/// this on the literal `'*'` would close one spelling and leave the next one
/// reaching identical enforcement from the identical field.
///
/// **CONTROL 3 is the NULL skip under its OWN constraint name.** Both `0178` and
/// `0179` are consistency fixes to `grants_check`; if a NULL schema under a named
/// table ever started failing under one of THEM, the three shapes would have
/// collapsed into one guard and none of these arms would measure anything.
#[tokio::test]
async fn a_wildcard_level_with_a_named_level_beneath_it_is_refused_by_the_database() {
    let Some(db) = test_db().await else { return };
    let catalog_id = seed_catalog(&db.pool).await;

    // THE FINDING, at every pairing of a match-any level with a named one.
    for (schema, table, column) in [
        (Some("*"), Some("orders"), None),  // the escalation, as authored
        (Some("**"), Some("orders"), None), // the same match-any, next spelling
        (Some("*"), Some("orders"), Some("amount")),
        (Some("*"), Some("*"), Some("amount")), // the gap one level down
        (Some("sales"), Some("*"), Some("amount")),
    ] {
        let err = sqlx::query(
            "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name, table_name, \
             column_name) VALUES ('allow', ARRAY['read'], $1, $2, $3, $4)",
        )
        .bind(catalog_id)
        .bind(schema)
        .bind(table)
        .bind(column)
        .execute(&db.pool)
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("grants_wildcard_levels_name_nothing_beneath"),
            "({schema:?}, {table:?}, {column:?}) names a level under a match-any one and must \
             be refused by grants_wildcard_levels_name_nothing_beneath, got: {err}"
        );
    }

    // ACCEPT-CONTROLS — every shape where a wildcard has NOTHING named beneath
    // it, plus the prefix matcher that is not a wildcard run at all. A predicate
    // that refused `*` outright would fail here, and that failure is the one the
    // plan cares about most.
    for (schema, table, column) in [
        (None, None, None),          // catalog-wide
        (Some("sales"), None, None), // schema-wide
        (Some("*"), None, None),     // every schema, nothing named under it
        (Some("*"), Some("*"), None),
        (Some("*"), Some("*"), Some("*")),
        (Some("**"), Some("***"), None), // runs of any length, still no gap
        (Some("sales"), Some("orders"), Some("*")), // every column of one table
        (Some("sales"), Some("*"), Some("*")),
        (Some("ord*"), Some("orders"), None), // StartsWith names a SET, not nothing
        (Some("sales"), Some("ord*"), Some("amount")),
        (Some("sales"), Some("orders"), Some("amount")),
    ] {
        sqlx::query(
            "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name, table_name, \
             column_name) VALUES ('allow', ARRAY['read'], $1, $2, $3, $4)",
        )
        .bind(catalog_id)
        .bind(schema)
        .bind(table)
        .bind(column)
        .execute(&db.pool)
        .await
        .unwrap_or_else(|e| {
            panic!("({schema:?}, {table:?}, {column:?}) must stay writable, got: {e}")
        });
    }

    // CONTROL 3 — the NULL skip, refused before either 0178 or 0179 existed, and
    // still refused under its OWN name.
    let err = sqlx::query(
        "INSERT INTO perm.grants (effect, actions, catalog_id, schema_name, table_name) \
         VALUES ('allow', ARRAY['read'], $1, NULL, 'orders')",
    )
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("\"grants_check\""),
        "a NULL schema under a named table must still be refused by `grants_check`, the \
         level-skip CHECK 0154 added — got: {err}"
    );
}

/// ⚠ **THIS TEST ASSERTED A REFUSAL UNTIL 2026-10-10, and 0179 no longer
/// constrains `perm.rules` at all.**
///
/// 0179 shipped `rules_wildcard_levels_name_nothing_beneath` as the twin of its
/// `perm.grants` constraint. A product decision on 2026-10-10 dropped the
/// rule-side half, so this test now pins the ACCEPT: a match-any level with a
/// named level beneath it is storable in `perm.rules`.
///
/// **Why the two tables differ.** On `perm.grants` the shape widens AUTHORITY —
/// the compiled policy is enforced in every schema of the catalog, the measured
/// escalation — and fails OPEN, so
/// `a_wildcard_level_with_a_named_level_beneath_it_is_refused_by_the_database`
/// still pins that refusal. On `perm.rules` it widens RESTRICTION: it withholds
/// more, fails CLOSED, and both plugins enforce it (every `dataMaskDef` level is
/// `"wildCard": true` on `ranger-plugins-common` 2.5.0 and 2.8.0).
/// `('sales','*','email')` is "mask `email` in every table of `sales`" — a
/// control a steward writes on purpose, which used to work.
///
/// **The controls are the point of this test, not the accept.** An insert that
/// succeeds proves nothing on its own: it would also succeed if someone dropped
/// every CHECK on the table. So this asserts, on the same connection and the same
/// columns, that the two refusals which did NOT move still fire — the `0154`
/// level-skip CHECKs for a NULL level and `0178`'s for an empty one. Only a
/// literal `*` the author typed became legal.
#[tokio::test]
async fn a_wildcard_rule_level_with_a_named_level_beneath_it_is_accepted_by_the_database() {
    let Some(db) = test_db().await else { return };
    let catalog_id = seed_catalog(&db.pool).await;

    // CONTROL — a mask on every column of one table: a wildcard with nothing
    // named beneath it, legal before the ruling and after it.
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, schema_name, \
         table_name, column_name) VALUES ($1, 'mask', 'MASK', 'path', $2, 'sales', 'orders', '*')",
    )
    .bind(unique("perm_rule"))
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .expect("a mask over every column of one table must stay legal");

    // THE RULING'S ACCEPT CASE, and the exact tuple the grant-side test refuses.
    for (schema, table, column) in [
        ("sales", "*", "email"),
        ("*", "orders", "email"),
        ("*", "orders", ""),
        ("**", "orders", "email"),
    ] {
        let column: Option<&str> = if column.is_empty() {
            None
        } else {
            Some(column)
        };
        sqlx::query(
            "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, \
             schema_name, table_name, column_name) VALUES ($1, 'mask', 'MASK', 'path', $2, $3, \
             $4, $5)",
        )
        .bind(unique("perm_rule"))
        .bind(catalog_id)
        .bind(schema)
        .bind(table)
        .bind(column)
        .execute(&db.pool)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "({schema:?}, {table:?}, {column:?}) must be STORABLE in perm.rules since \
                 2026-10-10 — 0179 constrains perm.grants only: {e}"
            )
        });
    }

    // CONTROL 1 — a NULL level is still refused, by `0154`'s `rules_check4`
    // (`table_name IS NOT NULL OR column_name IS NULL`). The ruling legalised a
    // literal `*`, not an absent level.
    let err = sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, schema_name, \
         table_name, column_name) VALUES ($1, 'mask', 'MASK', 'path', $2, 'sales', NULL, 'email')",
    )
    .bind(unique("perm_rule"))
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("rules_check"),
        "a NULL table under a named column must still be refused by `0154`'s level-skip \
         CHECK — got: {err}"
    );

    // CONTROL 2 — an EMPTY level is still refused, by `0178`. `''` is an absence
    // rather than an authored value, and that line did not move.
    let err = sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, schema_name, \
         table_name, column_name) VALUES ($1, 'mask', 'MASK', 'path', $2, '', 'orders', 'email')",
    )
    .bind(unique("perm_rule"))
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("rules_scope_levels_are_named"),
        "an EMPTY schema must still be refused by `rules_scope_levels_are_named` (0178) — \
         only a literal `*` became legal: {err}"
    );
}

/// The `perm.rules` half of 0178. Not the same severity — an empty level makes a
/// mask or row filter cover every sibling at that level, which withholds MORE
/// than the author wrote — but the same three columns, the same verbatim copy
/// into `scope_to_resources`, and the same predicate. Measured accepted before
/// 0178.
#[tokio::test]
async fn an_empty_rule_scope_level_is_refused_by_the_database() {
    let Some(db) = test_db().await else { return };
    let catalog_id = seed_catalog(&db.pool).await;

    // CONTROL — the legal row.
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, schema_name, \
         table_name, column_name) VALUES ($1, 'mask', 'MASK', 'path', $2, 'sales', 'orders', \
         'email')",
    )
    .bind(unique("perm_rule"))
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .expect("a fully named path mask must stay legal");

    let err = sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, schema_name, \
         table_name, column_name) VALUES ($1, 'mask', 'MASK', 'path', $2, '', 'orders', 'email')",
    )
    .bind(unique("perm_rule"))
    .bind(catalog_id)
    .execute(&db.pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("rules_scope_levels_are_named"),
        "an empty schema on a path rule must be refused by rules_scope_levels_are_named, \
         got: {err}"
    );
}

#[tokio::test]
async fn rule_round_trips_with_its_exempt_subtract() {
    // The symmetric twin of `grant_round_trips_with_its_principal_subtract`:
    // `Rule.exempt` is typed as a `PrincipalSet` exactly like
    // `Grant.principals`, and `perm.rule_exemptions` only exists (with its
    // `mode` column, added on review) because that subtract must round-trip
    // here too. Also exercises `RuleKind::Mask` and `Selector::Path`, and
    // `PrincipalKind::WorkspaceMembers`/`Role`/`User` on the exempt set.
    let Some(db) = test_db().await else { return };
    let store = PermStore::new(db.pool.clone());
    let catalog_id = seed_catalog(&db.pool).await;
    let workspace_id = seed_workspace(&db.pool).await;

    let r = Rule {
        id: Uuid::nil(),
        name: unique("mask_email"),
        kind: RuleKind::Mask {
            mask_type: "MASK_HASH".into(),
            value_expr: Some("sha256(email)".into()),
        },
        selector: Selector::Path(Scope {
            catalog_id,
            schema: Some("sales".into()),
            table: Some("orders".into()),
            column: Some("email".into()),
        }),
        exempt: PrincipalSet {
            include: vec![
                Principal {
                    kind: PrincipalKind::Role,
                    r#ref: "data_steward".into(),
                },
                Principal {
                    kind: PrincipalKind::WorkspaceMembers,
                    r#ref: workspace_id.to_string(),
                },
            ],
            exclude: vec![Principal {
                kind: PrincipalKind::User,
                r#ref: "contractor_bob".into(),
            }],
        },
    };
    let id = store.create_rule(&r).await.expect("create");

    let back = store.list_rules().await.expect("list");
    let got = back.iter().find(|x| x.id == id).expect("present");
    assert_eq!(got.exempt.include.len(), 2);
    assert_eq!(
        got.exempt.exclude.len(),
        1,
        "the exempt subtract must survive the round trip"
    );
    assert_eq!(got.exempt.exclude[0].r#ref, "contractor_bob");
    match &got.kind {
        RuleKind::Mask {
            mask_type,
            value_expr,
        } => {
            assert_eq!(mask_type, "MASK_HASH");
            assert_eq!(value_expr.as_deref(), Some("sha256(email)"));
        }
        other => panic!("expected RuleKind::Mask, got {other:?}"),
    }
    match &got.selector {
        Selector::Path(scope) => {
            assert_eq!(scope.catalog_id, catalog_id);
            assert_eq!(scope.column.as_deref(), Some("email"));
        }
        other => panic!("expected Selector::Path, got {other:?}"),
    }
}

#[tokio::test]
async fn grants_one_per_assignment_refuses_a_second_implicit_grant() {
    // `grants_one_per_assignment` is what stops a workspace assignment from
    // multiplying grants: a second implicit grant for the same
    // (catalog, workspace) must be refused by the database, not merely
    // discouraged by convention. Also exercises `GrantOrigin::WorkspaceAssignment`,
    // `Effect::Deny` and `Action::Write`, none of which the round-trip test covers.
    let Some(db) = test_db().await else { return };
    let store = PermStore::new(db.pool.clone());
    let catalog_id = seed_catalog(&db.pool).await;
    let workspace_id = seed_workspace(&db.pool).await;

    let implicit_grant = |effect: Effect| Grant {
        id: Uuid::nil(),
        effect,
        actions: vec![Action::Write],
        scope: Scope {
            catalog_id,
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet::default(),
        origin: GrantOrigin::WorkspaceAssignment { workspace_id },
    };

    store
        .create_grant(&implicit_grant(Effect::Deny))
        .await
        .expect("first implicit grant for this (catalog, workspace) must succeed");

    let err = store
        .create_grant(&implicit_grant(Effect::Deny))
        .await
        .expect_err("a second implicit grant for the same (catalog, workspace) must be refused");
    assert!(
        err.to_string().contains("grants_one_per_assignment"),
        "expected the grants_one_per_assignment unique-index violation, got: {err}"
    );
}

#[tokio::test]
async fn delete_grant_and_delete_rule_remove_their_rows() {
    // Neither delete_* method is exercised by the tests above. Also covers
    // `RuleKind::RowFilter` and `Selector::Tag`, the two model variants no
    // other test in this file reaches.
    let Some(db) = test_db().await else { return };
    let store = PermStore::new(db.pool.clone());
    let catalog_id = seed_catalog(&db.pool).await;
    let tag_key = seed_tag(&db.pool).await;

    let g = Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id,
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet {
            include: vec![Principal {
                kind: PrincipalKind::User,
                r#ref: "admin".into(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    let grant_id = store.create_grant(&g).await.expect("create grant");
    assert!(store.delete_grant(grant_id).await.expect("delete grant"));
    assert!(
        !store
            .list_grants()
            .await
            .expect("list")
            .iter()
            .any(|x| x.id == grant_id),
        "deleted grant must not be listed"
    );
    assert!(
        !store.delete_grant(grant_id).await.expect("delete again"),
        "deleting an already-deleted grant returns false, not an error"
    );

    let r = Rule {
        id: Uuid::nil(),
        name: unique("row_filter_rule"),
        kind: RuleKind::RowFilter {
            filter_expr: "region = current_user_region()".into(),
        },
        selector: Selector::Tag { key: tag_key },
        exempt: PrincipalSet::default(),
    };
    let rule_id = store.create_rule(&r).await.expect("create rule");
    assert!(store.delete_rule(rule_id).await.expect("delete rule"));
    assert!(
        !store
            .list_rules()
            .await
            .expect("list")
            .iter()
            .any(|x| x.id == rule_id),
        "deleted rule must not be listed"
    );
    assert!(
        !store.delete_rule(rule_id).await.expect("delete again"),
        "deleting an already-deleted rule returns false, not an error"
    );
}
