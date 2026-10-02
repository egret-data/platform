use std::time::Duration;

use egret_apps::grant::{GrantRepo, NewGrant, COOKIE_PREFIX, TICKET_PREFIX};
use egret_apps::session::AppMode;
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

const TICKET_TTL: Duration = Duration::from_secs(30);
const COOKIE_TTL: Duration = Duration::from_secs(8 * 3600);

struct Fixture {
    db: Db,
    user: UserId,
    ws: WorkspaceId,
    app: Uuid,
    version: Uuid,
    login_sid: Uuid,
}

impl Fixture {
    async fn new() -> Self {
        let url = egret_test_db::url().await.expect("test database");
        let db = Db::connect(&url, 10).await.expect("connect");
        let tag = Uuid::new_v4().simple().to_string();
        let user: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, status, password_hash) \
             VALUES ('local', $1, 'test', 'active', 'x') RETURNING id",
        )
        .bind(format!("grant_{tag}@egret.local"))
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let ws: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by, status) \
             VALUES ($1, 'Grant Test', $2, 'active') RETURNING id",
        )
        .bind(format!("grant-{tag}"))
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let login_sid: Uuid = sqlx::query_scalar(
            "INSERT INTO sessions (token_hash, user_id, absolute_expires_at, idle_expires_at) \
             VALUES ($1, $2, now() + interval '8 hours', now() + interval '30 minutes') RETURNING id",
        )
        .bind(format!("{tag}{}", Uuid::new_v4().simple()))
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
            login_sid,
        }
    }

    fn edit(&self) -> NewGrant {
        NewGrant {
            user_id: self.user,
            workspace_id: self.ws,
            app_id: self.app,
            version_id: None,
            mode: AppMode::Edit,
            login_sid: self.login_sid,
            impersonator: None,
        }
    }

    fn run(&self) -> NewGrant {
        NewGrant {
            version_id: Some(self.version),
            mode: AppMode::Run,
            ..self.edit()
        }
    }
}

#[tokio::test]
async fn a_ticket_redeems_once_and_the_replay_is_refused() {
    let fx = Fixture::new().await;
    let repo = GrantRepo::new(fx.db.clone());
    let ticket = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    assert!(ticket.starts_with(TICKET_PREFIX));

    let (grant, cookie) = repo
        .redeem(&ticket, COOKIE_TTL)
        .await
        .unwrap()
        .expect("first redeem");
    assert!(cookie.starts_with(COOKIE_PREFIX));
    assert_eq!(grant.user_id, fx.user);
    assert_eq!(grant.app_id, fx.app);
    assert_eq!(grant.mode, AppMode::Edit);
    assert_eq!(grant.version_id, None);

    assert!(
        repo.redeem(&ticket, COOKIE_TTL).await.unwrap().is_none(),
        "a spent ticket must not redeem again"
    );
}

#[tokio::test]
async fn an_expired_ticket_does_not_redeem() {
    let fx = Fixture::new().await;
    let repo = GrantRepo::new(fx.db.clone());
    let ticket = repo
        .issue_ticket(&fx.edit(), Duration::from_secs(1))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await; // a gap, never `>=` on equal clocks
    assert!(repo.redeem(&ticket, COOKIE_TTL).await.unwrap().is_none());
}

