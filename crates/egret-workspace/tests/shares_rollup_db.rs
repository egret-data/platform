//! Workspace object-grant roll-up view (read-only). Asserts
//! `AccessViewsRepo::list_object_grants` returns every OBJECT grant whose object
//! lives in THIS workspace — enriched with object name, role name, principal name
//! and effect — and, critically, EXCLUDES grants on objects that live in another
//! workspace (the IDOR guard). DB-gated; self-skips without DATABASE_URL —
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
    let url = std::env::var("DATABASE_URL").ok()?;
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
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect) \
         VALUES ($1, $2, $3, 'object', $4, $5, $6)",
    )
    .bind(ptype)
    .bind(pid)
    .bind(role)
    .bind(object_id)
    .bind(object_type)
    .bind(effect)
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
