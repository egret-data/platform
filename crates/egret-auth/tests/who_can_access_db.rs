//! DB-gated integration test for the reverse resolver (`who_can_access`):
//! RESOURCE→principals — "who can access this object, and why". Mirrors the sibling
//! `effective_db.rs` harness (same `connect` + `DATABASE_URL` gate, which FAILS without a
//! database and self-skips only under `EGRET_ALLOW_DB_SKIP`; CI / `just up` provides
//! Postgres).
//!
//! Unlike `effective_db.rs` (which uses bare-uuid object targets), this needs a
//! REAL `analytics.charts` row so the object can be resolved to a name + workspace
//! — seeded via the workspace→database→dataset→chart chain, mirroring
//! `egret-workspace/tests/shares_rollup_db.rs`.

use egret_auth::effective::AccessResolver;
use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", Uuid::new_v4().simple())
}

async fn seed_user(db: &Db, email: &str) -> UserId {
    UserRepo::new(db.clone())
        .insert_local(email, "U", &hash_password("pw").unwrap())
        .await
        .unwrap()
}

async fn seed_group(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

async fn seed_workspace(db: &Db, slug: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
        .bind(slug)
        .bind(format!("WS {slug}"))
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// A registered engine (workspace-scoped).
async fn seed_database(db: &Db, ws: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO query.databases (workspace_id, name, engine, host, port) \
         VALUES ($1, $2, 'trino', 'localhost', 8080) RETURNING id",
    )
    .bind(ws)
    .bind(uniq("engine"))
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// A chart (via a table-backed dataset) in the given workspace; returns (chart_id, name).
async fn seed_chart(db: &Db, ws: Uuid, database_id: Uuid) -> (Uuid, String) {
    let dataset_id: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.datasets (workspace_id, database_id, name, table_name) \
         VALUES ($1, $2, $3, 't') RETURNING id",
    )
    .bind(ws)
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
    .bind(ws)
    .bind(dataset_id)
    .bind(&name)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    (chart_id, name)
}

async fn ws_role_id(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn grant(
    db: &Db,
    ptype: &str,
    pid: Uuid,
    role_id: Uuid,
    scope_type: &str,
    scope_id: Option<Uuid>,
    object_type: Option<&str>,
    effect: &str,
) {
    sqlx::query(
        "INSERT INTO access_grants \
         (principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(ptype)
    .bind(pid)
    .bind(role_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(object_type)
    .bind(effect)
    .execute(&db.pool)
    .await
    .unwrap();
}

// Reverse resolution: who can access chart C, and why.
// - Group G granted ws_viewer DIRECTLY on C            → source "direct".
// - User U2 granted ws_editor on the WORKSPACE          → source "workspace:{slug}".
// - User U3 is a platform_admin                         → source "platform".
// A SECOND workspace W2 + chart C2 with its own grant MUST NOT leak in.
#[tokio::test]
async fn who_can_access_chart_reverse_resolution() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };

    let slug = uniq("wca-test");
    let w = seed_workspace(&db, &slug).await;
    let db_id = seed_database(&db, w).await;
    let (chart_c, chart_name) = seed_chart(&db, w, db_id).await;

    let viewer = ws_role_id(&db, "ws_viewer").await;
    let editor = ws_role_id(&db, "ws_editor").await;
    let admin = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM roles WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // G: ws_viewer DIRECTLY on chart C.
    let g_name = uniq("Analytics Team");
    let g = seed_group(&db, &g_name).await;
    grant(
        &db,
        "group",
        g,
        viewer,
        "object",
        Some(chart_c),
        Some("chart"),
        "allow",
    )
    .await;

    // U2: ws_editor on the WORKSPACE (cascades to C).
    let u2_email = format!("u2-{}@t.local", Uuid::new_v4().simple());
    let u2 = seed_user(&db, &u2_email).await;
    grant(
        &db,
        "user",
        u2.0,
        editor,
        "workspace",
        Some(w),
        None,
        "allow",
    )
    .await;

    // U3: platform_admin (superuser — accesses everything).
    let u3_email = format!("u3-{}@t.local", Uuid::new_v4().simple());
    let u3 = seed_user(&db, &u3_email).await;
    grant(&db, "user", u3.0, admin, "platform", None, None, "allow").await;

    // A SECOND workspace W2 + chart C2 with its own object grant — must NOT leak.
    let slug2 = uniq("wca-other");
    let w2 = seed_workspace(&db, &slug2).await;
    let db_id2 = seed_database(&db, w2).await;
    let (chart_c2, _c2_name) = seed_chart(&db, w2, db_id2).await;
    let g2_name = uniq("Other Team");
    let g2 = seed_group(&db, &g2_name).await;
    grant(
        &db,
        "group",
        g2,
        viewer,
        "object",
        Some(chart_c2),
        Some("chart"),
        "allow",
    )
    .await;
    // U-only on W2 workspace — must NOT appear for C either.
    let uw2_email = format!("uw2-{}@t.local", Uuid::new_v4().simple());
    let uw2 = seed_user(&db, &uw2_email).await;
    grant(
        &db,
        "user",
        uw2.0,
        editor,
        "workspace",
        Some(w2),
        None,
        "allow",
    )
    .await;

    // Act.
    let out = AccessResolver::new(db.clone())
        .who_can_access("chart", chart_c)
        .await
        .expect("who_can_access resolves chart C");

    // Object header.
    assert_eq!(out.object.kind, "chart");
    assert_eq!(out.object.id, chart_c.to_string());
    assert_eq!(out.object.name.as_deref(), Some(chart_name.as_str()));
    assert_eq!(out.object.workspace_slug.as_deref(), Some(slug.as_str()));

    // G — direct object grant.
    let ge = out
        .entries
        .iter()
        .find(|e| e.principal_id == g.to_string())
        .expect("group G present");
    assert_eq!(ge.principal_type, "group");
    assert_eq!(ge.principal_name.as_deref(), Some(g_name.as_str()));
    assert_eq!(ge.access, "ws_viewer");
    assert_eq!(ge.effect, "allow");
    assert_eq!(ge.source, "direct");

    // U2 — workspace-role cascade.
    let u2e = out
        .entries
        .iter()
        .find(|e| e.principal_id == u2.0.to_string())
        .expect("user U2 present");
    assert_eq!(u2e.principal_type, "user");
    assert_eq!(u2e.principal_name.as_deref(), Some(u2_email.as_str()));
    assert_eq!(u2e.access, "ws_editor");
    assert_eq!(u2e.effect, "allow");
    assert_eq!(u2e.source, format!("workspace:{slug}"));

    // U3 — platform admin.
    let u3e = out
        .entries
        .iter()
        .find(|e| e.principal_id == u3.0.to_string())
        .expect("platform admin U3 present");
    assert_eq!(u3e.principal_type, "user");
    assert_eq!(u3e.access, "platform_admin");
    assert_eq!(u3e.effect, "allow");
    assert_eq!(u3e.source, "platform");

    // Cross-workspace / cross-object exclusion — MANDATORY.
    assert!(
        !out.entries.iter().any(|e| e.principal_id == g2.to_string()),
        "a grant on chart C2 (another object/workspace) must NOT appear: {:?}",
        out.entries
    );
    assert!(
        !out.entries
            .iter()
            .any(|e| e.principal_id == uw2.0.to_string()),
        "a workspace grant on W2 must NOT appear for C: {:?}",
        out.entries
    );

    // Entries are ordered by (source, principal_name).
    let ordering: Vec<(&str, &str)> = out
        .entries
        .iter()
        .map(|e| (e.source.as_str(), e.principal_name.as_deref().unwrap_or("")))
        .collect();
    let mut sorted = ordering.clone();
    sorted.sort();
    assert_eq!(ordering, sorted, "entries ordered by (source, name)");
}

// Not-found object id → Err (maps to 404). Unknown kind → Err.
#[tokio::test]
async fn who_can_access_error_cases() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let resolver = AccessResolver::new(db.clone());

    // Random uuid, real kind → NotFound.
    let missing = resolver.who_can_access("chart", Uuid::new_v4()).await;
    assert!(
        matches!(missing, Err(egret_core::Error::NotFound(_))),
        "random chart id is NotFound, got {missing:?}"
    );

    // Unknown kind → Err (NotFound).
    let bogus = resolver.who_can_access("bogus_kind", Uuid::new_v4()).await;
    assert!(
        matches!(bogus, Err(egret_core::Error::NotFound(_))),
        "unknown kind is an error, got {bogus:?}"
    );
}
