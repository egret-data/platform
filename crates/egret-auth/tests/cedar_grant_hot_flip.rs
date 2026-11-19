//! EO4 end-to-end: a live `CedarPdp` hot-flips grant source Legacy -> (fold) Unified
//! -> Legacy through the switch row + `RolesChanged`, with decisions unchanged, and the
//! unified read is LIVE (the access_grants trigger keeps the mirror current).
//! DB-gated via egret_test_db; requires DATABASE_URL.

use egret_auth::events::RolesChanged;
use egret_auth::grants::AccessGrantStore;
use egret_auth::pdp::grant_source_switch::{read_source, read_state, write_source, GrantSource};
use egret_auth::pdp::CedarPdp;
use egret_auth::roles::RoleRepo;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::ids::{UserId, WorkspaceId};
use egret_events::EventBus;
use egret_metastore::Db;
use std::time::Duration;
use uuid::Uuid;

/// Serialises the tests that mutate the global grant-source switch row.
static SWITCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Restores the switch to Legacy and removes seeded rows even if an assertion fails.
struct Cleanup {
    db: Db,
    user: Uuid,
    workspaces: Vec<Uuid>,
    roles: Vec<Uuid>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let (db, user, workspaces) = (self.db.clone(), self.user, self.workspaces.clone());
        let roles = self.roles.clone();
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let _ = write_source(&db, GrantSource::Legacy, "e2e-cleanup").await;
                let _ = sqlx::query("DELETE FROM access_grants WHERE principal_id = $1")
                    .bind(user)
                    .execute(&db.pool)
                    .await;
                let _ = sqlx::query("DELETE FROM perm.cedar_grants WHERE principal_id = $1")
                    .bind(user)
                    .execute(&db.pool)
                    .await;
                for r in roles {
                    let _ = sqlx::query("DELETE FROM roles WHERE id = $1")
                        .bind(r)
                        .execute(&db.pool)
                        .await;
                }
                let _ = sqlx::query("DELETE FROM users WHERE id = $1")
                    .bind(user)
                    .execute(&db.pool)
                    .await;
                for w in workspaces {
                    let _ = sqlx::query("DELETE FROM workspaces WHERE id = $1")
                        .bind(w)
                        .execute(&db.pool)
                        .await;
                }
            });
        })
        .join();
    }
}

/// Decisions over the corpus: (action, resource) pairs for the subject.
async fn decisions(pdp: &CedarPdp, s: &Subject, w: WorkspaceId, other: WorkspaceId) -> Vec<bool> {
    let mut out = Vec::new();
    for (action, res) in [
        ("dashboard:read", Resource::Workspace(w)),
        ("dashboard:read", Resource::Workspace(other)),
        ("dashboard:manage", Resource::Workspace(w)),
        ("dashboard:manage", Resource::Workspace(other)),
        ("dashboard:read", Resource::Platform),
    ] {
        out.push(pdp.authorize(s, action, &res).await.unwrap().allow);
    }
    out
}

