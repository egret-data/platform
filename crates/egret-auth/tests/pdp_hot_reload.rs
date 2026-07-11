//! DB-gated hot-reload integration test (Effort B / B1b-0 Task 4).
//! Self-skips without DATABASE_URL. Proves an in-flight `CedarPdp` reflects a
//! permission grant/revoke made through `RoleRepo` sharing the same `EventBus`,
//! with NO rebuild of the PDP — the background reload task swaps the template
//! set on each `RolesChanged`.
//!
//! Model: `crates/egret-auth/tests/cedar_pdp.rs` + `role_events.rs`.

use egret_auth::pdp::CedarPdp;
use egret_auth::roles::RoleRepo;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::ids::{UserId, WorkspaceId};
use egret_events::EventBus;
use egret_metastore::Db;
use std::time::Duration;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", Uuid::new_v4().simple())
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

/// Poll `cond` until it returns true or the budget elapses. Returns whether it
/// became true. Uses std time — this is a normal `#[tokio::test]`, not a
/// workflow script, so `Instant`/`Duration` are available.
async fn poll_until<F, Fut>(mut cond: F, budget: Duration) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = std::time::Instant::now();
    loop {
        if cond().await {
            return true;
        }
        if start.elapsed() > budget {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn pdp_hot_reloads_permission_grant_and_revoke() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    // ── Seed: user U, workspace W, a fresh zero-perm workspace role R, and
    //    membership U→R@W (the 0068/0069 triggers mirror it into access_grants).
    let u = seed_user(&db).await;
    let w = seed_workspace(&db).await;

    let bus = EventBus::new();
    let pdp = CedarPdp::build(db.clone(), &bus).await.unwrap();
    let repo = RoleRepo::new(db.clone(), bus.clone());

    let role_name = uniq("hotreload");
    let r_id = repo
        .create("workspace", &role_name, "", None)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, principal_type, principal_id, role) \
         VALUES ($1, 'user', $2, $3)",
    )
    .bind(w.0)
    .bind(u.0)
    .bind(&role_name)
    .execute(&db.pool)
    .await
    .unwrap();

    let subject = Subject {
        user_id: u,
        group_ids: vec![],
    };
    let action = "dashboard:read";

    // (1) DENIED initially: role R holds zero permissions.
    let initial = pdp
        .authorize(&subject, action, &Resource::Workspace(w))
        .await
        .unwrap();
    assert!(
        !initial.allow,
        "role with zero permissions must deny `{action}` on W"
    );

    // (2) Grant `dashboard:read` through the repo — emits RolesChanged; the PDP's
    //     background task rebuilds its template set. NO rebuild of `pdp` here.
    repo.set_permissions(r_id, &["dashboard:read".into()])
        .await
        .unwrap();

    let allowed = poll_until(
        || async {
            pdp.authorize(&subject, action, &Resource::Workspace(w))
                .await
                .unwrap()
                .allow
        },
        Duration::from_secs(3),
    )
    .await;
    assert!(allowed, "PDP did not hot-reload the new permission");

    // (3) Revoke — flips back to DENIED the same way, still no rebuild.
    repo.set_permissions(r_id, &[]).await.unwrap();

    let denied_again = poll_until(
        || async {
            !pdp.authorize(&subject, action, &Resource::Workspace(w))
                .await
                .unwrap()
                .allow
        },
        Duration::from_secs(3),
    )
    .await;
    assert!(
        denied_again,
        "PDP did not hot-reload the permission revocation"
    );

    eprintln!(
        "pdp_hot_reload OK: initial-deny={}, grant->allow={}, revoke->deny={}",
        !initial.allow, allowed, denied_again
    );
}
