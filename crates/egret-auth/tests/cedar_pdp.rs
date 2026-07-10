//! DB-gated e2e integration test for `CedarPdp` (Effort B / B1a Task 8).
//! Self-skips without DATABASE_URL. Exercises the assembled decision point end
//! to end: build templates from `role_permissions`, resolve a user's mirrored
//! `access_grants`, link them, and authorize via Cedar.
//!
//! Model: `crates/egret-auth/tests/grant_store.rs`.

use egret_auth::pdp::CedarPdp;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

/// Insert a fresh local user and return its id.
async fn seed_user(db: &Db) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!("{}@test.local", Uuid::new_v4()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    UserId(id)
}

async fn seed_workspace(db: &Db) -> WorkspaceId {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'W') RETURNING id")
            .bind(format!("ws-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    WorkspaceId(id)
}

#[tokio::test]
async fn cedar_pdp_end_to_end() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    // Confirm the (action, ws-role) pair we assert Allow on is legitimate: the
    // seeded workspace role `ws_admin` (migration 0003) must hold `dashboard:read`.
    let ws_role = "ws_admin";
    let action = "dashboard:read";
    let has_perm: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp \
         JOIN roles r ON r.id = rp.role_id \
         WHERE r.scope = 'workspace' AND r.name = $1 AND rp.permission = $2",
    )
    .bind(ws_role)
    .bind(action)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        has_perm, 1,
        "seeded ws role `{ws_role}` must hold `{action}`"
    );

    // ── U1: a plain workspace member of W (no platform grant) ──────────────
    let u1 = seed_user(&db).await;
    let w = seed_workspace(&db).await;
    let w2 = seed_workspace(&db).await; // a workspace U1 is NOT a member of
                                        // Legacy write path; the `sync_workspace_grant` trigger mirrors it into
                                        // access_grants, resolving the role NAME to the ws-scoped role id.
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, principal_type, principal_id, role) \
         VALUES ($1, 'user', $2, $3)",
    )
    .bind(w.0)
    .bind(u1.0)
    .bind(ws_role)
    .execute(&db.pool)
    .await
    .unwrap();

    let pdp = CedarPdp::build(db.clone()).await.unwrap();
    let u1_subject = Subject {
        user_id: u1,
        group_ids: vec![],
    };

    // (1) ws-allow: the ws role grants `dashboard:read` on its own workspace.
    let allow = pdp
        .authorize(&u1_subject, action, &Resource::Workspace(w))
        .await
        .unwrap();
    assert!(allow.allow, "ws member must be allowed `{action}` on W");

    // (2) platform-deny: U1 has no platform grant → denied at platform scope.
    let plat = pdp
        .authorize(&u1_subject, "engine:create", &Resource::Platform)
        .await
        .unwrap();
    assert!(
        !plat.allow,
        "non-admin without a platform grant must be denied engine:create"
    );

    // (3) cross-ws-deny: the grant is scoped to W, not W2.
    let cross = pdp
        .authorize(&u1_subject, action, &Resource::Workspace(w2))
        .await
        .unwrap();
    assert!(
        !cross.allow,
        "grant on W must not authorize `{action}` on W2"
    );

    // ── U2: platform_admin (bypasses every scope) ─────────────────────────
    let u2 = seed_user(&db).await;
    let platform_admin_role_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("seeded platform role `platform_admin` must exist");
    // Legacy write path; `sync_platform_grant` mirrors it into access_grants.
    sqlx::query(
        "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
         VALUES ('user', $1, $2)",
    )
    .bind(u2.0)
    .bind(platform_admin_role_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let admin_pdp = CedarPdp::build(db.clone()).await.unwrap();
    let u2_subject = Subject {
        user_id: u2,
        group_ids: vec![],
    };

    // (4a) admin-bypass at platform scope.
    let admin_plat = admin_pdp
        .authorize(&u2_subject, "engine:create", &Resource::Platform)
        .await
        .unwrap();
    assert!(
        admin_plat.allow,
        "platform_admin must be allowed engine:create at platform scope"
    );
    // (4b) admin-bypass reaches workspace scope (W is in Platform::root).
    let admin_ws = admin_pdp
        .authorize(&u2_subject, action, &Resource::Workspace(w))
        .await
        .unwrap();
    assert!(
        admin_ws.allow,
        "platform_admin bypass must cover workspace scope"
    );

    eprintln!(
        "cedar_pdp_end_to_end OK: ws-allow={}, platform-deny={}, cross-ws-deny={}, admin-plat={}, admin-ws={}",
        allow.allow, !plat.allow, !cross.allow, admin_plat.allow, admin_ws.allow
    );
}
