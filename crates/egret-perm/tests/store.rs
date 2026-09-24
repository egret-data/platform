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