#[tokio::test]
async fn grant_source_hot_flips_stays_live_and_rolls_back() {
    let _g = SWITCH_LOCK.lock().await;
    let url = egret_test_db::url().await.expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    write_source(&db, GrantSource::Legacy, "e2e-setup")
        .await
        .unwrap();

    let u = UserId(
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
        )
        .bind(format!("{}@test.local", Uuid::new_v4()))
        .fetch_one(&db.pool)
        .await
        .unwrap(),
    );
    let mut ws = Vec::new();
    for _ in 0..2 {
        ws.push(WorkspaceId(
            sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO workspaces (slug, name) VALUES ($1, 'W') RETURNING id",
            )
            .bind(format!("ws-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        ));
    }
    let (w, other) = (ws[0], ws[1]);
    let mut cleanup = Cleanup {
        db: db.clone(),
        user: u.0,
        workspaces: vec![w.0, other.0],
        roles: vec![],
    };

    let bus = EventBus::new();
    let repo = RoleRepo::new(db.clone(), bus.clone());
    let role_name = format!("flip-{}", Uuid::new_v4().simple());
    let r_id = repo
        .create("workspace", &role_name, "", None)
        .await
        .unwrap();
    cleanup.roles.push(r_id);
    repo.set_permissions(r_id, &["dashboard:read".into()])
        .await
        .unwrap();
    // A second role that would allow the currently-denied dashboard:manage.
    let manage_role = repo
        .create(
            "workspace",
            &format!("flip-manage-{}", Uuid::new_v4().simple()),
            "",
            None,
        )
        .await
        .unwrap();
    cleanup.roles.push(manage_role);
    repo.set_permissions(manage_role, &["dashboard:manage".into()])
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3) ON CONFLICT DO NOTHING",
    )
    .bind(u.0)
    .bind(r_id)
    .bind(w.0)
    .execute(&db.pool)
    .await
    .unwrap();

    let pdp = CedarPdp::build(db.clone(), &bus).await.unwrap();
    let subject = Subject {
        user_id: u,
        group_ids: vec![],
    };

    // Phase 1: legacy baseline.
    let baseline = decisions(&pdp, &subject, w, other).await;
    assert!(baseline[0], "seed grant must allow dashboard:read on W");
    assert!(!baseline[1], "no grant on the other workspace");
    assert!(
        !baseline[2],
        "dashboard:manage is denied before the live grant"
    );

    // Barrier for unchanged-decision phases: toggle a marker permission on the same
    // role (emits RolesChanged AFTER the switch write) and wait until it lands.
    let marker_action = "chart:read";
    let mut marker_on = false;
    let mut barrier = || {
        let (repo, pdp, subject) = (&repo, &pdp, &subject);
        let target = !marker_on;
        marker_on = target;
        async move {
            let mut perms: Vec<String> = vec!["dashboard:read".into()];
            if target {
                perms.push(marker_action.into());
            }
            repo.set_permissions(r_id, &perms).await.unwrap();
            let landed = poll_until(
                || async {
                    pdp.authorize(subject, marker_action, &Resource::Workspace(w))
                        .await
                        .unwrap()
                        .allow
                        == target
                },
                Duration::from_secs(5),
            )
            .await;
            assert!(landed, "reload did not land");
        }
    };

    // Phase 2: fold + flip to Unified; decisions identical.
    let store = AccessGrantStore::new(db.clone());
    store.fold().await.unwrap();
    write_source(&db, GrantSource::Unified, "e2e@egret.local")
        .await
        .unwrap();
    barrier().await;
    assert_eq!(read_source(&db).await, GrantSource::Unified);
    assert_eq!(
        decisions(&pdp, &subject, w, other).await,
        baseline,
        "decisions changed under Unified"
    );

    // Phase 3: live-sync. Insert a legacy grant while Unified; the trigger mirrors it.
    let live_grant: Uuid = sqlx::query_scalar(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'workspace', $3) RETURNING id",
    )
    .bind(u.0)
    .bind(manage_role)
    .bind(other.0)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    bus.publish(RolesChanged);
    let allowed = poll_until(
        || async {
            pdp.authorize(&subject, "dashboard:manage", &Resource::Workspace(other))
                .await
                .unwrap()
                .allow
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(
        allowed,
        "Unified read must be live: new grant not reflected"
    );

    sqlx::query("DELETE FROM access_grants WHERE id = $1")
        .bind(live_grant)
        .execute(&db.pool)
        .await
        .unwrap();
    bus.publish(RolesChanged);
    let denied = poll_until(
        || async {
            !pdp.authorize(&subject, "dashboard:manage", &Resource::Workspace(other))
                .await
                .unwrap()
                .allow
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(denied, "removed grant must stop allowing under Unified");
    assert_eq!(decisions(&pdp, &subject, w, other).await, baseline);

    // Phase 3b (DISCRIMINATING): a unified-store-ONLY row (absent from access_grants).
    // Only passes if grants_for routes to perm.cedar_grants under Unified, and Legacy
    // must not see it. Cleanup deletes perm.cedar_grants by principal_id = user.
    let only_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO perm.cedar_grants (id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at) \
         VALUES ($1,'user',$2,$3,'workspace',$4,NULL,'allow',NULL)",
    )
    .bind(only_id)
    .bind(u.0)
    .bind(manage_role)
    .bind(other.0)
    .execute(&db.pool)
    .await
    .unwrap();
    bus.publish(RolesChanged);
    let manage_other = || async {
        pdp.authorize(&subject, "dashboard:manage", &Resource::Workspace(other))
            .await
            .unwrap()
            .allow
    };
    assert!(
        poll_until(manage_other, Duration::from_secs(5)).await,
        "Unified must route to perm.cedar_grants (row exists only there)"
    );
    write_source(&db, GrantSource::Legacy, "e2e@egret.local")
        .await
        .unwrap();
    bus.publish(RolesChanged);
    assert!(
        poll_until(|| async { !manage_other().await }, Duration::from_secs(5)).await,
        "Legacy must NOT see the unified-only row"
    );
    sqlx::query("DELETE FROM perm.cedar_grants WHERE id = $1")
        .bind(only_id)
        .execute(&db.pool)
        .await
        .unwrap();
    write_source(&db, GrantSource::Unified, "e2e@egret.local")
        .await
        .unwrap();
    bus.publish(RolesChanged);
    assert!(
        poll_until(|| async { !manage_other().await }, Duration::from_secs(5)).await,
        "unified-only row deleted"
    );

    // Phase 4: rollback to Legacy.
    write_source(&db, GrantSource::Legacy, "e2e@egret.local")
        .await
        .unwrap();
    barrier().await;
    assert_eq!(
        decisions(&pdp, &subject, w, other).await,
        baseline,
        "decisions changed after rollback to Legacy"
    );

    // Phase 5 (EO5): Shadow observes but serves access_grants. A mirror-only row is
    // NOT honoured (mutation-killer: Shadow must not route to perm.cedar_grants).
    let shadow_only = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO perm.cedar_grants (id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at) \
         VALUES ($1,'user',$2,$3,'workspace',$4,NULL,'allow',NULL)",
    )
    .bind(shadow_only)
    .bind(u.0)
    .bind(manage_role)
    .bind(other.0)
    .execute(&db.pool)
    .await
    .unwrap();
    write_source(&db, GrantSource::Shadow, "e2e@egret.local")
        .await
        .unwrap();
    barrier().await;
    assert_eq!(read_state(&db).await.unwrap().source, GrantSource::Shadow);
    assert_eq!(
        decisions(&pdp, &subject, w, other).await,
        baseline,
        "Shadow must serve legacy decisions; the mirror-only row must not be honoured"
    );
}
