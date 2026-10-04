//! Regression matrix (Effort B / B1a Task 9): prove the new `CedarPdp`
//! reproduces the legacy authorization decisions EXACTLY.
//!
//! The `legacy_oracle` below is the source of truth for the expected decision.
//! It reads the unified `access_grants` store (with the appropriate scope filter),
//! implementing the documented semantics:
//!
//! * platform_admin  = the user holds a platform-scope `access_grants` row (direct
//!   or via a group) to the role named `platform_admin`.
//! * platform decision(action) = is_platform_admin OR the user holds `action`
//!   via a platform role (direct or via group).
//! * workspace decision(W, action) = is_platform_admin OR the user holds
//!   `action` via a workspace role for W (direct or via group).
//!
//! `Resource::Platform` maps to the platform decision; `Resource::Workspace(W)`
//! to the workspace decision. Cedar must match the oracle in every cell.
//!
//! Self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//! Model: `crates/egret-auth/tests/cedar_pdp.rs`.

use egret_auth::pdp::CedarPdp;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::ids::GroupId;
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
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

async fn seed_group(db: &Db) -> GroupId {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(format!("grp-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    GroupId(id)
}

async fn add_user_to_group(db: &Db, user: UserId, group: GroupId) {
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user.0)
        .bind(group.0)
        .execute(&db.pool)
        .await
        .unwrap();
}

async fn platform_role_id(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'platform' AND name = $1")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap_or_else(|_| panic!("seeded platform role `{name}` must exist"))
}

async fn assign_platform_role_user(db: &Db, user: UserId, role_id: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL) ON CONFLICT DO NOTHING",
    )
    .bind(user.0)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .unwrap();
}

async fn add_ws_member_user(db: &Db, ws: WorkspaceId, user: UserId, role: &str) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = $3 \
         ON CONFLICT DO NOTHING",
    )
    .bind(ws.0)
    .bind(user.0)
    .bind(role)
    .execute(&db.pool)
    .await
    .unwrap();
}

async fn add_ws_member_group(db: &Db, ws: WorkspaceId, group: GroupId, role: &str) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'group', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = $3 \
         ON CONFLICT DO NOTHING",
    )
    .bind(ws.0)
    .bind(group.0)
    .bind(role)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// The user's direct group memberships, as the `Subject` sees them.
async fn groups_of(db: &Db, user: UserId) -> Vec<GroupId> {
    let rows: Vec<(Uuid,)> = sqlx::query_as("SELECT group_id FROM user_groups WHERE user_id = $1")
        .bind(user.0)
        .fetch_all(&db.pool)
        .await
        .unwrap();
    rows.into_iter().map(|(g,)| GroupId(g)).collect()
}

// ── Oracle ─────────────────────────────────────────────────────────────────

/// platform_admin = holds a platform-scope access_grant (direct or via group) to
/// the role named `platform_admin`.
async fn is_platform_admin(db: &Db, user: UserId) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS ( \
           SELECT 1 FROM access_grants pra \
           JOIN roles r ON r.id = pra.role_id \
           WHERE r.scope = 'platform' AND r.name = 'platform_admin' AND pra.scope_type = 'platform' \
             AND ( (pra.principal_type = 'user'  AND pra.principal_id = $1) \
                OR (pra.principal_type = 'group' AND pra.principal_id IN \
                      (SELECT group_id FROM user_groups WHERE user_id = $1)) ) )",
    )
    .bind(user.0)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// Holds `action` via a platform role (direct or via group).
async fn platform_holds(db: &Db, user: UserId, action: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS ( \
           SELECT 1 FROM access_grants pra \
           JOIN roles r ON r.id = pra.role_id AND r.scope = 'platform' \
           JOIN role_permissions rp ON rp.role_id = r.id \
           WHERE rp.permission = $2 AND pra.scope_type = 'platform' \
             AND ( (pra.principal_type = 'user'  AND pra.principal_id = $1) \
                OR (pra.principal_type = 'group' AND pra.principal_id IN \
                      (SELECT group_id FROM user_groups WHERE user_id = $1)) ) )",
    )
    .bind(user.0)
    .bind(action)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// Holds `action` via a workspace role for `ws` (direct or via group).
async fn workspace_holds(db: &Db, user: UserId, ws: WorkspaceId, action: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS ( \
           SELECT 1 FROM access_grants wm \
           JOIN roles r ON r.id = wm.role_id AND r.scope = 'workspace' \
           JOIN role_permissions rp ON rp.role_id = r.id \
           WHERE wm.scope_type = 'workspace' AND wm.scope_id = $2 AND rp.permission = $3 \
             AND ( (wm.principal_type = 'user'  AND wm.principal_id = $1) \
                OR (wm.principal_type = 'group' AND wm.principal_id IN \
                      (SELECT group_id FROM user_groups WHERE user_id = $1)) ) )",
    )
    .bind(user.0)
    .bind(ws.0)
    .bind(action)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// The legacy decision for `(user, action, resource)` — the EXPECTED value.
