//! SP3c-1: role principals -> managed `role:<name> holders` groups.

use egret_metastore::Db;
use uuid::Uuid;

#[tokio::test]
async fn a_managed_group_source_is_accepted() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let id = Uuid::new_v4();
    let r = sqlx::query("INSERT INTO public.groups (id, name, source) VALUES ($1,$2,'managed')")
        .bind(id)
        .bind(format!("role:data_steward holders {id}"))
        .execute(&db.pool)
        .await;
    assert!(r.is_ok(), "managed source must be accepted: {r:?}");
}

use egret_perm_compile::role_holder_group::{role_holder_group_name, sync_role_holder_group_in_tx};

async fn seed_user(pool: &sqlx::PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.users (id, provider, email, name) VALUES ($1,'local',$2,'T')")
        .bind(id)
        .bind(format!("rh_{id}@example.test"))
        .execute(pool)
        .await
        .expect("insert user");
    id
}

async fn grant_role(pool: &sqlx::PgPool, role_id: Uuid, user: Uuid) {
    sqlx::query(
        "INSERT INTO public.access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user',$1,$2,'platform')",
    )
    .bind(user)
    .bind(role_id)
    .execute(pool)
    .await
    .expect("insert grant");
}

/// A fresh platform role with one direct user holder: (role name, role id, user id).
async fn seed_role_with_one_user_holder(pool: &sqlx::PgPool) -> (String, Uuid, Uuid) {
    let role_id = Uuid::new_v4();
    let role = format!("rh_{}", &role_id.simple().to_string()[..12]);
    sqlx::query("INSERT INTO public.roles (id, scope, name) VALUES ($1,'platform',$2)")
        .bind(role_id)
        .bind(&role)
        .execute(pool)
        .await
        .expect("insert role");
    let u1 = seed_user(pool).await;
    grant_role(pool, role_id, u1).await;
    (role, role_id, u1)
}

/// Revoke `u1`'s grant and grant the role to a new user; returns the new user.
async fn revoke_u1_grant_u2(pool: &sqlx::PgPool, role_id: Uuid, u1: Uuid) -> Uuid {
    sqlx::query("DELETE FROM public.access_grants WHERE principal_id=$1 AND role_id=$2")
        .bind(u1)
        .bind(role_id)
        .execute(pool)
        .await
        .expect("revoke");
    let u2 = seed_user(pool).await;
    grant_role(pool, role_id, u2).await;
    u2
}

#[tokio::test]
async fn the_holder_group_mirrors_the_roles_holders() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let (role, _role_id, user_id) = seed_role_with_one_user_holder(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    let gid = sync_role_holder_group_in_tx(&mut tx, &role)
        .await
        .expect("sync");
    tx.commit().await.unwrap();

    let (name, source): (String, String) =
        sqlx::query_as("SELECT name, source FROM public.groups WHERE id=$1")
            .bind(gid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(name, role_holder_group_name(&role));
    assert_eq!(source, "managed");
    let members: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM public.user_groups WHERE group_id=$1")
            .bind(gid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(members, vec![user_id]);
}

#[tokio::test]
async fn re_syncing_reconciles_added_and_removed_holders() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let (role, role_id, u1) = seed_role_with_one_user_holder(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    let gid = sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();

    let u2 = revoke_u1_grant_u2(&pool, role_id, u1).await;
    let mut tx = pool.begin().await.unwrap();
    let gid2 = sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(gid, gid2, "idempotent: same group");
    let members: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM public.user_groups WHERE group_id=$1")
            .bind(gid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(members, vec![u2]);
}

#[tokio::test]
async fn a_holder_change_resyncs_the_managed_group_via_the_chokepoint() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let (role, role_id, _u1) = seed_role_with_one_user_holder(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();

    let u2 = seed_user(&pool).await;
    grant_role(&pool, role_id, u2).await;
    egret_perm_compile::role_membership::recompile_role_referencing_profiles(&pool)
        .await
        .unwrap();

    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.user_groups ug \
         JOIN public.groups g ON g.id = ug.group_id \
         WHERE g.name = $1 AND ug.user_id = $2)",
    )
    .bind(role_holder_group_name(&role))
    .bind(u2)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(present, "chokepoint must resync the holder group");
}

// ---------------------------------------------------------------------------
// SP3c-1 Task 4: the reversible, equivalence-gated role -> holder-group rewrite.
// ---------------------------------------------------------------------------

use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Scope,
};
use egret_perm_compile::recompile_service_in_tx;
use egret_perm_compile::role_principal_migrate::{
    migrate_role_principals_in_tx, undo_role_principals_in_tx,
};
use egret_policy::PolicyStore;
use serde_json::{json, Value};
use sqlx::PgPool;

