//! Data Apps P2 storage (migration 0163): drafts, append-only versions, and the
//! foreign keys P1 deferred. Schema section: every guarantee here is the
//! database's, asserted by the exact SQLSTATE a violating statement gets.

use egret_metastore::Db;
use uuid::Uuid;

struct Fixture {
    db: Db,
    user: Uuid,
    ws: Uuid,
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
        .bind(format!("apps_lc_{tag}@egret.local"))
        .fetch_one(&db.pool)
        .await
        .expect("insert user");
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Apps Lifecycle Test', $2, 'active') RETURNING id",
        )
        .bind(format!("apps-lc-{tag}"))
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
        let mut fx = Self {
            db,
            user,
            ws,
            app: Uuid::nil(),
            login_sid,
        };
        fx.app = fx.new_app().await;
        fx
    }

    async fn new_app(&self) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO analytics.apps (workspace_id, name, owner_user_id) \
             VALUES ($1, 'app', $2) RETURNING id",
        )
        .bind(self.ws)
        .bind(self.user)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert app")
    }

    async fn try_version(&self, app: Uuid, no: i32) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO analytics.app_versions (app_id, version_no, source, from_revision, published_by) \
             VALUES ($1, $2, 'import marimo', 1, $3) RETURNING id",
        )
        .bind(app)
        .bind(no)
        .bind(self.user)
        .fetch_one(&self.db.pool)
        .await
    }

    async fn version(&self, app: Uuid, no: i32) -> Uuid {
        self.try_version(app, no).await.expect("insert version")
    }

    /// Insert a run session for `app` pinned to `version`.
    async fn try_run_session(&self, app: Uuid, version: Uuid) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            "INSERT INTO analytics.app_sessions \
             (token_hash, user_id, workspace_id, app_id, version_id, mode, login_sid, \
              idle_expires_at, absolute_expires_at) \
             VALUES ($1, $2, $3, $4, $5, 'run', $6, \
                     now() + interval '30 minutes', now() + interval '8 hours') RETURNING id",
        )
        .bind(Uuid::new_v4().simple().to_string())
        .bind(self.user)
        .bind(self.ws)
        .bind(app)
        .bind(version)
        .bind(self.login_sid)
        .fetch_one(&self.db.pool)
        .await
    }

    async fn count(&self, sql: &'static str, id: Uuid) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(&self.db.pool)
            .await
            .expect("count")
    }
}

/// SQLSTATE of a failed statement.
fn code<T: std::fmt::Debug>(r: Result<T, sqlx::Error>) -> String {
    let e = r.expect_err("statement must fail");
    e.as_database_error()
        .and_then(|d| d.code().map(|c| c.to_string()))
        .unwrap_or_else(|| panic!("expected a database error, got {e:?}"))
}

/// Name of the constraint a failed statement violated.
fn constraint<T: std::fmt::Debug>(r: Result<T, sqlx::Error>) -> String {
    let e = r.expect_err("statement must fail");
    e.as_database_error()
        .and_then(|d| d.constraint().map(str::to_string))
        .unwrap_or_else(|| panic!("expected a constraint violation, got {e:?}"))
}

#[tokio::test]
async fn versions_cannot_be_updated() {
    let fx = Fixture::new().await;
    let v = fx.version(fx.app, 1).await;
    let r = sqlx::query("UPDATE analytics.app_versions SET source = 'x' WHERE id = $1")
        .bind(v)
        .execute(&fx.db.pool)
        .await;
    assert_eq!(code(r), "23001");
    let source: String =
        sqlx::query_scalar("SELECT source FROM analytics.app_versions WHERE id = $1")
            .bind(v)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(source, "import marimo");
}

#[tokio::test]
async fn versions_cannot_be_deleted_directly() {
    let fx = Fixture::new().await;
    let v = fx.version(fx.app, 1).await;
    let r = sqlx::query("DELETE FROM analytics.app_versions WHERE id = $1")
        .bind(v)
        .execute(&fx.db.pool)
        .await;
    assert_eq!(code(r), "23001");
    assert_eq!(
        fx.count(
            "SELECT count(*) FROM analytics.app_versions WHERE id = $1",
            v
        )
        .await,
        1
    );
}