async fn legacy_oracle(db: &Db, user: UserId, action: &str, resource: &Resource) -> bool {
    if is_platform_admin(db, user).await {
        return true;
    }
    match resource {
        Resource::Platform => platform_holds(db, user, action).await,
        Resource::Workspace(w) => workspace_holds(db, user, *w, action).await,
        // This legacy oracle only compares platform/workspace decisions; object
        // scope has its own tests in entities.rs and Task 3.
        Resource::Object(_) => unreachable!("legacy oracle is not exercised with object resources"),
    }
}

#[tokio::test]
async fn cedar_pdp_reproduces_legacy_decisions() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    // Two workspaces. Users are members of W1 only; W2 is a cross-workspace deny.
    let w1 = seed_workspace(&db).await;
    let w2 = seed_workspace(&db).await;

    // ── User A: platform_admin (bypasses every scope). ──
    let user_a = seed_user(&db).await;
    assign_platform_role_user(&db, user_a, platform_role_id(&db, "platform_admin").await).await;

    // ── User B: workspace role `ws_admin` in W1 only; no platform role. ──
    let user_b = seed_user(&db).await;
    add_ws_member_user(&db, w1, user_b, "ws_admin").await;

    // ── User C: a fresh platform role with exactly ONE known permission
    // (`engine:read`), so the platform expectation is unambiguous. Created
    // BEFORE the PDP is built so its template is compiled from role_permissions.
    let user_c = seed_user(&db).await;
    let regr_role_name = format!("regr_plat_role_{}", Uuid::new_v4().simple());
    let regr_role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) \
         VALUES ('platform', $1, 'Task 9 regression role') RETURNING id",
    )
    .bind(&regr_role_name)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'engine:read')")
        .bind(regr_role_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assign_platform_role_user(&db, user_c, regr_role_id).await;

    // ── User D: member of a GROUP that holds workspace role `ws_editor` in W1
    // (exercises the group grant path — both allow and deny cells).
    let user_d = seed_user(&db).await;
    let group = seed_group(&db).await;
    add_user_to_group(&db, user_d, group).await;
    add_ws_member_group(&db, w1, group, "ws_editor").await;

    // Build the PDP ONCE, after all roles/permissions exist.
    let bus = egret_events::EventBus::new();
    let pdp = CedarPdp::build(db.clone(), &bus).await.unwrap();

    let actions = [
        "engine:create",
        "engine:read",
        "dashboard:read",
        "dashboard:manage",
        "app:write",
    ];
    let users = [
        ("A/platform_admin", user_a),
        ("B/ws_admin@W1", user_b),
        ("C/plat:engine:read", user_c),
        ("D/group:ws_editor@W1", user_d),
    ];
    let resources = [
        ("Platform", Resource::Platform),
        ("W1", Resource::Workspace(w1)),
        ("W2", Resource::Workspace(w2)),
    ];

    let mut cells = 0usize;
    let mut allow_cells = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for (ulabel, user_id) in users {
        let group_ids = groups_of(&db, user_id).await;
        for action in actions {
            for (rlabel, resource) in &resources {
                let subject = Subject {
                    user_id,
                    group_ids: group_ids.clone(),
                };
                let expected = legacy_oracle(&db, user_id, action, resource).await;
                let got = pdp
                    .authorize(&subject, action, resource)
                    .await
                    .unwrap()
                    .allow;
                if got != expected {
                    mismatches.push(format!(
                        "MISMATCH [{ulabel} × {action} × {rlabel}] user={user_id}: cedar={got} legacy={expected}"
                    ));
                }
                cells += 1;
                if expected {
                    allow_cells += 1;
                }
            }
        }
    }

    if !mismatches.is_empty() {
        eprintln!(
            "pdp_regression: {} mismatch(es) of {cells} cells:",
            mismatches.len()
        );
        for m in &mismatches {
            eprintln!("  {m}");
        }
        panic!("{} cell(s) diverge from legacy oracle", mismatches.len());
    }

    // Sanity: the grid actually exercised allows AND denies (not all-deny/all-allow).
    assert!(allow_cells > 0, "grid must contain allow cells");
    assert!(allow_cells < cells, "grid must contain deny cells");

    eprintln!(
        "pdp_regression OK: {cells} cells, {allow_cells} allow / {} deny, ZERO mismatches",
        cells - allow_cells
    );
}