/// `migrate_role_principals_in_tx` is global (every service, every role) and its
/// equivalence gate checks the whole affected profile set, so tests that migrate
/// or seed role principals run one at a time within this binary.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn uniq(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Seeded {
    service_id: i64,
    catalog_id: Uuid,
    role: String,
    grant_id: Uuid,
}

/// A StarRocks profile (one bound engine, one attached Iceberg catalog) with a
/// read grant whose only principal is `kind='role'` naming a fresh role that has
/// one (username-less) user holder. The service is NOT compiled yet.
async fn seed_service_with_role_grant(db: &Db) -> Seeded {
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&uniq("sr_rolemig"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");

    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(uniq("rolemig-ws"))
            .bind("Role Mig WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-rolemig-test','AKIATEST') RETURNING id",
    )
    .bind(uniq("rolemig-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(uniq("rolemig_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{\"starrocksConfig\":{\"feImage\":\"ghcr.io/egret-data/egret-starrocks-fe:4.1.7-egret.4\"}}'::jsonb,$2) RETURNING id",
    )
    .bind(uniq("rolemig-eng"))
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
    .bind(uniq("rolemig_cat"))
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

    let (role, _role_id, _user_id) = seed_role_with_one_user_holder(pool).await;
    let perm = PermStore::new(pool.clone());
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
                kind: PrincipalKind::Role,
                r#ref: role.clone(),
            }],
            exclude: vec![],
        },
        origin: GrantOrigin::Explicit,
    };
    let grant_id = perm.create_grant(&grant).await.expect("create grant");

    Seeded {
        service_id: service.id,
        catalog_id,
        role,
        grant_id,
    }
}

/// Recompile the profile and return its compiled bodies (name + policy_jsonb,
/// ordered by name) as a JSON array — the artifact the engine downloads.
async fn compile_service_to_ranger_json(pool: &PgPool, service_id: i64) -> Value {
    let mut tx = pool.begin().await.unwrap();
    recompile_service_in_tx(&mut tx, service_id)
        .await
        .expect("recompile");
    tx.commit().await.unwrap();
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies WHERE service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(pool)
    .await
    .unwrap();
    Value::Array(
        rows.into_iter()
            .map(|(name, body)| json!({ "name": name, "body": body }))
            .collect(),
    )
}

/// Count rows with `kind='role'` via a literal query (sqlx 0.9 wants a
/// `&'static str`).
async fn role_kind_count(pool: &PgPool, role: &str, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .bind(role)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Delete the seeded grant (cascades its principals) so a global role-ref count
/// in another test is not polluted by this one.
async fn cleanup(pool: &PgPool, grant_id: Uuid) {
    sqlx::query("DELETE FROM perm.grants WHERE id = $1")
        .bind(grant_id)
        .execute(pool)
        .await
        .expect("cleanup grant");
}

#[tokio::test]
async fn migrating_role_refs_leaves_the_compile_byte_identical_and_removes_role_kind() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_role_grant(&db).await;

    let before = compile_service_to_ranger_json(&pool, seeded.service_id).await;
    assert!(
        !before.as_array().unwrap().is_empty(),
        "the role grant must compile to at least one policy before the rewrite: {before}"
    );

    let mut tx = pool.begin().await.unwrap();
    let m = migrate_role_principals_in_tx(&mut tx, "tester")
        .await
        .expect("migrate");
    tx.commit().await.unwrap();

    // Scoped to THIS test's fresh role name, not a global count.
    for (tbl, sql) in [
        (
            "perm.grant_principals",
            "SELECT count(*) FROM perm.grant_principals WHERE kind::text = 'role' AND ref = $1",
        ),
        (
            "perm.rule_exemptions",
            "SELECT count(*) FROM perm.rule_exemptions WHERE kind::text = 'role' AND ref = $1",
        ),
        (
            "perm.engine_grant_principals",
            "SELECT count(*) FROM perm.engine_grant_principals WHERE kind::text = 'role' AND ref = $1",
        ),
    ] {
        let n = role_kind_count(&pool, &seeded.role, sql).await;
        assert_eq!(n, 0, "{tbl} still has role refs to {}", seeded.role);
    }
    assert!(
        m.roles.contains(&seeded.role),
        "the role was migrated: {m:?}"
    );
    assert_eq!(m.rewritten, 1, "exactly the one grant principal: {m:?}");

    let after = compile_service_to_ranger_json(&pool, seeded.service_id).await;
    assert_eq!(
        before, after,
        "compile must be byte-identical after the rewrite"
    );

    cleanup(&pool, seeded.grant_id).await;
}