#[tokio::test]
async fn the_cookie_resolves_until_it_is_revoked_expires_or_its_login_session_ends() {
    let fx = Fixture::new().await;
    let repo = GrantRepo::new(fx.db.clone());

    let ticket = repo.issue_ticket(&fx.run(), TICKET_TTL).await.unwrap();
    let (grant, cookie) = repo.redeem(&ticket, COOKIE_TTL).await.unwrap().unwrap();
    let resolved = repo.resolve_cookie(&cookie).await.unwrap().expect("live");
    assert_eq!(resolved.id, grant.id);
    assert_eq!(resolved.version_id, Some(fx.version));

    repo.revoke(grant.id).await.unwrap();
    assert!(
        repo.resolve_cookie(&cookie).await.unwrap().is_none(),
        "revoked"
    );

    let t2 = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    let (g2, c2) = repo
        .redeem(&t2, Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(repo.resolve_cookie(&c2).await.unwrap().is_none(), "expired");
    let _ = g2;

    let t3 = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    let (_, c3) = repo.redeem(&t3, COOKIE_TTL).await.unwrap().unwrap();
    assert!(repo.resolve_cookie(&c3).await.unwrap().is_some());
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(fx.login_sid)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    assert!(
        repo.resolve_cookie(&c3).await.unwrap().is_none(),
        "logout deletes the login session and with it the grant"
    );
}

#[tokio::test]
async fn an_expired_but_undeleted_login_session_stops_the_cookie() {
    let fx = Fixture::new().await;
    let repo = GrantRepo::new(fx.db.clone());
    let ticket = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    let (_, cookie) = repo.redeem(&ticket, COOKIE_TTL).await.unwrap().unwrap();
    sqlx::query("UPDATE sessions SET idle_expires_at = now() - interval '1 second' WHERE id = $1")
        .bind(fx.login_sid)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    assert!(repo.resolve_cookie(&cookie).await.unwrap().is_none());
}

#[tokio::test]
async fn a_disabled_user_cannot_redeem_or_resolve() {
    let fx = Fixture::new().await;
    let repo = GrantRepo::new(fx.db.clone());
    let ticket = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    let (_, cookie) = repo.redeem(&ticket, COOKIE_TTL).await.unwrap().unwrap();
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(fx.user.0)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    assert!(repo.resolve_cookie(&cookie).await.unwrap().is_none());
    let t2 = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    assert!(repo.redeem(&t2, COOKIE_TTL).await.unwrap().is_none());
}

#[tokio::test]
async fn a_ticket_is_not_a_cookie_and_a_cookie_is_not_a_ticket() {
    let fx = Fixture::new().await;
    let repo = GrantRepo::new(fx.db.clone());
    let ticket = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    assert!(repo.resolve_cookie(&ticket).await.unwrap().is_none());
    let t2 = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    let (_, cookie) = repo.redeem(&t2, COOKIE_TTL).await.unwrap().unwrap();
    assert!(repo.redeem(&cookie, COOKIE_TTL).await.unwrap().is_none());
    assert!(repo.redeem("garbage", COOKIE_TTL).await.unwrap().is_none());
}

#[tokio::test]
async fn twenty_concurrent_redeems_of_one_ticket_yield_exactly_one_cookie() {
    let fx = Fixture::new().await;
    let repo = GrantRepo::new(fx.db.clone());
    let ticket = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let (repo, ticket) = (repo.clone(), ticket.clone());
        tasks.push(tokio::spawn(async move {
            repo.redeem(&ticket, COOKIE_TTL).await.unwrap().is_some()
        }));
    }
    let mut won = 0;
    for t in tasks {
        if t.await.unwrap() {
            won += 1;
        }
    }
    assert_eq!(won, 1);
}

#[tokio::test]
async fn the_impersonator_travels_with_the_grant() {
    let fx = Fixture::new().await;
    let repo = GrantRepo::new(fx.db.clone());
    let admin: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, status, password_hash) \
         VALUES ('local', $1, 'admin', 'active', 'x') RETURNING id",
    )
    .bind(format!("admin_{}@egret.local", Uuid::new_v4().simple()))
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    let mut new = fx.edit();
    new.impersonator = Some(UserId(admin));
    let ticket = repo.issue_ticket(&new, TICKET_TTL).await.unwrap();
    let (grant, cookie) = repo.redeem(&ticket, COOKIE_TTL).await.unwrap().unwrap();
    assert_eq!(grant.impersonator, Some(UserId(admin)));
    assert_eq!(
        repo.resolve_cookie(&cookie)
            .await
            .unwrap()
            .unwrap()
            .impersonator,
        Some(UserId(admin))
    );
}

#[tokio::test]
async fn sweep_removes_old_dead_grants_and_keeps_live_ones() {
    let fx = Fixture::new().await;
    let repo = GrantRepo::new(fx.db.clone());
    let live_t = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    let (_, live_c) = repo.redeem(&live_t, COOKIE_TTL).await.unwrap().unwrap();
    let stale = repo.issue_ticket(&fx.edit(), TICKET_TTL).await.unwrap();
    sqlx::query(
        "UPDATE analytics.app_grants SET ticket_expires_at = now() - interval '2 days' \
          WHERE ticket_hash = $1",
    )
    .bind(egret_core::token::hash_token(&stale))
    .execute(&fx.db.pool)
    .await
    .unwrap();
    assert!(repo.sweep(Duration::from_secs(24 * 3600)).await.unwrap() >= 1);
    assert!(repo.redeem(&stale, COOKIE_TTL).await.unwrap().is_none());
    assert!(repo.resolve_cookie(&live_c).await.unwrap().is_some());
}
