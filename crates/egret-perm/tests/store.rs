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
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Scope,
};
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn seed_catalog(pool: &sqlx::PgPool) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) \
         VALUES ($1, 'generic', '{}'::jsonb) RETURNING id",
    )
    .bind(format!(
        "perm_{}",
        &Uuid::new_v4().simple().to_string()[..12]
    ))
    .fetch_one(pool)
    .await
    .expect("catalog")
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
    assert!(
        err.to_string().contains("grants_check") || err.to_string().contains("violates check"),
        "expected a CHECK violation, got: {err}"
    );
}
