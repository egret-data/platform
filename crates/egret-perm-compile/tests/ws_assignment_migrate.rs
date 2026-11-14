//! SP3c-3: workspace members -> managed `ws:<slug> members` groups.

use egret_metastore::Db;
use egret_perm_compile::ws_members_group::{sync_ws_members_group_in_tx, ws_members_group_name};
use uuid::Uuid;

/// The suite shares one database; serialise so each test's own workspace is
/// the only thing it asserts on.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn seed_user(pool: &sqlx::PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.users (id, provider, email, name) VALUES ($1,'local',$2,'T')")
        .bind(id)
        .bind(format!("wm_{id}@example.test"))
        .execute(pool)
        .await
        .expect("insert user");
    id
}

async fn seed_workspace(pool: &sqlx::PgPool) -> (Uuid, String) {
    let slug = format!("wm-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO public.workspaces (slug, name) VALUES ($1,$1) RETURNING id",
    )
    .bind(&slug)
    .fetch_one(pool)
    .await
    .expect("insert workspace");
    (id, slug)
}

async fn add_member(pool: &sqlx::PgPool, ws: Uuid, user: Uuid) {
    sqlx::query(
        "INSERT INTO public.access_grants (principal_type, principal_id, scope_type, scope_id, effect) \
         VALUES ('user',$1,'workspace',$2,'allow')",
    )
    .bind(user)
    .bind(ws)
    .execute(pool)
    .await
    .expect("insert member grant");
}

async fn group_members(pool: &sqlx::PgPool, name: &str) -> (String, Vec<Uuid>) {
    let (gid, source): (Uuid, String) =
        sqlx::query_as("SELECT id, source FROM public.groups WHERE name = $1")
            .bind(name)
            .fetch_one(pool)
            .await
            .expect("group exists");
    let mut users: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM public.user_groups WHERE group_id = $1")
            .bind(gid)
            .fetch_all(pool)
            .await
            .expect("members");
    users.sort();
    (source, users)
}

async fn sync(pool: &sqlx::PgPool, ws: Uuid) {
    let mut tx = pool.begin().await.expect("begin");
    sync_ws_members_group_in_tx(&mut tx, ws)
        .await
        .expect("sync");
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn the_ws_members_group_mirrors_the_workspace_members() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let (ws, slug) = seed_workspace(&db.pool).await;
    let u = seed_user(&db.pool).await;
    add_member(&db.pool, ws, u).await;

    sync(&db.pool, ws).await;

    let (source, users) = group_members(&db.pool, &ws_members_group_name(&slug)).await;
    assert_eq!(source, "managed");
    assert_eq!(users, vec![u]);
}

#[tokio::test]
async fn re_syncing_reconciles_added_and_removed_members() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let (ws, slug) = seed_workspace(&db.pool).await;
    let u1 = seed_user(&db.pool).await;
    let u2 = seed_user(&db.pool).await;
    add_member(&db.pool, ws, u1).await;
    sync(&db.pool, ws).await;
    let name = ws_members_group_name(&slug);
    assert_eq!(group_members(&db.pool, &name).await.1, vec![u1]);

    // Add u2, remove u1, then sync again (twice, to prove idempotence).
    add_member(&db.pool, ws, u2).await;
    sqlx::query(
        "DELETE FROM public.access_grants WHERE principal_id = $1 AND scope_type = 'workspace' AND scope_id = $2",
    )
    .bind(u1)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("remove member");
    sync(&db.pool, ws).await;
    sync(&db.pool, ws).await;

    assert_eq!(group_members(&db.pool, &name).await.1, vec![u2]);
}

