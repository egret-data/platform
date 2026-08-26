//! DB-gated app-wiring regression test (Effort B / B1b-1 Task 3, F1 closure).
//! Self-skips without DATABASE_URL. Proves the PDP, held as `Arc<dyn Pdp>`
//! exactly as `CpState` stores it, hot-reloads from the SAME `EventBus` that
//! `RoleRepo` publishes to. A different bus would silently never reload — this
//! is the failure the F1 wiring guards against. Authorization is invoked through
//! the trait object (dyn dispatch), not a concrete `CedarPdp`.
//!
//! Model: `crates/egret-auth/tests/pdp_hot_reload.rs`.

use egret_auth::pdp::CedarPdp;
use egret_auth::roles::RoleRepo;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::ids::{UserId, WorkspaceId};
use egret_events::EventBus;
use egret_metastore::Db;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
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

/// Poll `cond` until it returns true or the budget elapses.
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
async fn arc_dyn_pdp_hot_reloads_from_the_shared_bus() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    // ── The exact wiring the composition root performs: ONE bus, the PDP built
    //    from it and stored behind `Arc<dyn Pdp>` (as `CpState.pdp` is), and a
    //    RoleRepo publishing to that SAME bus.
    let bus = EventBus::new();
    let pdp: Arc<dyn Pdp> = Arc::new(CedarPdp::build(db.clone(), &bus).await.unwrap());
    let repo = RoleRepo::new(db.clone(), bus.clone()); // SAME bus

    // Seed user U, workspace W, a fresh zero-perm workspace role R (via the repo),
    // and membership U→R@W (direct access_grants seed).
    let u = seed_user(&db).await;
    let w = seed_workspace(&db).await;
    let role_name = uniq("appwiring");
    let r_id = repo
        .create("workspace", &role_name, "", None)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = $3 \
         ON CONFLICT DO NOTHING",
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

    // (1) DENIED initially — role R holds zero permissions. Call THROUGH the
    //     trait object to prove dyn-dispatch authorization works.
    let initial = pdp
        .authorize(&subject, action, &Resource::Workspace(w))
        .await
        .unwrap();
    assert!(
        !initial.allow,
        "zero-perm role must deny `{action}` on W before any grant"
    );

    // (2) Grant the permission through the repo — emits RolesChanged on the shared
    //     bus. The `Arc<dyn Pdp>`'s background task must observe it and reload.
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
    assert!(
        allowed,
        "Arc<dyn Pdp> did not hot-reload from the shared bus"
    );

    eprintln!(
        "pdp_app_wiring OK: initial-deny={}, shared-bus grant->allow={}",
        !initial.allow, allowed
    );
}
