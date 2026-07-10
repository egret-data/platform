//! DB-gated e2e integration test for object-scoped authorization (Effort B /
//! B2-1a Task 3). Self-skips without DATABASE_URL.
//!
//! Exercises the full object-grant decision path: `grants_for` loads object
//! grants keyed by the object's own id AND its folder id, and `CedarPdp`
//! resolves folder-inherit, cross-folder deny, item isolation, workspace
//! cascade, and platform non-cascade.
//!
//! Note: the seeded `ws_viewer` role does NOT actually hold `dashboard:read`
//! (only `catalog:read` + `notebook:read`) — the object/workspace grants below
//! use `ws_editor`, which genuinely holds both `dashboard:read` and
//! `dashboard:manage`. The role only namespaces the permission; the scope
//! resolution under test is identical.
//!
//! Model: `crates/egret-auth/tests/cedar_pdp.rs`.

use egret_auth::pdp::CedarPdp;
use egret_core::authz::{ObjectRef, Pdp, Resource, Subject};
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

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

/// A real `analytics.folders` row (entity_type='dashboard') in workspace `ws`.
async fn seed_folder(db: &Db, ws: WorkspaceId) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO analytics.folders (workspace_id, entity_type, name) \
         VALUES ($1, 'dashboard', $2) RETURNING id",
    )
    .bind(ws.0)
    .bind(format!("folder-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// Insert an object-scoped grant of `role_id` to user `u` on `(object_type, scope_id)`.
async fn grant_object(db: &Db, u: UserId, role_id: Uuid, object_type: &str, scope_id: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id) \
         VALUES ('user', $1, $2, 'object', $3, $4)",
    )
    .bind(u.0)
    .bind(role_id)
    .bind(object_type)
    .bind(scope_id)
    .execute(&db.pool)
    .await
    .unwrap();
}

fn dashboard(id: Uuid, folder: Option<Uuid>, ws: WorkspaceId) -> Resource {
    Resource::Object(ObjectRef {
        kind: "dashboard".into(),
        id,
        folder_id: folder,
        workspace_id: ws,
    })
}

#[tokio::test]
async fn object_scope_end_to_end() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    // ── Role setup ─────────────────────────────────────────────────────────
    // `ws_editor` genuinely holds dashboard:read + dashboard:manage (migration
    // 0015); confirm before relying on it (ws_viewer does NOT — see module doc).
    let ws_editor_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope = 'workspace' AND name = 'ws_editor'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("seeded ws role `ws_editor` must exist");
    let editor_perms: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions \
         WHERE role_id = $1 AND permission IN ('dashboard:read', 'dashboard:manage')",
    )
    .bind(ws_editor_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        editor_perms, 2,
        "ws_editor must hold dashboard:read AND dashboard:manage"
    );

    // A platform-scoped role that DOES hold dashboard:read — proves that even a
    // permission-bearing platform grant cannot reach an object (template uses
    // `==`, so ?resource = Platform::root never matches an Object). Created
    // BEFORE the PDP build so its template is compiled in.
    let plat_role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 't') RETURNING id",
    )
    .bind(format!("obj_scope_plat_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'dashboard:read')")
        .bind(plat_role_id)
        .execute(&db.pool)
        .await
        .unwrap();

    // ── Content: workspace W, folders F/F2, dashboards D1(F) D2(F2) D3(F) ────
    let w = seed_workspace(&db).await;
    let f = seed_folder(&db, w).await;
    let f2 = seed_folder(&db, w).await;
    let d1 = Uuid::new_v4(); // in folder F
    let d2 = Uuid::new_v4(); // in folder F2
    let d3 = Uuid::new_v4(); // in folder F (sibling of D1)

    // U: NOT a workspace member of W, so ONLY the explicit grants below reach it.
    let u = seed_user(&db).await;

    let bus = egret_events::EventBus::new();
    let pdp = CedarPdp::build(db.clone(), &bus).await.unwrap();
    let subject = Subject {
        user_id: u,
        group_ids: vec![],
    };
    let rd = "dashboard:read";
    let mg = "dashboard:manage";

    // ── Phase A: folder grant on F (ws_editor). Object grants read live. ─────
    grant_object(&db, u, ws_editor_id, "folder", f).await;

    let a_d1 = pdp
        .authorize(&subject, rd, &dashboard(d1, Some(f), w))
        .await
        .unwrap();
    assert!(a_d1.allow, "folder grant on F must reach D1 inside F");
    let a_d2 = pdp
        .authorize(&subject, rd, &dashboard(d2, Some(f2), w))
        .await
        .unwrap();
    assert!(!a_d2.allow, "folder grant on F must NOT reach D2 in F2");
    let a_d3 = pdp
        .authorize(&subject, rd, &dashboard(d3, Some(f), w))
        .await
        .unwrap();
    assert!(a_d3.allow, "folder grant on F must reach sibling D3 in F");

    // ── Phase B: item grant on D2 (ws_editor). Isolates a single object. ─────
    grant_object(&db, u, ws_editor_id, "dashboard", d2).await;

    let b_d2 = pdp
        .authorize(&subject, mg, &dashboard(d2, Some(f2), w))
        .await
        .unwrap();
    assert!(b_d2.allow, "item grant on D2 must authorize D2 (manage)");
    // A sibling of D2 in F2 (no grant reaches it) is denied.
    let d_sib = Uuid::new_v4();
    let b_sib = pdp
        .authorize(&subject, mg, &dashboard(d_sib, Some(f2), w))
        .await
        .unwrap();
    assert!(
        !b_sib.allow,
        "item grant on D2 must NOT authorize a sibling in F2"
    );

    // ── Phase C: workspace grant on W → U reads ANY dashboard in W. ──────────
    // D2 in F2 was denied for read in Phase A; the ws grant now cascades.
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3)",
    )
    .bind(u.0)
    .bind(ws_editor_id)
    .bind(w.0)
    .execute(&db.pool)
    .await
    .unwrap();

    let c_d2 = pdp
        .authorize(&subject, rd, &dashboard(d2, Some(f2), w))
        .await
        .unwrap();
    assert!(
        c_d2.allow,
        "workspace grant must cascade to ANY dashboard in W (D2 in F2)"
    );

    // ── Phase D: a platform (non-admin) grant authorizes NO object action ────
    // Separate user so U's ws grant doesn't interfere. The platform role holds
    // dashboard:read but its `==` template can't match an Object.
    let u_plat = seed_user(&db).await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(u_plat.0)
    .bind(plat_role_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let plat_subject = Subject {
        user_id: u_plat,
        group_ids: vec![],
    };
    let d_plat = pdp
        .authorize(&plat_subject, rd, &dashboard(d1, Some(f), w))
        .await
        .unwrap();
    assert!(
        !d_plat.allow,
        "a platform-scoped grant must NOT authorize an object action"
    );

    eprintln!(
        "object_scope_end_to_end OK: folder-inherit-allow(D1={},D3={}), cross-folder-deny(D2={}), item-isolate(D2={},sibling={}), ws-cascade(D2={}), platform-none({})",
        a_d1.allow, a_d3.allow, !a_d2.allow, b_d2.allow, !b_sib.allow, c_d2.allow, !d_plat.allow
    );
}
