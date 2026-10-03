use egret_apps::podtoken::{PodKind, PodTokenRepo, POD_TOKEN_PREFIX};
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

struct Fixture {
    db: Db,
    user: UserId,
    ws: WorkspaceId,
    app: Uuid,
    version: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let url = egret_test_db::url().await.expect("test database");
        let db = Db::connect(&url, 5).await.expect("connect");
        let tag = Uuid::new_v4().simple().to_string();
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, status, password_hash) \
             VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
        )
        .bind(format!("pod_{tag}@egret.local"))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Pod Test', $2, 'active') RETURNING id",
        )
        .bind(format!("pod-{tag}"))
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let app: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.apps (workspace_id, name, owner_user_id) \
             VALUES ($1, 'app', $2) RETURNING id",
        )
        .bind(ws)
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let version: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.app_versions (app_id, version_no, source) \
             VALUES ($1, 1, 'import marimo') RETURNING id",
        )
        .bind(app)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        Self {
            db,
            user: UserId(user),
            ws: WorkspaceId(ws),
            app,
            version,
        }
    }
}

#[tokio::test]
async fn an_edit_token_resolves_to_its_user_and_workspace_and_stores_only_the_hash() {
    let fx = Fixture::new().await;
    let repo = PodTokenRepo::new(fx.db.clone());
    let token = repo.mint_edit(fx.user, fx.ws).await.unwrap();
    assert!(token.starts_with(POD_TOKEN_PREFIX));
    let p = repo.resolve(&token).await.unwrap().expect("live");
    assert_eq!(p.kind, PodKind::Edit);
    assert_eq!(p.user_id, Some(fx.user));
    assert_eq!(p.workspace_id, fx.ws);
    assert_eq!((p.app_id, p.version_id), (None, None));
    let plaintext_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM analytics.app_pod_tokens WHERE token_hash = $1")
            .bind(&token)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(plaintext_rows, 0, "the plaintext must not be stored");
}

#[tokio::test]
async fn a_run_token_is_bound_to_one_app_and_version() {
    let fx = Fixture::new().await;
    let repo = PodTokenRepo::new(fx.db.clone());
    let token = repo.mint_run(fx.ws, fx.app, fx.version).await.unwrap();
    let p = repo.resolve(&token).await.unwrap().expect("live");
    assert_eq!(p.kind, PodKind::Run);
    assert_eq!(p.user_id, None);
    assert_eq!((p.app_id, p.version_id), (Some(fx.app), Some(fx.version)));
}

#[tokio::test]
async fn a_version_of_another_app_cannot_be_pinned() {
    let fx = Fixture::new().await;
    let other = Fixture::new().await;
    let repo = PodTokenRepo::new(fx.db.clone());
    repo.mint_run(fx.ws, fx.app, other.version)
        .await
        .expect_err("version belongs to a different app");
}

#[tokio::test]
async fn unknown_foreign_prefixed_and_revoked_tokens_do_not_resolve() {
    let fx = Fixture::new().await;
    let repo = PodTokenRepo::new(fx.db.clone());
    assert!(repo.resolve("nope").await.unwrap().is_none());
    // An app-session token (`ega_`) is not a pod token even if it were a stored hash.
    assert!(repo.resolve("ega_whatever").await.unwrap().is_none());
    let token = repo.mint_edit(fx.user, fx.ws).await.unwrap();
    let id = repo.resolve(&token).await.unwrap().unwrap().id;
    repo.revoke_for_pod(id).await.unwrap();
    assert!(repo.resolve(&token).await.unwrap().is_none());
}

#[tokio::test]
async fn revoke_edit_for_kills_every_live_edit_token_of_that_user_in_that_workspace_only() {
    let fx = Fixture::new().await;
    let other = Fixture::new().await;
    let repo = PodTokenRepo::new(fx.db.clone());
    let a = repo.mint_edit(fx.user, fx.ws).await.unwrap();
    let b = repo.mint_edit(fx.user, fx.ws).await.unwrap();
    // the same user in another workspace, and another user in this one, are untouched
    let elsewhere = repo.mint_edit(other.user, other.ws).await.unwrap();
    let run = repo.mint_run(fx.ws, fx.app, fx.version).await.unwrap();

    assert_eq!(repo.revoke_edit_for(fx.user, fx.ws).await.unwrap(), 2);
    assert!(repo.resolve(&a).await.unwrap().is_none());
    assert!(repo.resolve(&b).await.unwrap().is_none());
    assert!(repo.resolve(&elsewhere).await.unwrap().is_some());
    assert!(
        repo.resolve(&run).await.unwrap().is_some(),
        "a run token is not an edit token"
    );
    assert_eq!(
        repo.revoke_edit_for(fx.user, fx.ws).await.unwrap(),
        0,
        "already revoked"
    );
}

#[tokio::test]
async fn revoke_run_for_kills_every_live_token_of_that_version_only() {
    let fx = Fixture::new().await;
    let other_version: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.app_versions (app_id, version_no, source) \
         VALUES ($1, 2, 'import marimo') RETURNING id",
    )
    .bind(fx.app)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    let repo = PodTokenRepo::new(fx.db.clone());
    let a = repo.mint_run(fx.ws, fx.app, fx.version).await.unwrap();
    let b = repo.mint_run(fx.ws, fx.app, fx.version).await.unwrap();
    // another version of the same app, and an edit token in the same workspace, are untouched
    let other = repo.mint_run(fx.ws, fx.app, other_version).await.unwrap();
    let edit = repo.mint_edit(fx.user, fx.ws).await.unwrap();

    assert_eq!(
        repo.revoke_run_for(fx.ws, fx.app, fx.version)
            .await
            .unwrap(),
        2
    );
    assert!(repo.resolve(&a).await.unwrap().is_none());
    assert!(repo.resolve(&b).await.unwrap().is_none());
    assert!(
        repo.resolve(&other).await.unwrap().is_some(),
        "a different version's token is not this version's token"
    );
    assert!(
        repo.resolve(&edit).await.unwrap().is_some(),
        "an edit token is not a run token"
    );
    assert_eq!(
        repo.revoke_run_for(fx.ws, fx.app, fx.version)
            .await
            .unwrap(),
        0,
        "already revoked"
    );
}
