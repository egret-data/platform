//! DB-gated tests for per-workspace feature entitlements; self-skips without `DATABASE_URL`
//! ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_workspace::workspaces::WorkspaceRepo;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

/// `workspaces.created_by` is a real FK to `users(id)` (migration 0003), so a
/// synthetic UUID would fail the insert — seed an actual user.
async fn seed_user(db: &Db) -> UserId {
    UserRepo::new(db.clone())
        .insert_local(
            &format!("{}@test.local", uuid::Uuid::new_v4()),
            "U",
            &hash_password("pw").unwrap(),
        )
        .await
        .unwrap()
}

async fn seed_ws(db: &Db, repo: &WorkspaceRepo) -> WorkspaceId {
    let owner = seed_user(db).await;
    repo.create(&uniq("feat"), "Feature test ws", owner)
        .await
        .expect("create ws")
        .id
}

/// The keys a fresh workspace (no rows at all) has OFF: exactly those whose
/// `default_enabled` is false -- `workflow_containers` since W5a.
fn off_by_default() -> std::collections::HashSet<String> {
    egret_workspace::features::FEATURES
        .iter()
        .filter(|f| !f.default_enabled)
        .map(|f| f.key.to_string())
        .collect()
}

/// An absent row means the key's `default_enabled`: every key that existed
/// before W5a stays ON, and `workflow_containers` is OFF.
#[tokio::test]
async fn absent_row_means_enabled() {
    let Some(db) = connect().await else { return };
    let repo = WorkspaceRepo::new(db.clone());
    let ws = seed_ws(&db, &repo).await;

    let disabled = repo.disabled_features(ws).await.unwrap();

    assert_eq!(
        disabled,
        off_by_default(),
        "a fresh workspace disables only the features that are off by default"
    );
    assert_eq!(
        disabled,
        ["workflow_containers", "workflow_kestra_plugins"]
            .into_iter()
            .map(String::from)
            .collect(),
    );
    for key in egret_workspace::features::FEATURE_KEYS {
        if !off_by_default().contains(*key) {
            assert!(!disabled.contains(*key), "{key} must stay on with no row");
        }
    }

    // The batched read answers the same, including for a workspace that has
    // no row at all (it is not simply missing from the map).
    let batched = repo.disabled_features_for(&[ws.0]).await.unwrap();
    assert_eq!(batched.get(&ws.0), Some(&disabled));
}

/// W5a: the off-by-default key is turned ON by an explicit `true` row, and the
/// dependency on `workflows` is enforced on the resulting state.
#[tokio::test]
async fn workflow_containers_is_turned_on_by_an_explicit_row() {
    let Some(db) = connect().await else { return };
    let repo = WorkspaceRepo::new(db.clone());
    let ws = seed_ws(&db, &repo).await;

    repo.set_features(ws, &[("workflow_containers".into(), true)], None)
        .await
        .unwrap();
    // W5b's `workflow_kestra_plugins` stays at its own default (off).
    let only_plugins: std::collections::HashSet<String> = ["workflow_kestra_plugins".to_string()]
        .into_iter()
        .collect();
    assert_eq!(repo.disabled_features(ws).await.unwrap(), only_plugins);
    assert_eq!(
        repo.disabled_features_for(&[ws.0])
            .await
            .unwrap()
            .get(&ws.0),
        Some(&only_plugins)
    );

    let err = repo
        .set_features(ws, &[("workflows".into(), false)], None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_core::Error::InvalidInput(ref m) if m.contains("workflow_containers requires workflows")),
        "{err:?}"
    );
}

#[tokio::test]
async fn set_features_persists_only_the_disabled_ones() {
    let Some(db) = connect().await else { return };
    let repo = WorkspaceRepo::new(db.clone());
    let ws = seed_ws(&db, &repo).await;

    repo.set_features(ws, &[("apps".into(), false), ("charts".into(), true)], None)
        .await
        .unwrap();

    let disabled = repo.disabled_features(ws).await.unwrap();
    assert!(disabled.contains("apps"));
    assert!(!disabled.contains("charts"));
}

#[tokio::test]
async fn re_enabling_clears_the_disabled_set() {
    let Some(db) = connect().await else { return };
    let repo = WorkspaceRepo::new(db.clone());
    let ws = seed_ws(&db, &repo).await;

    repo.set_features(ws, &[("apps".into(), false)], None)
        .await
        .unwrap();
    repo.set_features(ws, &[("apps".into(), true)], None)
        .await
        .unwrap();

    assert_eq!(repo.disabled_features(ws).await.unwrap(), off_by_default());
}

#[tokio::test]
async fn unknown_feature_key_is_rejected() {
    let Some(db) = connect().await else { return };
    let repo = WorkspaceRepo::new(db.clone());
    let ws = seed_ws(&db, &repo).await;

    let err = repo
        .set_features(ws, &[("teleporter".into(), false)], None)
        .await
        .unwrap_err();

    assert!(
        matches!(err, egret_core::Error::InvalidInput(ref m) if m.contains("teleporter")),
        "expected InvalidInput naming the bad key, got {err:?}"
    );
}

#[tokio::test]
async fn set_features_is_atomic_on_a_bad_key() {
    let Some(db) = connect().await else { return };
    let repo = WorkspaceRepo::new(db.clone());
    let ws = seed_ws(&db, &repo).await;

    let _ = repo
        .set_features(
            ws,
            &[("apps".into(), false), ("teleporter".into(), false)],
            None,
        )
        .await;

    assert_eq!(
        repo.disabled_features(ws).await.unwrap(),
        off_by_default(),
        "a rejected batch must write nothing"
    );
}
