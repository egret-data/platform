//! d-i: an engine bound to a policy profile of another type. `check_profile_bindable`
//! refuses such a bind, but a binding made before that check still exists. Every
//! write that recompiles the profile must refuse it, rather than compile StarRocks
//! policy for a Trino profile (or the reverse).
//!
//! DB-gated; FAILS without a database unless `EGRET_ALLOW_DB_SKIP` is set.

use egret_metastore::Db;
use egret_perm::{CompileError, PermStore};
use egret_perm_compile::recompile_service;
use egret_policy::PolicyStore;
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

/// A profile of `profile_type`, and an engine of `engine_type` bound to it.
async fn bound_pair(db: &Db, profile_type: &str, engine_type: &str) -> (i64, String, String) {
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy)
        .await
        .expect("seed trino service-def");
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let svc = policy
        .create_service(&unique("d1_profile"), profile_type, &serde_json::json!({}))
        .await
        .expect("create profile");
    let engine_name = unique("d1_engine");
    sqlx::query(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, $2, 'egret', '{}'::jsonb, $3)",
    )
    .bind(&engine_name)
    .bind(engine_type)
    .bind(svc.id)
    .execute(&db.pool)
    .await
    .expect("bind engine to profile");
    (svc.id, svc.name, engine_name)
}

#[tokio::test]
async fn a_profile_with_an_engine_of_another_type_refuses_to_compile() {
    let Some(db) = test_db().await else { return };
    let (service_id, profile, engine) = bound_pair(&db, "trino", "starrocks").await;

    let perm = PermStore::new(db.pool.clone());
    let policy = PolicyStore::new(db.clone());
    let err = recompile_service(&perm, &policy, service_id)
        .await
        .expect_err("a cross-type binding must refuse the compile");

    match err {
        CompileError::EngineTypeMismatch {
            profile: p,
            profile_type,
            engine: e,
            engine_type,
        } => {
            assert_eq!(p, profile);
            assert_eq!(profile_type, "trino");
            assert_eq!(e, engine);
            assert_eq!(engine_type, "starrocks");
        }
        other => panic!("expected EngineTypeMismatch, got {other:?}"),
    }
}

#[tokio::test]
async fn a_profile_with_an_engine_of_its_own_type_compiles() {
    let Some(db) = test_db().await else { return };
    let (service_id, _, _) = bound_pair(&db, "trino", "trino").await;

    let perm = PermStore::new(db.pool.clone());
    let policy = PolicyStore::new(db.clone());
    recompile_service(&perm, &policy, service_id)
        .await
        .expect("a same-type binding compiles");
}