#[tokio::test]
async fn undo_restores_role_refs_when_rows_are_untouched() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_role_grant(&db).await;

    let mut tx = pool.begin().await.unwrap();
    let m = migrate_role_principals_in_tx(&mut tx, "tester")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    undo_role_principals_in_tx(&mut tx, m.id).await.unwrap();
    tx.commit().await.unwrap();

    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM perm.grant_principals \
          WHERE kind::text = 'role' AND ref = $1 AND grant_id = $2",
    )
    .bind(&seeded.role)
    .bind(seeded.grant_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "undo restores the role ref");

    cleanup(&pool, seeded.grant_id).await;
}

#[tokio::test]
async fn undo_refuses_when_a_rewritten_row_changed() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_role_grant(&db).await;

    let mut tx = pool.begin().await.unwrap();
    let m = migrate_role_principals_in_tx(&mut tx, "tester")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Tamper with the rewritten group principal so its recorded hash no longer
    // matches what is stored.
    sqlx::query(
        "UPDATE perm.grant_principals SET ref = ref || '_tampered' \
          WHERE grant_id = $1 AND kind = 'group'::perm.principal_kind",
    )
    .bind(seeded.grant_id)
    .execute(&pool)
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let err = undo_role_principals_in_tx(&mut tx, m.id)
        .await
        .expect_err("undo must refuse a changed row");
    tx.rollback().await.unwrap();
    assert!(
        err.to_string().contains("undo refused"),
        "a 409 naming the change: {err}"
    );

    cleanup(&pool, seeded.grant_id).await;
}

// ---------------------------------------------------------------------------
// SP3c-1 Task 5: the admin endpoints and the transitional write-seam guards,
// driven through the real router.
// ---------------------------------------------------------------------------

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_perm_compile::{perm_router, PermState};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt; // oneshot

