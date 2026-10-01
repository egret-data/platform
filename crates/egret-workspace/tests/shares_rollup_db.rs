//! Workspace object-grant roll-up view (read-only). Asserts
//! `AccessViewsRepo::list_object_grants` returns every OBJECT grant whose object
//! lives in THIS workspace — enriched with object name, role name, principal name
//! and effect — and, critically, EXCLUDES grants on objects that live in another
//! workspace (the IDOR guard). DB-gated; self-skips without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS —
//! matching the sibling `members_ax1.rs` / `access_views_db.rs` harness (same
//! `connect`/`uniq` setup).

use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_workspace::access_views::AccessViewsRepo;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

async fn seed_user_email(db: &Db, email: &str) -> UserId {
    UserRepo::new(db.clone())
        .insert_local(email, "U", &hash_password("pw").unwrap())
        .await
        .unwrap()
}

async fn seed_group(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// A registered engine (workspace-scoped).
async fn seed_database(db: &Db, ws: WorkspaceId) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port) \
         VALUES ($1, $2, 'trino', 'localhost', 8080) RETURNING id",
    )
    .bind(ws.0)
    .bind(uniq("engine"))
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// A chart (via a table-backed dataset) in the given workspace; returns (chart_id, name).
async fn seed_chart(db: &Db, ws: WorkspaceId, database_id: Uuid) -> (Uuid, String) {
    let dataset_id: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.datasets (workspace_id, database_id, name, table_name) \
         VALUES ($1, $2, $3, 't') RETURNING id",
    )
    .bind(ws.0)
    .bind(database_id)
    .bind(uniq("ds"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let name = uniq("Revenue chart");
    let chart_id: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.charts (workspace_id, dataset_id, name, viz_type, params) \
         VALUES ($1, $2, $3, 'bar', '{}'::jsonb) RETURNING id",
    )
    .bind(ws.0)
    .bind(dataset_id)
    .bind(&name)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    (chart_id, name)
}

async fn role_id(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// A data app in the given workspace; returns (app_id, name).
async fn seed_app(db: &Db, ws: WorkspaceId) -> (Uuid, String) {
    let name = uniq("Sales app");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.apps (workspace_id, name) VALUES ($1, $2) RETURNING id",
    )
    .bind(ws.0)
    .bind(&name)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    (id, name)
}

/// Insert an object-scoped grant (allow/deny) on `object_id` for a principal.
async fn grant_object(
    db: &Db,
    ptype: &str,
    pid: Uuid,
    role: Uuid,
    object_type: &str,
    object_id: Uuid,
    effect: &str,
) {
    grant_object_expiring(db, ptype, pid, role, object_type, object_id, effect, None).await
}

/// Insert an object-scoped grant (allow/deny) on `object_id` for a principal,
/// with an optional `expires_at`.
#[allow(clippy::too_many_arguments)]
async fn grant_object_expiring(
    db: &Db,
    ptype: &str,
    pid: Uuid,
    role: Uuid,
    object_type: &str,
    object_id: Uuid,
    effect: &str,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) {
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, expires_at) \
         VALUES ($1, $2, $3, 'object', $4, $5, $6, $7)",
    )
    .bind(ptype)
    .bind(pid)
    .bind(role)
    .bind(object_id)
    .bind(object_type)
    .bind(effect)
    .bind(expires_at)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// The roll-up lists exactly the object grants whose object lives in THIS
/// workspace (allow + deny, enriched), and NEVER a grant on an object that lives
/// in another workspace (the cross-workspace IDOR guard).
#[tokio::test]
async fn lists_object_grants_scoped_to_workspace() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let repo = AccessViewsRepo::new(db.clone());
    let wrepo = WorkspaceRepo::new(db.clone());

    let owner = seed_user_email(
        &db,
        &format!("own-{}@t.local", uuid::Uuid::new_v4().simple()),
    )
    .await;

    // Workspace W: chart C, granted to group G (ws_viewer, allow) and user U (deny).
    let w = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();
    let database_id = seed_database(&db, w.id).await;
    let (chart_c, chart_c_name) = seed_chart(&db, w.id, database_id).await;
    let group_name = uniq("Analytics Team");
    let g = seed_group(&db, &group_name).await;
    let u_email = format!("u-{}@t.local", uuid::Uuid::new_v4().simple());
    let u = seed_user_email(&db, &u_email).await;
    let viewer = role_id(&db, "ws_viewer").await;
    let editor = role_id(&db, "ws_editor").await;
    grant_object(&db, "group", g, viewer, "chart", chart_c, "allow").await;
    grant_object(&db, "user", u.0, editor, "chart", chart_c, "deny").await;

    // A SECOND workspace W2 with chart C2 and a grant on it — must NOT leak into W.
    let w2 = wrepo.create(&uniq("ws"), "W2", owner).await.unwrap();
    let database_id2 = seed_database(&db, w2.id).await;
    let (chart_c2, _c2_name) = seed_chart(&db, w2.id, database_id2).await;
    let g2 = seed_group(&db, &uniq("Other Team")).await;
    grant_object(&db, "group", g2, viewer, "chart", chart_c2, "allow").await;

    // Act.
    let rows = repo.list_object_grants(w.id).await.unwrap();

    // Only the two grants on C — nothing from W2.
    assert_eq!(
        rows.len(),
        2,
        "exactly the two object grants on C in this workspace, got {rows:?}"
    );
    assert!(
        rows.iter().all(|r| r.object_id == chart_c),
        "every row must be on chart C"
    );
    // The cross-workspace grant on C2 is EXCLUDED (mandatory IDOR assertion).
    assert!(
        !rows.iter().any(|r| r.object_id == chart_c2),
        "a grant on an object in another workspace must NOT appear"
    );

    // Enrichment: kind, object name, role, effect, principal name.
    for r in &rows {
        assert_eq!(r.kind, "chart", "object kind is chart");
        assert_eq!(
            r.object_name.as_deref(),
            Some(chart_c_name.as_str()),
            "object name enriched"
        );
    }

    let allow = rows
        .iter()
        .find(|r| r.effect == "allow")
        .expect("the allow grant on C");
    assert_eq!(allow.principal_type, "group");
    assert_eq!(allow.principal_id, g);
    assert_eq!(allow.principal_name.as_deref(), Some(group_name.as_str()));
    assert_eq!(allow.role, "ws_viewer");

    let deny = rows
        .iter()
        .find(|r| r.effect == "deny")
        .expect("the deny grant on C");
    assert_eq!(deny.principal_type, "user");
    assert_eq!(deny.principal_id, u.0);
    assert_eq!(deny.principal_name.as_deref(), Some(u_email.as_str()));
    assert_eq!(deny.role, "ws_editor");
}

