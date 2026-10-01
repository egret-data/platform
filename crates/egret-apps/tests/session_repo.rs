use std::time::Duration;

use egret_apps::session::{AppMode, AppSessionRepo, NewAppSession};
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

const IDLE: Duration = Duration::from_secs(1800);
const ABS: Duration = Duration::from_secs(8 * 3600);

struct Fixture {
    db: Db,
    user: UserId,
    ws: WorkspaceId,
    app: Uuid,
    login_sid: Uuid,
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
        .bind(format!("apps_{tag}@egret.local"))
        .fetch_one(&db.pool)
        .await
        .expect("insert user");
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Apps Test', $2, 'active') RETURNING id",
        )
        .bind(format!("apps-{tag}"))
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .expect("insert workspace");
        let login_sid: Uuid = sqlx::query_scalar(
            "INSERT INTO sessions (token_hash, user_id, absolute_expires_at, idle_expires_at) \
             VALUES ($1, $2, now() + interval '8 hours', now() + interval '30 minutes') RETURNING id",
        )
        .bind(format!("{tag}{}", Uuid::new_v4().simple()))
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .expect("insert login session");
        let app: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.apps (workspace_id, name, owner_user_id) \
             VALUES ($1, 'app', $2) RETURNING id",
        )
        .bind(ws)
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .expect("insert app");
        Self {
            db,
            user: UserId(user),
            ws: WorkspaceId(ws),
            app,
            login_sid,
        }
    }

    fn new_run(&self) -> NewAppSession {
        NewAppSession {
            user_id: self.user,
            workspace_id: self.ws,
            app_id: self.app,
            version_id: Some(Uuid::new_v4()),
            mode: AppMode::Run,
            login_sid: self.login_sid,
        }
    }

    // Callers scope the SQL by `app_id = $1`: each fixture owns its own app, so tests stay
    // independent even though the test database is shared within a process.
    async fn scalar<T>(&self, sql: &'static str) -> T
    where
        T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send + Unpin,
    {
        sqlx::query_scalar(sql)
            .bind(self.app)
            .fetch_one(&self.db.pool)
            .await
            .expect("scalar")
    }

    async fn row2<A, B>(&self, sql: &'static str) -> (A, B)
    where
        (A, B): for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
    {
        sqlx::query_as(sql)
            .bind(self.app)
            .fetch_one(&self.db.pool)
            .await
            .expect("row2")
    }

    async fn exec(&self, sql: &'static str, id: Uuid) {
        sqlx::query(sql)
            .bind(id)
            .execute(&self.db.pool)
            .await
            .expect("exec");
    }

    async fn exec_with_hash(&self, sql: &'static str, token: &str) {
        sqlx::query(sql)
            .bind(egret_core::token::hash_token(token))
            .execute(&self.db.pool)
            .await
            .expect("exec_with_hash");
    }
}

#[tokio::test]
async fn mint_then_resolve_returns_the_session_and_stores_only_the_hash() {
    let fx = Fixture::new().await;
    let repo = AppSessionRepo::new(fx.db.clone());
    let token = repo.mint(&fx.new_run(), IDLE, ABS).await.unwrap();
    assert!(token.starts_with("ega_"));
    let s = repo.resolve(&token, IDLE).await.unwrap().expect("live");
    assert_eq!(s.user_id, fx.user);
    assert_eq!(s.app_id, fx.app);
    assert!(matches!(s.mode, AppMode::Run));
    let stored: String = fx
        .scalar("SELECT token_hash FROM analytics.app_sessions WHERE app_id = $1")
        .await;
    assert_ne!(stored, token);
    assert_eq!(stored, egret_core::token::hash_token(&token));
}

#[tokio::test]
async fn unknown_revoked_idle_and_absolute_expired_tokens_do_not_resolve() {
    let fx = Fixture::new().await;
    let repo = AppSessionRepo::new(fx.db.clone());
    assert!(repo.resolve("ega_nope", IDLE).await.unwrap().is_none());

    let revoked = repo.mint(&fx.new_run(), IDLE, ABS).await.unwrap();
    repo.revoke(&revoked).await.unwrap();
    assert!(repo.resolve(&revoked, IDLE).await.unwrap().is_none());

    let idle = repo.mint(&fx.new_run(), IDLE, ABS).await.unwrap();
    fx.exec_with_hash("UPDATE analytics.app_sessions SET idle_expires_at = now() - interval '1 second' WHERE token_hash = $1", &idle).await;
    assert!(repo.resolve(&idle, IDLE).await.unwrap().is_none());

    let abs = repo.mint(&fx.new_run(), IDLE, ABS).await.unwrap();
    fx.exec_with_hash("UPDATE analytics.app_sessions SET idle_expires_at = now() - interval '2 seconds', absolute_expires_at = now() - interval '1 second' WHERE token_hash = $1", &abs).await;
    assert!(repo.resolve(&abs, IDLE).await.unwrap().is_none());
}

#[tokio::test]
async fn resolve_slides_idle_expiry_but_never_past_absolute() {
    let fx = Fixture::new().await;
    let repo = AppSessionRepo::new(fx.db.clone());
    let token = repo
        .mint(
            &fx.new_run(),
            Duration::from_secs(60),
            Duration::from_secs(90),
        )
        .await
        .unwrap();
    let before: chrono::DateTime<chrono::Utc> = fx
        .scalar("SELECT idle_expires_at FROM analytics.app_sessions WHERE app_id = $1")
        .await;
    tokio::time::sleep(Duration::from_millis(1100)).await; // a gap, so equal clocks cannot pass
    repo.resolve(&token, Duration::from_secs(3600))
        .await
        .unwrap()
        .expect("live");
    let (idle, abs): (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) =
        fx.row2("SELECT idle_expires_at, absolute_expires_at FROM analytics.app_sessions WHERE app_id = $1").await;
    assert!(idle > before);
    assert_eq!(idle, abs); // clamped to absolute
}

#[tokio::test]
async fn deleting_the_login_session_revokes_by_cascade() {
    let fx = Fixture::new().await;
    let repo = AppSessionRepo::new(fx.db.clone());
    let token = repo.mint(&fx.new_run(), IDLE, ABS).await.unwrap();
    fx.exec("DELETE FROM sessions WHERE id = $1", fx.login_sid)
        .await;
    assert!(repo.resolve(&token, IDLE).await.unwrap().is_none());
}

#[tokio::test]
async fn edit_mode_with_a_version_is_rejected_by_the_schema() {
    let fx = Fixture::new().await;
    let repo = AppSessionRepo::new(fx.db.clone());
    let mut bad = fx.new_run();
    bad.mode = AppMode::Edit; // version_id is Some
    assert_eq!(sqlstate(repo.mint(&bad, IDLE, ABS).await), "23514");
}

#[tokio::test]
async fn session_workspace_must_match_the_apps_workspace() {
    let fx = Fixture::new().await;
    let other_ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Other', $2, 'active') RETURNING id",
    )
    .bind(format!("apps-other-{}", Uuid::new_v4().simple()))
    .bind(fx.user.0)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert other workspace");
    let mut bad = fx.new_run();
    bad.workspace_id = WorkspaceId(other_ws);
    let repo = AppSessionRepo::new(fx.db.clone());
    assert_eq!(sqlstate(repo.mint(&bad, IDLE, ABS).await), "23503");
}

/// SQLSTATE of the database error a failed `mint` carries.
fn sqlstate(r: egret_core::Result<String>) -> String {
    let egret_core::Error::Internal(e) = r.expect_err("mint must fail") else {
        panic!("expected Internal");
    };
    e.downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .and_then(|d| d.code().map(|c| c.to_string()))
        .expect("database error with SQLSTATE")
}
