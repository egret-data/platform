//! Migration 0165: grants, pod tokens, and the app_sessions columns that trace a token
//! to a grant and to an impersonator (spec §6.1, §6.2, §5.3).

use egret_metastore::Db;
use uuid::Uuid;

struct Fixture {
    db: Db,
    user: Uuid,
    ws: Uuid,
    app: Uuid,
    version: Uuid,
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
        .bind(format!("origin_{tag}@egret.local"))
        .fetch_one(&db.pool)
        .await
        .expect("insert user");
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Origin Test', $2, 'active') RETURNING id",
        )
        .bind(format!("origin-{tag}"))
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
        let version: Uuid = sqlx::query_scalar(
            "INSERT INTO analytics.app_versions (app_id, version_no) \
             VALUES ($1, 1) RETURNING id",
        )
        .bind(app)
        .fetch_one(&db.pool)
        .await
        .expect("insert version");
        Self {
            db,
            user,
            ws,
            app,
            version,
            login_sid,
        }
    }

    /// An edit grant (no version), unredeemed, ticket valid for a minute.
    async fn insert_ticket(&self, ticket_hash: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO analytics.app_grants \
               (ticket_hash, user_id, workspace_id, app_id, version_id, mode, login_sid, ticket_expires_at) \
             VALUES ($1, $2, $3, $4, NULL, 'edit', $5, now() + interval '1 minute') RETURNING id",
        )
        .bind(ticket_hash)
        .bind(self.user)
        .bind(self.ws)
        .bind(self.app)
        .bind(self.login_sid)
        .fetch_one(&self.db.pool)
        .await
        .expect("insert grant")
    }

    async fn redeem(&self, ticket_hash: &str, cookie_hash: &str) -> u64 {
        sqlx::query(
            "UPDATE analytics.app_grants \
                SET cookie_hash = $2, redeemed_at = now(), cookie_expires_at = now() + interval '8 hours' \
              WHERE ticket_hash = $1 AND redeemed_at IS NULL AND revoked_at IS NULL \
                AND ticket_expires_at > now()",
        )
        .bind(ticket_hash)
        .bind(cookie_hash)
        .execute(&self.db.pool)
        .await
        .expect("redeem")
        .rows_affected()
    }
}

fn unique(prefix: &str) -> String {
    format!("{prefix}{}", Uuid::new_v4().simple())
}

#[tokio::test]
async fn a_ticket_is_spent_by_the_first_redeem_and_not_by_the_second() {
    let fx = Fixture::new().await;
    let ticket = unique("t");
    fx.insert_ticket(&ticket).await;
    assert_eq!(fx.redeem(&ticket, &unique("c")).await, 1);
    assert_eq!(fx.redeem(&ticket, &unique("c")).await, 0);
}

#[tokio::test]
async fn a_grant_must_be_redeemed_and_have_a_cookie_together() {
    let fx = Fixture::new().await;
    let id = fx.insert_ticket(&unique("t")).await;
    // redeemed_at without cookie_hash is the half-spent state the CHECK forbids.
    let err = sqlx::query("UPDATE analytics.app_grants SET redeemed_at = now() WHERE id = $1")
        .bind(id)
        .execute(&fx.db.pool)
        .await
        .expect_err("half-redeemed grant must be rejected");
    assert!(err.to_string().contains("app_grants_check"), "{err}");
}

#[tokio::test]
async fn deleting_the_login_session_deletes_the_grant_and_its_app_sessions() {
    let fx = Fixture::new().await;
    let grant = fx.insert_ticket(&unique("t")).await;
    let session: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.app_sessions \
           (token_hash, user_id, workspace_id, app_id, version_id, mode, login_sid, grant_id, \
            idle_expires_at, absolute_expires_at) \
         VALUES ($1, $2, $3, $4, NULL, 'edit', $5, $6, now() + interval '30 minutes', now() + interval '8 hours') \
         RETURNING id",
    )
    .bind(unique("h"))
    .bind(fx.user)
    .bind(fx.ws)
    .bind(fx.app)
    .bind(fx.login_sid)
    .bind(grant)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert app session with a grant");

    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(fx.login_sid)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM analytics.app_grants WHERE id = $1")
        .bind(grant)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    let sessions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM analytics.app_sessions WHERE id = $1")
            .bind(session)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!((grants, sessions), (0, 0));
}

#[tokio::test]
async fn a_run_pod_token_needs_an_app_and_a_version_and_an_edit_one_a_user_and_neither() {
    let fx = Fixture::new().await;
    let insert = |kind: &'static str, user: Option<Uuid>, app: Option<Uuid>, ver: Option<Uuid>| {
        let pool = fx.db.pool.clone();
        let ws = fx.ws;
        async move {
            sqlx::query(
                "INSERT INTO analytics.app_pod_tokens \
                   (token_hash, kind, user_id, workspace_id, app_id, version_id) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(unique("p"))
            .bind(kind)
            .bind(user)
            .bind(ws)
            .bind(app)
            .bind(ver)
            .execute(&pool)
            .await
        }
    };
    // valid shapes
    insert("edit", Some(fx.user), None, None)
        .await
        .expect("edit");
    insert("run", None, Some(fx.app), Some(fx.version))
        .await
        .expect("run");
    // invalid shapes
    insert("run", None, Some(fx.app), None)
        .await
        .expect_err("run without a version");
    insert("run", None, None, Some(fx.version))
        .await
        .expect_err("run without an app");
    insert("edit", None, None, None)
        .await
        .expect_err("edit without a user");
    insert("edit", Some(fx.user), Some(fx.app), None)
        .await
        .expect_err("edit bound to an app");
}

#[tokio::test]
async fn app_sessions_record_the_impersonator_and_null_it_when_that_user_is_deleted() {
    let fx = Fixture::new().await;
    let admin: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'admin', 'active', 'x') RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("adm")))
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    let session: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.app_sessions \
           (token_hash, user_id, workspace_id, app_id, version_id, mode, login_sid, \
            impersonator_user_id, idle_expires_at, absolute_expires_at) \
         VALUES ($1, $2, $3, $4, NULL, 'edit', $5, $6, now() + interval '30 minutes', now() + interval '8 hours') \
         RETURNING id",
    )
    .bind(unique("h"))
    .bind(fx.user)
    .bind(fx.ws)
    .bind(fx.app)
    .bind(fx.login_sid)
    .bind(admin)
    .fetch_one(&fx.db.pool)
    .await
    .expect("insert");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(admin)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let left: Option<Uuid> =
        sqlx::query_scalar("SELECT impersonator_user_id FROM analytics.app_sessions WHERE id = $1")
            .bind(session)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(left, None);
}
