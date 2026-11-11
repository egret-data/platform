//! Package D, migration 0219: every write that can change who may query an
//! engine notifies `egret_starrocks_user_sync` at commit, and a rolled-back
//! one does not. DB-gated (`DATABASE_URL`, through `egret-test-db`).
//!
//!   env -u EGRET_ALLOW_DB_SKIP \
//!     DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-engine-mgmt --test starrocks_user_sync_triggers

use std::time::Duration;

use egret_metastore::Db;
use sqlx::postgres::PgListener;
use uuid::Uuid;

const CHANNEL: &str = "egret_starrocks_user_sync";

/// The triggers fire on every committed write in the database, so a test
/// running beside another would hear its writes: a rollback would look like
/// it notified, and a missing trigger would be covered by a sibling's. The
/// tests in this file take turns. (Each test process has its own database,
/// and NOTIFY does not cross databases.)
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn listening(db: &Db) -> PgListener {
    let mut l = PgListener::connect_with(&db.pool).await.expect("listener");
    l.listen(CHANNEL).await.expect("listen");
    l
}

/// Whether a notification arrives within one second.
async fn notified(l: &mut PgListener) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(1), l.recv()).await,
        Ok(Ok(n)) if n.channel() == CHANNEL
    )
}

async fn new_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("sync-{}@trigger.test", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("user")
}

async fn new_workspace(db: &Db) -> Uuid {
    let slug = format!("sync-{}", Uuid::new_v4().simple());
    sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $1) RETURNING id")
        .bind(slug)
        .fetch_one(&db.pool)
        .await
        .expect("workspace")
}

const GRANT_VIEWER: &str = "INSERT INTO access_grants \
    (principal_type, principal_id, role_id, scope_type, scope_id) \
    SELECT 'user', $1, id, 'workspace', $2 FROM roles \
     WHERE scope = 'workspace' AND name = 'ws_viewer'";

#[tokio::test]
async fn a_membership_write_notifies_at_commit() {
    let _turn = SERIAL.lock().await;
    let Some(db) = db().await else { return };
    let (user, ws) = (new_user(&db).await, new_workspace(&db).await);
    let mut l = listening(&db).await;
    sqlx::query(GRANT_VIEWER)
        .bind(user)
        .bind(ws)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        notified(&mut l).await,
        "a workspace grant must wake the sync"
    );
}

#[tokio::test]
async fn a_rolled_back_membership_write_does_not_notify() {
    let _turn = SERIAL.lock().await;
    let Some(db) = db().await else { return };
    let (user, ws) = (new_user(&db).await, new_workspace(&db).await);
    let mut l = listening(&db).await;
    let mut tx = db.pool.begin().await.unwrap();
    sqlx::query(GRANT_VIEWER)
        .bind(user)
        .bind(ws)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(!notified(&mut l).await, "a rollback must notify nothing");
}

#[tokio::test]
async fn disabling_a_user_and_a_group_membership_notify() {
    let _turn = SERIAL.lock().await;
    let Some(db) = db().await else { return };
    let user = new_user(&db).await;
    let mut l = listening(&db).await;
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(user)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(notified(&mut l).await, "disable");
    let group: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(format!("sync-{}", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user)
        .bind(group)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(notified(&mut l).await, "group membership");
}

/// Decided on 2026-11-11 (1): a platform-role grant and a write to the
/// role table wake the sync, since platform admins are synced.
#[tokio::test]
async fn a_platform_admin_grant_and_a_role_write_notify() {
    let _turn = SERIAL.lock().await;
    let Some(db) = db().await else { return };
    let user = new_user(&db).await;
    let mut l = listening(&db).await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(user)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(notified(&mut l).await, "a platform_admin grant");
    sqlx::query("DELETE FROM access_grants WHERE principal_id = $1")
        .bind(user)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(notified(&mut l).await, "its revoke");
    // A no-op rename still runs the trigger, and changes nothing shared.
    sqlx::query("UPDATE roles SET name = name WHERE name = 'platform_admin'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(notified(&mut l).await, "a write to the role table");
}

#[tokio::test]
async fn an_engine_assignment_notifies() {
    let _turn = SERIAL.lock().await;
    let Some(db) = db().await else { return };
    let ws = new_workspace(&db).await;
    let engine = egret_engine_mgmt::EngineMgmtStore::new(db.clone())
        .create_engine(
            &format!("sync-{}", Uuid::new_v4().simple()),
            "starrocks",
            "egret-engines",
            &serde_json::json!({}),
            None,
        )
        .await
        .expect("engine");
    let mut l = listening(&db).await;
    sqlx::query(
        "INSERT INTO engine_mgmt.engine_workspaces (engine_id, workspace_id) VALUES ($1, $2)",
    )
    .bind(engine.id)
    .bind(ws)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(notified(&mut l).await, "assignment");
}
