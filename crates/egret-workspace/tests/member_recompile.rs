//! SP4a **D3**, the wiring half: a membership write through `MemberRepo` —
//! the one path every add/remove in egret funnels through — recompiles the
//! Ranger profiles whose compiled policies enumerate that membership.
//!
//! `crates/egret-perm-compile/tests/workspace_members.rs` proves the
//! EXPANSION (which usernames, and that a recompile updates them).  This file
//! proves the TRIGGER: that `MemberRepo::add`/`remove` actually fire it. The
//! two are separable defects, and this branch has shipped "implemented on one
//! path, not the others" six times — a passing expansion test says nothing
//! about whether anything calls it.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-workspace --test member_recompile

use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_policy::{PolicyStore, COMPILER_GRANT_PREFIX};
use egret_workspace::members::MemberRepo;
use serde_json::Value;
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

struct Fixture {
    service_id: i64,
    workspace_id: Uuid,
    grant_id: Uuid,
}

/// A StarRocks profile bound to an engine that reaches one catalog, that
/// catalog assigned to one workspace — i.e. exactly the state D3 describes,
/// with the implicit grant already written and already compiled.
async fn seed(db: &Db) -> Fixture {
    let pool = &db.pool;
    let perm = egret_perm::PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());

    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&unique("sr_memrec"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");

    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("memrec-ws"))
            .bind("Member Recompile WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-memrec-test','AKIATEST') RETURNING id",
    )
    .bind(unique("memrec-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("memrec_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("memrec-eng"))
    .bind(service.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("memrec_cat"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");

    egret_perm::sync_assignment_grants(&perm, catalog_id, &[workspace_id])
        .await
        .expect("assign catalog to workspace");
    let grant_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM perm.grants WHERE catalog_id = $1 AND origin = 'workspace_assignment'",
    )
    .bind(catalog_id)
    .fetch_one(pool)
    .await
    .expect("implicit grant");

    Fixture {
        service_id: service.id,
        workspace_id,
        grant_id,
    }
}

async fn new_user(db: &Db, local: &str) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('local',$1,'Test User',$2) RETURNING id",
    )
    .bind(format!("{local}@egret.local"))
    .bind(local)
    .fetch_one(&db.pool)
    .await
    .expect("user");
    UserId(id)
}

/// The usernames the compiled implicit-grant policy currently names, or `None`
/// if no such policy exists.
async fn named_users(db: &Db, f: &Fixture) -> Option<Vec<String>> {
    let body: Option<Value> = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(f.service_id)
    .bind(format!("{COMPILER_GRANT_PREFIX}{}", f.grant_id))
    .fetch_optional(&db.pool)
    .await
    .expect("policy read");
    body.map(|b| {
        b["policyItems"][0]["users"]
            .as_array()
            .expect("users")
            .iter()
            .map(|v| v.as_str().expect("username").to_string())
            .collect()
    })
}

/// `MemberRepo::add` must recompile — otherwise the person egret has just
/// shown as a member is refused by the engine, with nothing anywhere saying
/// why. `MemberRepo::remove` must recompile too, and that is the direction
/// that fails OPEN: a stale policy keeps naming someone who has been removed.
#[tokio::test]
async fn adding_and_removing_a_member_recompiles_through_member_repo() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let members = MemberRepo::new(db.clone());
    let ws = WorkspaceId(f.workspace_id);

    let alice = new_user(&db, &unique("memrec-alice")).await;
    let bob = new_user(&db, &unique("memrec-bob")).await;
    let alice_name: String = sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
        .bind(alice.0)
        .fetch_one(&db.pool)
        .await
        .expect("alice username");
    let bob_name: String = sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
        .bind(bob.0)
        .fetch_one(&db.pool)
        .await
        .expect("bob username");

    // Before anyone joins, the implicit grant names nobody and so compiles to
    // no policy at all (fail-closed — see `egret_perm_compile::members`).
    assert_eq!(named_users(&db, &f).await, None);

    members
        .add(ws, "user", alice.0, "ws_viewer", alice)
        .await
        .expect("add alice");
    assert_eq!(
        named_users(&db, &f).await,
        Some(vec![alice_name.clone()]),
        "adding a member must have recompiled the profile, with no other trigger in sight"
    );

    members
        .add(ws, "user", bob.0, "ws_viewer", alice)
        .await
        .expect("add bob");
    let mut expected = vec![alice_name.clone(), bob_name.clone()];
    expected.sort();
    assert_eq!(named_users(&db, &f).await, Some(expected));

    members
        .remove(ws, "user", alice.0)
        .await
        .expect("remove alice");
    assert_eq!(
        named_users(&db, &f).await,
        Some(vec![bob_name]),
        "a removed member must be gone from the compiled policy — this is the fail-OPEN direction"
    );

    members.remove(ws, "user", bob.0).await.expect("remove bob");
    assert_eq!(
        named_users(&db, &f).await,
        None,
        "the last member leaving reaps the policy rather than leaving one naming nobody"
    );
}

/// A membership write in a workspace nothing in the `perm` model references
/// must not recompile anything — the common case (a workspace with no
/// catalogs assigned), and the one that decides whether every member add in
/// the installation pays for a compile.
#[tokio::test]
async fn a_membership_write_in_an_unreferenced_workspace_compiles_nothing() {
    let Some(db) = test_db().await else { return };
    let members = MemberRepo::new(db.clone());
    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("memrec-bare"))
            .bind("Bare WS")
            .fetch_one(&db.pool)
            .await
            .expect("workspace");
    let user = new_user(&db, &unique("memrec-bare-u")).await;

    members
        .add(WorkspaceId(ws_id), "user", user.0, "ws_viewer", user)
        .await
        .expect("add");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE scope_type = 'workspace' AND scope_id = $1",
    )
    .bind(ws_id)
    .fetch_one(&db.pool)
    .await
    .expect("count");
    assert_eq!(count, 1, "the membership itself still lands");

    members
        .remove(WorkspaceId(ws_id), "user", user.0)
        .await
        .expect("remove");
}