/// A user joining or leaving a group that HOLDS the workspace changes the
/// workspace's members; the live group-membership path (`PermGroupRecompiler`)
/// must re-sync the managed `ws:<slug> members` group, in both directions.
#[tokio::test]
async fn a_group_membership_change_resyncs_the_ws_members_group_via_the_live_path() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let (ws, slug) = seed_workspace(&db.pool).await;
    let u = seed_user(&db.pool).await;
    let name = ws_members_group_name(&slug);

    // A local group that holds the workspace.
    let repo = egret_auth::groups::GroupRepo::new(db.clone());
    let holder_name = format!("wm-holder-{}", &Uuid::new_v4().simple().to_string()[..10]);
    let holder = repo
        .create_local(&holder_name, &egret_perm_compile::PermGroupRecompiler)
        .await
        .expect("create group");
    sqlx::query(
        "INSERT INTO public.access_grants (principal_type, principal_id, role_id, scope_type, scope_id, effect) \
         SELECT 'group', $1, r.id, 'workspace', $2, 'allow' FROM roles r \
          WHERE r.scope = 'workspace' AND r.name = 'ws_viewer'",
    )
    .bind(holder)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("group holds the workspace");
    sync(&db.pool, ws).await;
    assert!(group_members(&db.pool, &name).await.1.is_empty());

    repo.add_member(
        holder,
        egret_core::UserId(u),
        &egret_perm_compile::PermGroupRecompiler,
    )
    .await
    .expect("add member");
    assert_eq!(group_members(&db.pool, &name).await.1, vec![u]);

    repo.remove_member(
        holder,
        egret_core::UserId(u),
        &egret_perm_compile::PermGroupRecompiler,
    )
    .await
    .expect("remove member");
    assert!(
        group_members(&db.pool, &name).await.1.is_empty(),
        "a user leaving the holding group must leave the managed group (fail-closed)"
    );
}

#[tokio::test]
async fn the_ws_assignment_migration_table_records_a_run() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let catalog = Uuid::new_v4();
    let grant = Uuid::new_v4();
    let ws = Uuid::new_v4();
    let original = serde_json::json!([{
        "grant_id": grant,
        "origin": "workspace_assignment",
        "origin_workspace_id": ws,
        "principal_kind": "workspace_members",
        "principal_ref": ws.to_string(),
    }]);

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO perm.ws_assignment_migration \
         (catalogs, grant_ids, principal_hashes, original, migrated_by) \
         VALUES ($1,$2,$3,$4,$5) RETURNING id",
    )
    .bind(vec![catalog])
    .bind(vec![grant])
    .bind(vec!["hash-a".to_string()])
    .bind(&original)
    .bind("tester")
    .fetch_one(&db.pool)
    .await
    .expect("insert run");

    let (catalogs, grant_ids, hashes, back, by, has_ts): (
        Vec<Uuid>,
        Vec<Uuid>,
        Vec<String>,
        serde_json::Value,
        String,
        bool,
    ) = sqlx::query_as(
        "SELECT catalogs, grant_ids, principal_hashes, original, migrated_by, \
                migrated_at IS NOT NULL \
         FROM perm.ws_assignment_migration WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&db.pool)
    .await
    .expect("read run");

    assert_eq!(catalogs, vec![catalog]);
    assert_eq!(grant_ids, vec![grant]);
    assert_eq!(hashes, vec!["hash-a".to_string()]);
    assert_eq!(back, original);
    assert_eq!(by, "tester");
    assert!(has_ts, "migrated_at defaults to now()");

    sqlx::query("DELETE FROM perm.ws_assignment_migration WHERE id = $1")
        .bind(id)
        .execute(&db.pool)
        .await
        .expect("cleanup");
}

// ---------------------------------------------------------------------------
// SP3c-3 Task 4: the reversible, equivalence-gated workspace-assignment ->
// `ws:<slug> members` group rewrite. `migrate_ws_assignments_in_tx` is global
// (every service, every implicit grant) and its equivalence gate checks the
// whole affected profile set, so these tests take the file's SERIAL mutex and
// run every migrate/undo inside ONE transaction that is rolled back — nothing
// is committed, so no other test's catalog/workspace is touched. Assertions are
// scoped to this test's own grant id, never a global count.