/// The roll-up surfaces each grant's `expires_at` (P2a Task 2, part B): a grant
/// seeded with an expiry returns it (round-tripped), a grant seeded without one
/// returns `None`. The roll-up is the SharedObjects MANAGEMENT view, so it must
/// keep listing an EXPIRED grant too (no expiry filter here — that's the read-time
/// admission predicate's job, not this view's).
#[tokio::test]
async fn lists_object_grants_with_expires_at() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let repo = AccessViewsRepo::new(db.clone());
    let wrepo = WorkspaceRepo::new(db.clone());

    let owner = seed_user_email(
        &db,
        &format!("own-{}@t.local", uuid::Uuid::new_v4().simple()),
    )
    .await;

    let w = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();
    let database_id = seed_database(&db, w.id).await;
    let (chart_c, _name) = seed_chart(&db, w.id, database_id).await;
    let viewer = role_id(&db, "ws_viewer").await;

    // Grant A: has an expiry (1 hour in the future).
    let g_expiring = seed_group(&db, &uniq("Expiring Team")).await;
    let expiry = chrono::Utc::now() + chrono::Duration::hours(1);
    grant_object_expiring(
        &db,
        "group",
        g_expiring,
        viewer,
        "chart",
        chart_c,
        "allow",
        Some(expiry),
    )
    .await;

    // Grant B: an EXPIRED grant — must still be listed (management roll-up, no
    // expiry filter).
    let g_expired = seed_group(&db, &uniq("Expired Team")).await;
    let past = chrono::Utc::now() - chrono::Duration::hours(1);
    grant_object_expiring(
        &db,
        "group",
        g_expired,
        viewer,
        "chart",
        chart_c,
        "allow",
        Some(past),
    )
    .await;

    // Grant C: no expiry — NULL.
    let g_forever = seed_group(&db, &uniq("Forever Team")).await;
    grant_object(&db, "group", g_forever, viewer, "chart", chart_c, "allow").await;

    let rows = repo.list_object_grants(w.id).await.unwrap();
    assert_eq!(rows.len(), 3, "all three grants listed, expired included");

    let a = rows
        .iter()
        .find(|r| r.principal_id == g_expiring)
        .expect("expiring grant listed");
    let a_expires = a
        .expires_at
        .expect("expiring grant must round-trip expires_at");
    assert!(
        (a_expires - expiry).num_seconds().abs() <= 1,
        "round-tripped expires_at ({a_expires}) must match seeded value ({expiry})"
    );

    let b = rows
        .iter()
        .find(|r| r.principal_id == g_expired)
        .expect("expired grant must still be listed by the management roll-up");
    let b_expires = b
        .expires_at
        .expect("expired grant must round-trip expires_at");
    assert!(
        (b_expires - past).num_seconds().abs() <= 1,
        "round-tripped expires_at ({b_expires}) must match seeded past value ({past})"
    );

    let c = rows
        .iter()
        .find(|r| r.principal_id == g_forever)
        .expect("no-expiry grant listed");
    assert_eq!(
        c.expires_at, None,
        "a grant seeded without expires_at must round-trip to None"
    );
}

/// A share on a data app (kind `app`) is listed like any other kind, with the
/// app's name; a share on an app in another workspace is not.
#[tokio::test]
async fn lists_app_grants_scoped_to_workspace() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let repo = AccessViewsRepo::new(db.clone());
    let wrepo = WorkspaceRepo::new(db.clone());
    let owner = seed_user_email(
        &db,
        &format!("own-{}@t.local", uuid::Uuid::new_v4().simple()),
    )
    .await;
    let w = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();
    let w2 = wrepo.create(&uniq("ws"), "W2", owner).await.unwrap();
    let (app, app_name) = seed_app(&db, w.id).await;
    let (other_app, _) = seed_app(&db, w2.id).await;
    let viewer = role_id(&db, "ws_viewer").await;
    let g = seed_group(&db, &uniq("App Team")).await;
    grant_object(&db, "group", g, viewer, "app", app, "allow").await;
    grant_object(&db, "group", g, viewer, "app", other_app, "allow").await;

    let rows = repo.list_object_grants(w.id).await.unwrap();

    assert_eq!(rows.len(), 1, "only the share on this workspace's app: {rows:?}");
    assert_eq!(rows[0].kind, "app");
    assert_eq!(rows[0].object_id, app);
    assert_eq!(rows[0].object_name.as_deref(), Some(app_name.as_str()));
    assert_eq!(rows[0].role, "ws_viewer");
}
