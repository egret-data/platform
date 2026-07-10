//! DB-gated regression for security review #1/#4 (migration 0069): a
//! `workspace_members` UPSERT that changes a member's role fires an UPDATE, which
//! 0068's INSERT/DELETE-only triggers left unmirrored — the demoted member kept
//! their old-role grant in `access_grants`. 0069 adds an UPDATE branch (delete
//! OLD-role grant + insert NEW-role grant) and NULL guards.
//!
//! Self-skips without DATABASE_URL. Model: `access_grants_migration.rs`.

use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!("{}@test.local", Uuid::new_v4()))
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn ws_role_id(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap_or_else(|_| panic!("seeded workspace role `{name}` must exist"))
}

/// The set of role_ids `access_grants` currently records for this user/workspace.
async fn grant_role_ids(db: &Db, user: Uuid, ws: Uuid) -> Vec<Uuid> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT role_id FROM access_grants \
         WHERE principal_id = $1 AND scope_type = 'workspace' AND scope_id = $2",
    )
    .bind(user)
    .bind(ws)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    rows.into_iter().map(|(r,)| r).collect()
}

/// The exact UPSERT `MemberRepo::add` issues (`ON CONFLICT ... DO UPDATE SET role`).
async fn upsert_member(db: &Db, ws: Uuid, user: Uuid, role: &str) {
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, principal_type, principal_id, role) \
         VALUES ($1, 'user', $2, $3) \
         ON CONFLICT (workspace_id, principal_type, principal_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(ws)
    .bind(user)
    .bind(role)
    .execute(&db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn upsert_role_downgrade_mirrors_into_access_grants() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    let user = seed_user(&db).await;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'W') RETURNING id")
            .bind(format!("ws-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();

    let admin_role = ws_role_id(&db, "ws_admin").await;
    let viewer_role = ws_role_id(&db, "ws_viewer").await;

    // ── Grant ws_admin (INSERT branch). ──
    upsert_member(&db, ws, user, "ws_admin").await;
    let before = grant_role_ids(&db, user, ws).await;
    assert_eq!(
        before,
        vec![admin_role],
        "INSERT must mirror the ws_admin grant"
    );

    // ── Demote to ws_viewer via the SAME upsert (fires UPDATE, not INSERT). ──
    upsert_member(&db, ws, user, "ws_viewer").await;
    let after = grant_role_ids(&db, user, ws).await;

    assert!(
        after.contains(&viewer_role),
        "UPDATE must mirror the NEW ws_viewer grant; got {after:?}"
    );
    assert!(
        !after.contains(&admin_role),
        "UPDATE must remove the STALE ws_admin grant (demoted admin must not keep admin); got {after:?}"
    );
    assert_eq!(
        after,
        vec![viewer_role],
        "exactly the ws_viewer grant remains after downgrade"
    );

    eprintln!(
        "grant_downgrade OK: ws_admin({admin_role}) -> ws_viewer({viewer_role}) mirrored; \
         stale admin grant removed"
    );
}