use egret_perm_compile::recompile_service_in_tx;
use egret_perm_compile::ws_assignment_migrate::{
    migrate_ws_assignments_in_tx, undo_ws_assignments_in_tx,
};
use egret_policy::PolicyStore;
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};

fn uniq(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Seeded {
    service_id: i64,
    catalog_id: Uuid,
    workspace_id: Uuid,
    slug: String,
    grant_id: Uuid,
}

/// A StarRocks profile (one bound engine, one attached Iceberg catalog) whose
/// catalog's warehouse belongs to a fresh workspace with one (username-less)
/// member — but with NO assignment grant yet. The authoring tests bind through
/// the real path; the Task-4 seed adds a legacy grant on top. The service is NOT
/// compiled yet.
struct UnboundSeed {
    service_id: i64,
    catalog_id: Uuid,
    workspace_id: Uuid,
    slug: String,
    member: Uuid,
}

async fn seed_unbound_service_and_catalog(db: &Db) -> UnboundSeed {
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&uniq("sr_wsmig"), "starrocks", &json!({}))
        .await
        .expect("create service");

    let (workspace_id, slug) = seed_workspace(pool).await;
    let member = seed_user(pool).await;
    add_member(pool, workspace_id, member).await;

    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-wsmig-test','AKIATEST') RETURNING id",
    )
    .bind(uniq("wsmig-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(uniq("wsmig_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{\"starrocksConfig\":{\"feImage\":\"ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(uniq("wsmig-eng"))
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
    .bind(uniq("wsmig_cat"))
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

    UnboundSeed {
        service_id: service.id,
        catalog_id,
        workspace_id,
        slug,
        member,
    }
}

/// [`seed_unbound_service_and_catalog`] plus a LEGACY implicit grant, written
/// exactly as the PRE-SP3c-3 assignment path wrote it — via raw SQL, because the
/// authoring path (`sync_assignment_grants_in_tx`) now writes the `ws:<slug>
/// members` group shape directly (Task 5) and can no longer produce the old
/// `workspace_assignment` + `workspace_members` row this migration rewrites. This
/// is the pre-existing binding Task 4 backfills.
async fn seed_service_with_ws_assignment_grant(db: &Db) -> Seeded {
    let pool = &db.pool;
    let base = seed_unbound_service_and_catalog(db).await;
    let grant_id: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.grants (effect, actions, catalog_id, origin, origin_workspace_id) \
         VALUES ('allow'::perm.effect, ARRAY['read'], $1, 'workspace_assignment', $2) RETURNING id",
    )
    .bind(base.catalog_id)
    .bind(base.workspace_id)
    .fetch_one(pool)
    .await
    .expect("insert legacy implicit grant");
    sqlx::query(
        "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
         VALUES ($1, 'workspace_members'::perm.principal_kind, $2, 'include'::perm.principal_mode)",
    )
    .bind(grant_id)
    .bind(base.workspace_id.to_string())
    .execute(pool)
    .await
    .expect("insert legacy workspace_members principal");

    Seeded {
        service_id: base.service_id,
        catalog_id: base.catalog_id,
        workspace_id: base.workspace_id,
        slug: base.slug,
        grant_id,
    }
}

/// The compiled bodies of a profile (name + policy_jsonb, ordered by name) as a
/// JSON array — the artifact the engine downloads — read inside `tx`.
async fn compiled_bodies_in_tx(tx: &mut Transaction<'_, Postgres>, service_id: i64) -> Value {
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies WHERE service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(&mut **tx)
    .await
    .unwrap();
    Value::Array(
        rows.into_iter()
            .map(|(name, body)| json!({ "name": name, "body": body }))
            .collect(),
    )
}

/// Delete the seeded grant (cascades its principals) so a later test's view of
/// the implicit-grant set is not polluted by this one.
async fn cleanup(pool: &PgPool, grant_id: Uuid) {
    sqlx::query("DELETE FROM perm.grants WHERE id = $1")
        .bind(grant_id)
        .execute(pool)
        .await
        .expect("cleanup grant");
}

#[tokio::test]
async fn migrating_a_ws_assignment_is_byte_identical_and_removes_workspace_members() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_ws_assignment_grant(&db).await;

    // Everything inside ONE transaction that is rolled back: the global migrate
    // touches every implicit grant, and we must not commit that to the shared DB.
    let mut tx = pool.begin().await.unwrap();
    recompile_service_in_tx(&mut tx, seeded.service_id)
        .await
        .expect("compile before");
    let before = compiled_bodies_in_tx(&mut tx, seeded.service_id).await;
    assert!(
        !before.as_array().unwrap().is_empty(),
        "the workspace-members grant must compile to at least one policy before the rewrite: \
         {before}"
    );

    let m = migrate_ws_assignments_in_tx(&mut tx, "tester")
        .await
        .expect("migrate");
    assert!(
        m.catalogs.contains(&seeded.catalog_id),
        "this catalog was migrated: {m:?}"
    );

    // (a) No `workspace_members` principal remains for THIS grant; it is now a
    // `workspace_assignment_group` grant naming the `ws:<slug> members` group —
    // the SAME shape the authoring path writes for a fresh bind (Task 5).
    let ws_members_left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM perm.grant_principals \
          WHERE grant_id = $1 AND kind = 'workspace_members'::perm.principal_kind",
    )
    .bind(seeded.grant_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(ws_members_left, 0, "no workspace_members principal remains");

    let (origin, ows): (String, Option<Uuid>) =
        sqlx::query_as("SELECT origin, origin_workspace_id FROM perm.grants WHERE id = $1")
            .bind(seeded.grant_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(origin, "workspace_assignment_group");
    assert_eq!(
        ows,
        Some(seeded.workspace_id),
        "origin_workspace_id is KEPT on a group-assignment grant (converges with the authoring path)"
    );

    let group_ref: String = sqlx::query_scalar(
        "SELECT ref FROM perm.grant_principals \
          WHERE grant_id = $1 AND kind = 'group'::perm.principal_kind",
    )
    .bind(seeded.grant_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(group_ref, ws_members_group_name(&seeded.slug));

    // (b) The compiled output is byte-identical.
    recompile_service_in_tx(&mut tx, seeded.service_id)
        .await
        .expect("compile after");
    let after = compiled_bodies_in_tx(&mut tx, seeded.service_id).await;
    assert_eq!(
        before, after,
        "compile must be byte-identical after the rewrite"
    );

    tx.rollback().await.unwrap();
    cleanup(&pool, seeded.grant_id).await;
}

#[tokio::test]
async fn undo_restores_the_workspace_assignment() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_ws_assignment_grant(&db).await;

    // migrate + undo in one rolled-back transaction: the provenance row inserted
    // by migrate is visible to undo in the same tx.
    let mut tx = pool.begin().await.unwrap();
    let m = migrate_ws_assignments_in_tx(&mut tx, "tester")
        .await
        .expect("migrate");
    undo_ws_assignments_in_tx(&mut tx, m.id)
        .await
        .expect("undo");

    let (origin, ows): (String, Option<Uuid>) =
        sqlx::query_as("SELECT origin, origin_workspace_id FROM perm.grants WHERE id = $1")
            .bind(seeded.grant_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(origin, "workspace_assignment", "origin restored");
    assert_eq!(
        ows,
        Some(seeded.workspace_id),
        "origin_workspace_id restored"
    );

    let (kind, p_ref): (String, String) =
        sqlx::query_as("SELECT kind::text, ref FROM perm.grant_principals WHERE grant_id = $1")
            .bind(seeded.grant_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(kind, "workspace_members", "principal kind restored");
    assert_eq!(
        p_ref,
        seeded.workspace_id.to_string(),
        "principal ref restored"
    );

    tx.rollback().await.unwrap();
    cleanup(&pool, seeded.grant_id).await;
}

#[tokio::test]
async fn undo_409s_when_a_row_changed() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_ws_assignment_grant(&db).await;

    let mut tx = pool.begin().await.unwrap();
    let m = migrate_ws_assignments_in_tx(&mut tx, "tester")
        .await
        .expect("migrate");

    // Tamper with the rewritten managed-group principal so its recorded hash no
    // longer matches what is stored.
    sqlx::query(
        "UPDATE perm.grant_principals SET ref = ref || '_tampered' \
          WHERE grant_id = $1 AND kind = 'group'::perm.principal_kind",
    )
    .bind(seeded.grant_id)
    .execute(&mut *tx)
    .await
    .unwrap();

    let err = undo_ws_assignments_in_tx(&mut tx, m.id)
        .await
        .expect_err("undo must refuse a changed row");
    assert!(
        err.to_string().contains("undo refused"),
        "a 409 naming the change: {err}"
    );

    tx.rollback().await.unwrap();
    cleanup(&pool, seeded.grant_id).await;
}

// ---------------------------------------------------------------------------
// SP3c-3 Task 5: the AUTHORING path. Binding a catalog to a workspace through the
// real `egret_perm::sync_assignment_grants_in_tx` now writes the explicit
// `ws:<slug> members` group grant directly — the same shape Task 4 migrates onto.
// These run every write inside ONE transaction that is rolled back (nothing
// commits), scoped to this test's own catalog/workspace, under the file's SERIAL
// mutex. The managed group is ensured here with `sync_ws_members_group_in_tx`
// exactly as `egret-bin`'s `recompile_catalog_assignment` does on the production
// bind path (which this crate cannot call directly without a dependency cycle).

/// The (origin, origin_workspace_id, principal kind, ref, mode) of the single
/// assignment grant on a catalog — the "shape" that Task 4's migrate and Task 5's
/// authoring must agree on.
async fn grant_shape(
    tx: &mut Transaction<'_, Postgres>,
    grant_id: Uuid,
) -> (String, Option<Uuid>, String, String, String) {
    sqlx::query_as(
        "SELECT g.origin, g.origin_workspace_id, p.kind::text, p.ref, p.mode::text \
           FROM perm.grants g JOIN perm.grant_principals p ON p.grant_id = g.id \
          WHERE g.id = $1",
    )
    .bind(grant_id)
    .fetch_one(&mut **tx)
    .await
    .expect("grant shape")
}

#[tokio::test]
async fn binding_authors_a_ws_members_group_grant_that_enforces() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let base = seed_unbound_service_and_catalog(&db).await;

    // Everything in ONE rolled-back transaction.
    let mut tx = pool.begin().await.unwrap();

    // Bind via the REAL authoring path, then ensure the managed group exactly as
    // the production recompile seam (`egret-bin`) does before it compiles.
    egret_perm::sync_assignment_grants_in_tx(&mut tx, base.catalog_id, &[base.workspace_id])
        .await
        .expect("bind");
    sync_ws_members_group_in_tx(&mut tx, base.workspace_id)
        .await
        .expect("ensure ws members group");

    // Exactly one assignment grant on this catalog, the group shape.
    let grant_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM perm.grants WHERE catalog_id = $1 AND origin = 'workspace_assignment_group'",
    )
    .bind(base.catalog_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(grant_ids.len(), 1, "exactly one group-assignment grant");
    let (origin, ows, kind, p_ref, mode) = grant_shape(&mut tx, grant_ids[0]).await;
    assert_eq!(origin, "workspace_assignment_group");
    assert_eq!(ows, Some(base.workspace_id));
    assert_eq!(kind, "group");
    assert_eq!(p_ref, ws_members_group_name(&base.slug));
    assert_eq!(mode, "include");

    // It ENFORCES: recompiling names the member in the compiled artifact the
    // engine downloads. The member is username-less; the compiler mints it, so we
    // read the minted name inside the tx and assert it is present.
    recompile_service_in_tx(&mut tx, base.service_id)
        .await
        .expect("compile");
    let bodies = compiled_bodies_in_tx(&mut tx, base.service_id).await;
    assert!(
        !bodies.as_array().unwrap().is_empty(),
        "the group grant must compile to at least one policy: {bodies}"
    );
    let username: String = sqlx::query_scalar("SELECT username FROM public.users WHERE id = $1")
        .bind(base.member)
        .fetch_one(&mut *tx)
        .await
        .expect("member username minted by compile");
    assert!(
        bodies.to_string().contains(&username),
        "the workspace member must be named in the compiled policy (enforced): {bodies}"
    );

    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn re_binding_the_same_workspace_is_idempotent() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let base = seed_unbound_service_and_catalog(&db).await;

    let mut tx = pool.begin().await.unwrap();
    egret_perm::sync_assignment_grants_in_tx(&mut tx, base.catalog_id, &[base.workspace_id])
        .await
        .expect("bind");
    let first: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM perm.grants WHERE catalog_id = $1 AND origin = 'workspace_assignment_group'",
    )
    .bind(base.catalog_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(first.len(), 1);

    // Re-bind the SAME set: no accumulation, same grant id (not delete+reinsert).
    egret_perm::sync_assignment_grants_in_tx(&mut tx, base.catalog_id, &[base.workspace_id])
        .await
        .expect("re-bind");
    let second: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM perm.grants WHERE catalog_id = $1 AND origin = 'workspace_assignment_group'",
    )
    .bind(base.catalog_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        second, first,
        "re-binding is idempotent: still exactly one grant, same id"
    );

    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn unbinding_removes_the_group_grant() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let base = seed_unbound_service_and_catalog(&db).await;

    let mut tx = pool.begin().await.unwrap();
    egret_perm::sync_assignment_grants_in_tx(&mut tx, base.catalog_id, &[base.workspace_id])
        .await
        .expect("bind");
    // Unbind: empty set removes the group grant.
    egret_perm::sync_assignment_grants_in_tx(&mut tx, base.catalog_id, &[])
        .await
        .expect("unbind");
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM perm.grants \
          WHERE catalog_id = $1 \
            AND origin IN ('workspace_assignment', 'workspace_assignment_group')",
    )
    .bind(base.catalog_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(left, 0, "unbinding removes the assignment grant");

    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn a_migrated_grant_and_a_freshly_authored_grant_have_the_same_shape() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_ws_assignment_grant(&db).await;

    let mut tx = pool.begin().await.unwrap();

    // (1) Migrate the legacy grant (Task 4) and capture its resulting shape.
    migrate_ws_assignments_in_tx(&mut tx, "tester")
        .await
        .expect("migrate");
    let migrated = grant_shape(&mut tx, seeded.grant_id).await;

    // (2) Delete it and RE-AUTHOR the SAME (catalog, workspace) through the real
    // authoring path; capture that grant's shape.
    sqlx::query("DELETE FROM perm.grants WHERE id = $1")
        .bind(seeded.grant_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    egret_perm::sync_assignment_grants_in_tx(&mut tx, seeded.catalog_id, &[seeded.workspace_id])
        .await
        .expect("re-author");
    let fresh_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM perm.grants WHERE catalog_id = $1 AND origin = 'workspace_assignment_group'",
    )
    .bind(seeded.catalog_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let authored = grant_shape(&mut tx, fresh_id).await;

    // Same shape in every column that is not the (minted) grant id: origin,
    // origin_workspace_id, principal kind, ref and mode.
    assert_eq!(
        migrated, authored,
        "Task 4's migrate output and Task 5's authoring output must be identical rows"
    );
    assert_eq!(migrated.0, "workspace_assignment_group");
    assert_eq!(migrated.1, Some(seeded.workspace_id));
    assert_eq!(migrated.2, "group");
    assert_eq!(migrated.3, ws_members_group_name(&seeded.slug));

    tx.rollback().await.unwrap();
    cleanup(&pool, seeded.grant_id).await;
}
