//! `CatalogStore::delete_warehouse_releasing_catalog_with_compiler` (SP4a
//! Task 16, D3 defect 3): a warehouse teardown cascades away
//! `perm.grants`/`perm.compiled` for its catalog, but NOT the
//! `policy.authz_policies` row those rows once compiled — that row survives
//! until some later, unrelated recompile sweeps it, which is the same shape
//! Task 7 fixed once already ("a deleted grant's policy enforced forever").
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-catalog-engine --test warehouse_delete_recompile
//!
//! `delete_warehouse_releasing_catalog`'s own guard refuses the delete while
//! any engine still attaches the catalog, so the "services this catalog
//! reaches NOW" derivation (`egret_policy::services_for_catalog_via`) is
//! always empty by the time this function does anything — recompiling THAT
//! set would find nothing wrong. What this fixture recreates is the case
//! that IS wrong: an engine reached this catalog in the past, compiled a
//! policy for it, and was detached WITHOUT a recompile (`RegistryStore::delete_attachment`
//! directly, bypassing `egret-engine-mgmt::catalog_apply::detach_from_engine`'s
//! own recompile hook — realistic, since that hook is fire-and-forget by
//! contract and can fail silently). The stale `policy.authz_policies` row is
//! asserted present BEFORE the delete and gone AFTER it, with the compiler
//! wired — and, in a second test, still present when it is not, so the
//! assertion is provably about THIS fix and not some other cleanup path.

use egret_catalog_engine::CatalogStore;
use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Scope,
};
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

/// A real `CatalogAssignmentCompiler` — the same construction `egret-bin`'s
/// `ProfileCompilerImpl` wires in production, minus the unrelated
/// `recompile_profile` method that port doesn't need.
struct RealCompiler;

#[async_trait::async_trait]
impl egret_catalog_registry::CatalogAssignmentCompiler for RealCompiler {
    async fn recompile_catalog_assignment_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        catalog_id: Uuid,
    ) -> Result<(), egret_perm::CompileError> {
        let mut ids = egret_policy::services_for_catalog_via(tx, catalog_id)
            .await
            .map_err(|e| egret_perm::CompileError::Db(e.to_string()))?;
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() {
            return Ok(());
        }
        egret_perm_compile::recompile_services_in_tx(tx, &ids).await?;
        Ok(())
    }

    async fn recompile_services_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        service_ids: &[i64],
    ) -> Result<(), egret_perm::CompileError> {
        if service_ids.is_empty() {
            return Ok(());
        }
        egret_perm_compile::recompile_services_in_tx(tx, service_ids).await?;
        Ok(())
    }
}

struct Fixture {
    pool: sqlx::PgPool,
    warehouse_id: Uuid,
    catalog_id: Uuid,
    service_id: i64,
}

