//! EO3 end-to-end: a live `CedarPdp` hot-flips Legacy -> Shadow -> Compiled -> Legacy
//! through the switch row + `RolesChanged`, with decisions unchanged throughout.
//! DB-gated via egret_test_db; requires DATABASE_URL.

use egret_auth::pdp::source_switch::{write_source, PolicySource};
use egret_auth::pdp::CedarPdp;
use egret_auth::roles::RoleRepo;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::ids::{UserId, WorkspaceId};
use egret_events::EventBus;
use egret_metastore::Db;
use std::time::Duration;
use uuid::Uuid;

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

/// Decisions over the corpus: (action, resource) pairs for the subject.
async fn decisions(pdp: &CedarPdp, s: &Subject, w: WorkspaceId, other: WorkspaceId) -> Vec<bool> {
    let mut out = Vec::new();
    for (action, res) in [
        ("dashboard:read", Resource::Workspace(w)),
        ("dashboard:read", Resource::Workspace(other)),
        ("dashboard:manage", Resource::Workspace(w)),
        ("dashboard:read", Resource::Platform),
    ] {
        out.push(pdp.authorize(s, action, &res).await.unwrap().allow);
    }
    out
}

#[tokio::test]
async fn pdp_hot_flips_between_sources_and_rolls_back() {
    let url = egret_test_db::url().await.expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    sqlx::query("DELETE FROM perm.pdp_policy_source")
        .execute(&db.pool)
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

    let bus = EventBus::new();
    let repo = RoleRepo::new(db.clone(), bus.clone());
    let role_name = format!("flip-{}", Uuid::new_v4().simple());
    let r_id = repo
        .create("workspace", &role_name, "", None)
        .await
        .unwrap();
    repo.set_permissions(r_id, &["dashboard:read".into()])
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

    let marker_action = "chart:read";
    let baseline = decisions(&pdp, &subject, w, other).await;
    assert!(baseline[0], "seed grant must allow dashboard:read on W");
    assert!(!baseline[1], "no grant on the other workspace");

    let mut marker_on = false;
    let mut flip = |src: PolicySource| {
        let (db, repo, pdp, subject) = (&db, &repo, &pdp, &subject);
        let target = !marker_on;
        marker_on = target;
        async move {
            write_source(db, src, "e2e@egret.local").await.unwrap();
            // Barrier: toggle an extra permission on the same role (one role per
            // principal+scope), emitting RolesChanged AFTER the switch write.
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
            assert!(landed, "reload did not land after flip to {src:?}");
        }
    };

    let stable = |d: &[bool]| d.to_vec();
    let want = stable(&baseline);

    for src in [
        PolicySource::Shadow,
        PolicySource::Compiled,
        PolicySource::Legacy,
    ] {
        flip(src).await;
        let got = decisions(&pdp, &subject, w, other).await;
        assert_eq!(stable(&got), want, "decisions changed under {src:?}");
        if src == PolicySource::Compiled {
            let compiled = CedarPdp::compiled_ids_for_test(&db).await.unwrap();
            assert!(!compiled.is_empty());
            assert_eq!(
                pdp.live_served_ids_for_test(),
                compiled,
                "live PDP must hold the compiled set"
            );
            assert_eq!(
                CedarPdp::served_policy_ids_for_test(&db).await.unwrap(),
                CedarPdp::compiled_ids_for_test(&db).await.unwrap(),
                "Compiled must serve the compiled set"
            );
        }
        if src == PolicySource::Legacy {
            let legacy = CedarPdp::legacy_ids_for_test(&db).await.unwrap();
            assert!(!legacy.is_empty());
            assert_eq!(
                pdp.live_served_ids_for_test(),
                legacy,
                "live PDP must hold the legacy set after rollback"
            );
            assert_eq!(
                CedarPdp::served_policy_ids_for_test(&db).await.unwrap(),
                CedarPdp::legacy_ids_for_test(&db).await.unwrap(),
                "rollback must serve the legacy set"
            );
        }
    }

    sqlx::query("DELETE FROM perm.pdp_policy_source")
        .execute(&db.pool)
        .await
        .unwrap();
}