async fn admin_app(db: &Db) -> (axum::Router, String) {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!("{}@rolemig.egret.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("user");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(uid)
    .execute(&db.pool)
    .await
    .expect("grant platform_admin");
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let token = sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let app = perm_router().with_state(PermState {
        opa_cache: None,
        perm: PermStore::new(db.pool.clone()),
        policy: PolicyStore::new(db.clone()),
        sessions,
        pdp,
    });
    (app, token)
}

async fn post_json(app: &axum::Router, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(uri)
                .header("Cookie", format!("egret_session={token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

fn grant_body(catalog_id: Uuid, kind: &str, r#ref: &str) -> Value {
    json!({
        "effect": "allow",
        "actions": ["read"],
        "scope": { "catalog_id": catalog_id, "schema": null, "table": null, "column": null },
        "principals": { "include": [{ "kind": kind, "ref": r#ref }], "exclude": [] }
    })
}

#[tokio::test]
async fn writing_a_new_role_principal_is_refused() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let seeded = seed_service_with_role_grant(&db).await;
    let (app, token) = admin_app(&db).await;

    let (status, body) = post_json(
        &app,
        "/perm/grants",
        &token,
        grant_body(seeded.catalog_id, "role", &seeded.role),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.to_string().contains("grant to a group instead"),
        "{body}"
    );
    cleanup(&db.pool, seeded.grant_id).await;
}

#[tokio::test]
async fn writing_a_new_ref_to_a_holder_group_is_refused() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let seeded = seed_service_with_role_grant(&db).await;
    let (app, token) = admin_app(&db).await;

    // A REAL holder group (it exists and validates as a group) is still refused.
    let mut tx = db.pool.begin().await.unwrap();
    sync_role_holder_group_in_tx(&mut tx, &seeded.role)
        .await
        .expect("sync");
    tx.commit().await.unwrap();
    let real = role_holder_group_name(&seeded.role);
    for name in [real, "role:ghost_role holders".to_string()] {
        let (status, body) = post_json(
            &app,
            "/perm/grants",
            &token,
            grant_body(seeded.catalog_id, "group", &name),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");
        assert!(
            body.to_string().contains("managed migration targets"),
            "{name}: {body}"
        );
    }
    cleanup(&db.pool, seeded.grant_id).await;
}

#[tokio::test]
async fn the_migrate_endpoint_removes_role_refs_and_undo_restores_them() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let seeded = seed_service_with_role_grant(&db).await;
    let (app, token) = admin_app(&db).await;
    let count_sql =
        "SELECT count(*) FROM perm.grant_principals WHERE grant_id = $1 AND kind::text = 'role'";
    let roles_of = |id: Uuid| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(count_sql)
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(roles_of(seeded.grant_id).await, 1);

    let (status, body) = post_json(&app, "/perm/migrate/role-principals", &token, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["roles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == seeded.role.as_str()),
        "{body}"
    );
    assert!(body["rewritten"].as_u64().unwrap() >= 1, "{body}");
    assert_eq!(roles_of(seeded.grant_id).await, 0, "role ref removed");

    let (status, body2) = post_json(
        &app,
        "/perm/migrate/role-principals/undo",
        &token,
        json!({ "id": body["id"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body2}");
    assert_eq!(roles_of(seeded.grant_id).await, 1, "role ref restored");
    cleanup(&db.pool, seeded.grant_id).await;
}

// ---------------------------------------------------------------------------
// SP3c-1 Task 6: `explain` is unchanged for a migrated role's holder.
// ---------------------------------------------------------------------------

/// `/perm/explain` for `username` and `/perm/explain/principal` for the role,
/// both for a read of the seeded catalog, as serialised JSON text.
async fn explain_texts(
    app: &axum::Router,
    token: &str,
    s: &Seeded,
    username: &str,
) -> (String, String) {
    let target =
        json!({ "catalog_id": s.catalog_id, "schema": null, "table": null, "column": null });
    let (status, member) = post_json(
        app,
        "/perm/explain",
        token,
        json!({ "subject": { "user": username }, "action": "read", "target": target }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{member}");
    let (status, principal) = post_json(
        app,
        "/perm/explain/principal",
        token,
        json!({ "principal": { "kind": "role", "ref": s.role }, "action": "read", "target": target }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{principal}");
    (member.to_string(), principal.to_string())
}

#[tokio::test]
async fn explain_is_byte_identical_across_the_role_migration() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_role_grant(&db).await;
    let (app, token) = admin_app(&db).await;

    // The compile mints the holder's missing username.
    let compiled = compile_service_to_ranger_json(&pool, seeded.service_id).await;
    assert!(!compiled.as_array().unwrap().is_empty(), "{compiled}");
    let username: String = sqlx::query_scalar(
        "SELECT u.username FROM users u \
           JOIN access_grants ag ON ag.principal_type = 'user' AND ag.principal_id = u.id \
           JOIN roles r ON r.id = ag.role_id \
          WHERE r.name = $1 AND r.scope = 'platform'",
    )
    .bind(&seeded.role)
    .fetch_one(&pool)
    .await
    .expect("the holder has a username after the compile");

    let before = explain_texts(&app, &token, &seeded, &username).await;
    assert!(
        before.0.contains("select"),
        "the explain must actually answer: {}",
        before.0
    );

    let mut tx = pool.begin().await.unwrap();
    let m = migrate_role_principals_in_tx(&mut tx, "tester")
        .await
        .expect("migrate");
    tx.commit().await.unwrap();
    assert!(m.roles.contains(&seeded.role), "{m:?}");

    let after = explain_texts(&app, &token, &seeded, &username).await;
    assert_eq!(before.0, after.0, "member explain changed across migration");
    assert_eq!(before.1, after.1, "role explain changed across migration");

    cleanup(&pool, seeded.grant_id).await;
}

// ---------------------------------------------------------------------------
// SP3c-1 final review Fix 1: the LIVE role-holder-change path must re-sync the
// managed holder group AND recompile the group-ref grant, or a revoke after the
// migration fails OPEN (the ex-holder keeps the access until restart).
// ---------------------------------------------------------------------------

/// The stored (already-committed) compiled artifact for `service_id`, WITHOUT
/// recompiling — so a test asserts what the path under test wrote, not what a
/// fresh compile would write.
async fn read_compiled_json(pool: &PgPool, service_id: i64) -> Value {
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies WHERE service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(pool)
    .await
    .unwrap();
    Value::Array(
        rows.into_iter()
            .map(|(name, body)| json!({ "name": name, "body": body }))
            .collect(),
    )
}

async fn group_has_user(pool: &PgPool, group_name: &str, user: Uuid) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.user_groups ug \
         JOIN public.groups g ON g.id = ug.group_id \
         WHERE g.name = $1 AND ug.user_id = $2)",
    )
    .bind(group_name)
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn revoking_a_role_via_the_live_path_resyncs_and_recompiles_the_holder_group() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let seeded = seed_service_with_role_grant(&db).await;

    // The role id and its one holder (carol), for the live revoke path.
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE name = $1 AND scope = 'platform'")
            .bind(&seeded.role)
            .fetch_one(&pool)
            .await
            .unwrap();
    let carol: Uuid = sqlx::query_scalar(
        "SELECT principal_id FROM access_grants WHERE role_id = $1 AND principal_type = 'user'",
    )
    .bind(role_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Compile once so carol's missing username is minted, then capture it.
    let compiled = compile_service_to_ranger_json(&pool, seeded.service_id).await;
    assert!(!compiled.as_array().unwrap().is_empty(), "{compiled}");
    let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(carol)
        .fetch_one(&pool)
        .await
        .expect("the compile minted carol's username");

    // Migrate: the grant now names the managed `role:<role> holders` group.
    let mut tx = pool.begin().await.unwrap();
    migrate_role_principals_in_tx(&mut tx, "tester")
        .await
        .expect("migrate");
    tx.commit().await.unwrap();

    let gname = role_holder_group_name(&seeded.role);
    assert!(
        group_has_user(&pool, &gname, carol).await,
        "carol must be in the holder group right after the migration"
    );
    let before_revoke = read_compiled_json(&pool, seeded.service_id).await;
    assert!(
        before_revoke.to_string().contains(&username),
        "the migrated group-ref grant must still grant carol before the revoke: {before_revoke}"
    );

    // REVOKE via the LIVE control-plane path (not the migration, not the boot repair).
    egret_perm_compile::unassign_platform_role_and_recompile(&pool, "user", carol, role_id)
        .await
        .expect("live unassign");

    // Fail-CLOSED: the holder group drops carol, and the committed compiled
    // artifact no longer names her — in the same path, without a restart.
    assert!(
        !group_has_user(&pool, &gname, carol).await,
        "the live revoke must drop carol from the managed holder group"
    );
    let after_revoke = read_compiled_json(&pool, seeded.service_id).await;
    assert!(
        !after_revoke.to_string().contains(&username),
        "the live revoke must recompile the group-ref grant so carol is no longer granted: \
         {after_revoke}"
    );
    assert_ne!(
        before_revoke, after_revoke,
        "the live revoke must have recompiled the profile (it did not)"
    );

    cleanup(&pool, seeded.grant_id).await;
}

// ---------------------------------------------------------------------------
// SP3c-1 final review Fix 2: never adopt a pre-existing local/oidc group that
// happens to carry the managed name — that would be a silent, un-gated access
// change. Refuse the collision instead.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_refuses_to_adopt_a_preexisting_non_managed_group() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let (role, _role_id, _u1) = seed_role_with_one_user_holder(&pool).await;

    // A pre-existing LOCAL group squats the managed name.
    let name = role_holder_group_name(&role);
    sqlx::query(
        "INSERT INTO public.groups (id, name, source) VALUES (gen_random_uuid(), $1, 'local')",
    )
    .bind(&name)
    .execute(&pool)
    .await
    .expect("seed a local group of the managed name");

    let mut tx = pool.begin().await.unwrap();
    let err = sync_role_holder_group_in_tx(&mut tx, &role)
        .await
        .expect_err("a non-managed group of the same name must be refused");
    drop(tx);
    assert!(
        matches!(
            err,
            egret_perm::CompileError::HolderGroupNameCollision { .. }
        ),
        "the collision must be refused as HolderGroupNameCollision, not adopted: {err:?}"
    );

    // The squatting group is untouched: still local (not flipped to managed).
    let source: String = sqlx::query_scalar("SELECT source FROM public.groups WHERE name = $1")
        .bind(&name)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        source, "local",
        "the pre-existing group must NOT be flipped to managed"
    );
}