/// The trigger's one exception: a delete that is the ON DELETE CASCADE from the
/// app (itself reached from the workspace). Includes a restore (a version whose
/// `restored_from` points at another version of the same app), a draft, and a
/// run session pinned to a version, so every edge the cascade crosses is present.
#[tokio::test]
async fn deleting_the_workspace_cascades_through_versions() {
    let fx = Fixture::new().await;
    let v1 = fx.version(fx.app, 1).await;
    let v2: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.app_versions (app_id, version_no, source, restored_from) \
         SELECT app_id, 2, source, id FROM analytics.app_versions WHERE id = $1 RETURNING id",
    )
    .bind(v1)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert restore");
    sqlx::query(
        "INSERT INTO analytics.app_drafts (app_id, source, updated_by) VALUES ($1, 'draft', $2)",
    )
    .bind(fx.app)
    .bind(fx.user)
    .execute(&fx.db.pool)
    .await
    .expect("insert draft");
    fx.try_run_session(fx.app, v1)
        .await
        .expect("run session on v1");
    fx.try_run_session(fx.app, v2)
        .await
        .expect("run session on v2");

    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(fx.ws)
        .execute(&fx.db.pool)
        .await
        .expect("deleting the workspace must cascade through the append-only versions");

    for sql in [
        "SELECT count(*) FROM analytics.apps WHERE id = $1",
        "SELECT count(*) FROM analytics.app_versions WHERE app_id = $1",
        "SELECT count(*) FROM analytics.app_drafts WHERE app_id = $1",
        "SELECT count(*) FROM analytics.app_sessions WHERE app_id = $1",
    ] {
        assert_eq!(fx.count(sql, fx.app).await, 0, "{sql}");
    }
}

#[tokio::test]
async fn version_numbers_are_unique_per_app() {
    let fx = Fixture::new().await;
    fx.version(fx.app, 1).await;
    assert_eq!(code(fx.try_version(fx.app, 1).await), "23505");
    assert_eq!(
        constraint(fx.try_version(fx.app, 1).await),
        "app_versions_app_no_key"
    );
    // The same number on a DIFFERENT app is fine.
    let other = fx.new_app().await;
    fx.version(other, 1).await;
}

#[tokio::test]
async fn draft_source_over_one_mib_is_rejected_by_the_schema() {
    let fx = Fixture::new().await;
    let insert = |source: String| {
        sqlx::query("INSERT INTO analytics.app_drafts (app_id, source) VALUES ($1, $2)")
            .bind(fx.app)
            .bind(source)
            .execute(&fx.db.pool)
    };
    assert_eq!(code(insert("a".repeat(1_048_577)).await), "23514");
    // Exactly 1 MiB is the limit, not past it.
    insert("a".repeat(1_048_576))
        .await
        .expect("a 1 MiB draft is accepted");
}

#[tokio::test]
async fn a_run_session_must_pin_a_version_of_its_own_app() {
    let fx = Fixture::new().await;
    let app_b = fx.new_app().await;
    let v_b = fx.version(app_b, 1).await;
    let v_a = fx.version(fx.app, 1).await;

    assert_eq!(code(fx.try_run_session(fx.app, v_b).await), "23503");
    assert_eq!(
        constraint(fx.try_run_session(fx.app, v_b).await),
        "app_sessions_version_app_fk"
    );
    // A version that does not exist at all is refused the same way.
    assert_eq!(
        code(fx.try_run_session(fx.app, Uuid::new_v4()).await),
        "23503"
    );
    // Control: its own app's version is accepted.
    fx.try_run_session(fx.app, v_a)
        .await
        .expect("a run session pinned to its own app's version");
}

#[tokio::test]
async fn deleting_a_folder_uncategorizes_its_apps() {
    let fx = Fixture::new().await;
    let folder: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.folders (workspace_id, entity_type, name, created_by) \
         VALUES ($1, 'app', $2, $3) RETURNING id",
    )
    .bind(fx.ws)
    .bind(format!("f-{}", Uuid::new_v4().simple()))
    .bind(fx.user)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert folder");
    sqlx::query("UPDATE analytics.apps SET folder_id = $2 WHERE id = $1")
        .bind(fx.app)
        .bind(folder)
        .execute(&fx.db.pool)
        .await
        .expect("file the app");
    // The FK exists: a folder id that is not a folder is refused.
    let r = sqlx::query("UPDATE analytics.apps SET folder_id = $2 WHERE id = $1")
        .bind(fx.app)
        .bind(Uuid::new_v4())
        .execute(&fx.db.pool)
        .await;
    assert_eq!(code(r), "23503");

    sqlx::query("DELETE FROM analytics.folders WHERE id = $1")
        .bind(folder)
        .execute(&fx.db.pool)
        .await
        .expect("delete folder");
    let folder_id: Option<Uuid> =
        sqlx::query_scalar("SELECT folder_id FROM analytics.apps WHERE id = $1")
            .bind(fx.app)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(folder_id, None);
}