/// A Trino profile with one engine bound to it, one registry catalog
/// (warehouse-shaped, matching what `ensure_warehouse_catalog` itself would
/// produce) attached to that engine, and one EXPLICIT grant scoped to the
/// catalog — compiled once during setup so the fixture starts with a real
/// `policy.authz_policies` row, not a hand-inserted stand-in. Deliberately
/// does NOT go through `ensure_warehouse_catalog`'s own implicit
/// `workspace_assignment` grant (that principal is `workspace_members`,
/// expanded only against real workspace membership — D3/D7 — which this
/// fixture has no need to seed): an ordinary `User` grant cascades away
/// identically, and is what this test is actually about.
async fn seed_attached_and_compiled(db: &Db) -> Fixture {
    let pool = db.pool.clone();
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(pool.clone());

    egret_policy::seed_trino(&policy)
        .await
        .expect("seed trino service-def");
    let service = policy
        .create_service(&unique("trino_wd"), "trino", &serde_json::json!({}))
        .await
        .expect("create service");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("wd-ws"))
            .bind("Warehouse Delete WS")
            .fetch_one(&pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-wd-test','AKIATEST') RETURNING id",
    )
    .bind(unique("wd-sc"))
    .fetch_one(&pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("wd_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(&pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret-engines','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("wd-eng"))
    .bind(service.id)
    .fetch_one(&pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("wd_cat"))
    .bind(warehouse_id)
    .fetch_one(&pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(&pool)
    .await
    .expect("attach catalog");

    let grant = Grant {
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
                r#ref: unique("wd-user"),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    perm.create_grant(&grant).await.expect("create grant");

    let summary = egret_perm_compile::recompile_service(&perm, &policy, service.id)
        .await
        .expect("initial recompile");
    assert_eq!(
        summary.created, 1,
        "the fixture's one explicit grant must compile to exactly one policy"
    );

    Fixture {
        pool,
        warehouse_id,
        catalog_id,
        service_id: service.id,
    }
}

/// Scoped read: every `perm-grant-*` policy this fixture's own service_id
/// carries — never an unscoped count (this workspace's test-db design shares
/// one process-private database across concurrently-running `#[tokio::test]`s
/// in a binary, per `egret-perm-compile/tests/recompile.rs`'s own doc).
async fn compiler_owned_policy_count(pool: &sqlx::PgPool, service_id: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM policy.authz_policies \
          WHERE service_id = $1 AND name LIKE $2 || '%'",
    )
    .bind(service_id)
    .bind(egret_policy::COMPILER_GRANT_PREFIX)
    .fetch_one(pool)
    .await
    .expect("count compiler-owned policies")
}

/// Deleting a warehouse whose catalog's grant compiled a policy on a profile
/// it has since been DETACHED from (with no recompile in between — the
/// window a fire-and-forget detach hook can leave open) sweeps that stale
/// compiled policy immediately, with the compiler wired.
#[tokio::test]
async fn delete_warehouse_sweeps_a_stale_compiled_policy_with_the_compiler_wired() {
    let Some(db) = test_db().await else { return };
    let f = seed_attached_and_compiled(&db).await;
    assert_eq!(
        compiler_owned_policy_count(&f.pool, f.service_id).await,
        1,
        "setup must leave exactly one compiled policy before the detach"
    );

    // Detach WITHOUT recompiling — the realistic precondition
    // `delete_warehouse_releasing_catalog`'s own guard requires (zero
    // attachments) but that a real detach-time recompile failure can also
    // reach.
    let registry = egret_catalog_registry::RegistryStore::new(db.clone());
    let engine_id: Uuid = sqlx::query_scalar(
        "SELECT engine_id FROM registry.engine_catalog_attachments WHERE catalog_id = $1",
    )
    .bind(f.catalog_id)
    .fetch_one(&f.pool)
    .await
    .expect("read attachment");
    registry
        .delete_attachment(engine_id, f.catalog_id)
        .await
        .expect("detach without recompiling");
    assert_eq!(
        compiler_owned_policy_count(&f.pool, f.service_id).await,
        1,
        "the stale policy is still there right after an unrecompiled detach — \
         this is the window the fix closes, not something detach itself fixes"
    );

    let store = CatalogStore::new(db.clone());
    store
        .delete_warehouse_releasing_catalog_with_compiler(f.warehouse_id, Some(&RealCompiler))
        .await
        .expect("delete warehouse");

    assert_eq!(
        compiler_owned_policy_count(&f.pool, f.service_id).await,
        0,
        "the warehouse delete's own recompile must sweep the now-orphaned \
         compiled policy immediately, with no manual compile step"
    );
    let warehouse_gone: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM catalog.warehouses WHERE id = $1")
            .bind(f.warehouse_id)
            .fetch_optional(&f.pool)
            .await
            .expect("read warehouse");
    assert!(warehouse_gone.is_none());
    let grants_gone: i64 =
        sqlx::query_scalar("SELECT count(*) FROM perm.grants WHERE catalog_id = $1")
            .bind(f.catalog_id)
            .fetch_one(&f.pool)
            .await
            .expect("count grants");
    assert_eq!(
        grants_gone, 0,
        "the catalog's grant must have cascaded away"
    );
}

/// The control case: without a compiler wired, `delete_warehouse_releasing_catalog`
/// behaves exactly as it always has — the warehouse and its grant still go,
/// but the stale compiled policy is left for a later recompile, same as
/// before this task. Proves the sweep in the test above is attributable to
/// the NEW code, not to the cascade alone.
#[tokio::test]
async fn delete_warehouse_leaves_the_stale_policy_when_no_compiler_is_wired() {
    let Some(db) = test_db().await else { return };
    let f = seed_attached_and_compiled(&db).await;

    let registry = egret_catalog_registry::RegistryStore::new(db.clone());
    let engine_id: Uuid = sqlx::query_scalar(
        "SELECT engine_id FROM registry.engine_catalog_attachments WHERE catalog_id = $1",
    )
    .bind(f.catalog_id)
    .fetch_one(&f.pool)
    .await
    .expect("read attachment");
    registry
        .delete_attachment(engine_id, f.catalog_id)
        .await
        .expect("detach without recompiling");

    let store = CatalogStore::new(db.clone());
    store
        .delete_warehouse_releasing_catalog(f.warehouse_id)
        .await
        .expect("delete warehouse");

    assert_eq!(
        compiler_owned_policy_count(&f.pool, f.service_id).await,
        1,
        "with no compiler wired, the stale policy is left behind — this is \
         the pre-existing behaviour the fix above changes, not a regression \
         introduced by it"
    );
}
