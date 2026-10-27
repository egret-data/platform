//! `GET /api/control-plane/governance/changes` (DG #3, spec §3.4): governance
//! events newest first, each family only to a reader holding its permission,
//! with labels resolved at read time.
//!
//! The feed is global, so this binary's tests run one at a time ([`SERIAL`])
//! and each starts from an empty `audit.audit_log`; the database is private
//! to this test process (`egret_test_db`). DB-gated; FAILS without a database
//! unless `EGRET_ALLOW_DB_SKIP` is set.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_audit::governance::{action, record_change, Change};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{
    control_plane_router, CpState, ExposureEngine, MaterializedViewLister, MvRow,
    UnwiredCatalogLister,
};
use egret_metastore::Db;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct NoMvLister;
#[async_trait::async_trait]
impl MaterializedViewLister for NoMvLister {
    async fn list(&self, _engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        panic!("the feed never lists materialized views")
    }
}

struct Fixture {
    db: Db,
    users: UserRepo,
    sessions: SessionService,
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..10])
}

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    sqlx::query("TRUNCATE audit.audit_log")
        .execute(&db.pool)
        .await
        .expect("empty the feed");
    Some(Fixture {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        db,
    })
}

async fn state(f: &Fixture, retention: Option<i64>) -> CpState {
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto)),
    );
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    CpState {
        storage: egret_catalog_engine::CatalogStore::new(f.db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(f.db.clone()),
        users: f.users.clone(),
        groups: egret_auth::groups::GroupRepo::new(f.db.clone()),
        roles: egret_auth::roles::RoleRepo::new(f.db.clone(), egret_events::EventBus::new()),
        access_resolver: egret_auth::effective::AccessResolver::new(f.db.clone()),
        secrets,
        sessions: f.sessions.clone(),
        browser: None,
        pdp,
        audit: egret_audit::AuditSink::new(16).0,
        audit_store: egret_audit::AuditStore::new(f.db.clone()),
        tags: egret_bi::TagRepo::new(f.db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            f.db.clone(),
            egret_events::EventBus::new(),
        ),
        db: f.db.clone(),
        mv_lister: Arc::new(NoMvLister),
        mv_list_timeout: std::time::Duration::from_secs(15),
        lineage_sweeper: Arc::new(egret_catalog_engine::NoCatalogSweeper),
        view_catalog: Arc::new(egret_catalog_engine::NoViewCatalog),
        view_audit_timeout: std::time::Duration::from_secs(15),
        catalog_lister: Arc::new(UnwiredCatalogLister),
        browse_timeout: std::time::Duration::from_secs(15),
        audit_retention_days: retention,
        storage_scope: egret_controlplane::StorageScope::s3(
            egret_controlplane::SystemPrefixes::default(),
        ),
    }
}

async fn user(f: &Fixture, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, $2) RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("chg")))
    .bind(name)
    .fetch_one(&f.db.pool)
    .await
    .expect("user")
}

async fn session(f: &Fixture, perms: &[&str]) -> String {
    let uid = user(f, "Reader").await;
    if !perms.is_empty() {
        let role: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id",
        )
        .bind(unique("chg_role"))
        .fetch_one(&f.db.pool)
        .await
        .expect("role");
        for p in perms {
            sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
                .bind(role)
                .bind(p)
                .execute(&f.db.pool)
                .await
                .expect("permission");
        }
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
             VALUES ('user', $1, $2, 'platform')",
        )
        .bind(uid)
        .bind(role)
        .execute(&f.db.pool)
        .await
        .expect("access grant");
    }
    f.sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

fn principal(id: Uuid, impersonator: Option<Uuid>) -> egret_core::Principal {
    egret_core::Principal {
        user_id: egret_core::UserId(id),
        email: "x@egret.local".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: impersonator.map(|i| egret_core::context::Impersonator {
            user_id: egret_core::UserId(i),
            email: "admin@egret.local".into(),
            workspace_scope: None,
            expires_at: chrono::Utc::now(),
        }),
    }
}

async fn record(
    f: &Fixture,
    p: &egret_core::Principal,
    action: &'static str,
    resource_type: &'static str,
    resource_id: &str,
    summary: Value,
) {
    let mut conn = f.db.pool.acquire().await.unwrap();
    record_change(
        &mut conn,
        Change {
            action,
            resource_type,
            resource_id: resource_id.to_string(),
            summary,
            principal: p,
        },
    )
    .await
    .expect("record");
}

/// A legacy event as the best-effort sink writes it.
async fn legacy(
    f: &Fixture,
    actor: Uuid,
    action: &str,
    resource_type: &str,
    id: &str,
    detail: Value,
) {
    sqlx::query(
        "INSERT INTO audit.audit_log (kind, actor_user_id, action, resource_type, resource_id, detail) \
         VALUES ('mutation', $1, $2, $3, $4, $5)",
    )
    .bind(actor)
    .bind(action)
    .bind(resource_type)
    .bind(id)
    .bind(detail)
    .execute(&f.db.pool)
    .await
    .expect("legacy event");
}

async fn changes(st: &CpState, token: &str, query: &str) -> (StatusCode, Value) {
    let req = HttpRequest::builder()
        .uri(format!("/governance/changes{query}"))
        .header("Cookie", format!("egret_session={token}"))
        .body(Body::empty())
        .unwrap();
    let resp = control_plane_router()
        .with_state(st.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn actions(body: &Value) -> Vec<String> {
    let mut a: Vec<String> = body["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| e["action"].as_str().unwrap().to_string())
        .collect();
    a.sort();
    a
}

fn grant_summary() -> Value {
    json!({
        "catalog": "ice", "schema": "s", "table": "t", "column": null,
        "effect": "allow", "actions": ["read"], "include": ["group:g1"], "exclude": []
    })
}

#[tokio::test]
async fn each_family_is_returned_only_to_its_readers() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let ana = user(&f, "Ana").await;
    let p = principal(ana, None);
    record(
        &f,
        &p,
        action::GRANT_CREATE,
        "perm_grant",
        &Uuid::new_v4().to_string(),
        json!({"before": null, "after": grant_summary()}),
    )
    .await;
    record(&f, &p, action::CLEARANCE_SET, "tag", &Uuid::new_v4().to_string(),
        json!({"before": null, "after": {"tag_key": "pii", "required_roles": ["r1"], "required_groups": []}})).await;
    legacy(
        &f,
        ana,
        "tag:assign",
        "catalog",
        &format!("{}:s.t.c", Uuid::new_v4()),
        json!({"tag_id": Uuid::new_v4()}),
    )
    .await;
    legacy(&f, ana, "tag:assign", "object", "dashboard:1", json!({})).await;
    legacy(
        &f,
        ana,
        "role:assign",
        "grant",
        &Uuid::new_v4().to_string(),
        json!({"principal_type": "user", "principal_id": ana, "role_id": Uuid::new_v4()}),
    )
    .await;
    let tag_reader = session(&f, &["tag:read"]).await;
    let policy_reader = session(&f, &["policy:read"]).await;
    let nobody = session(&f, &[]).await;
    let st = state(&f, Some(90)).await;

    let (status, body) = changes(&st, &tag_reader, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        actions(&body),
        ["tag:assign", "tag_clearance:set"],
        "{body}"
    );
    let (status, body) = changes(&st, &policy_reader, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        actions(&body),
        ["perm_grant:create", "role:assign"],
        "{body}"
    );
    let (status, _) = changes(&st, &nobody, "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn actors_targets_and_legacy_events_are_labelled_at_read_time() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let ana = user(&f, "Ana").await;
    let admin = user(&f, "Admin").await;
    let gone = user(&f, "Gone").await;
    // A grant that exists, one that does not.
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'tpch', '{}'::jsonb) RETURNING id",
    )
    .bind(unique("chg_cat"))
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let live: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.grants (effect, actions, catalog_id) VALUES ('allow', '{read}', $1) RETURNING id",
    )
    .bind(cat)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let deleted = Uuid::new_v4();
    record(
        &f,
        &principal(ana, Some(admin)),
        action::GRANT_CREATE,
        "perm_grant",
        &live.to_string(),
        json!({"before": null, "after": grant_summary()}),
    )
    .await;
    record(
        &f,
        &principal(gone, None),
        action::GRANT_DELETE,
        "perm_grant",
        &deleted.to_string(),
        json!({"before": grant_summary(), "after": null}),
    )
    .await;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(gone)
        .execute(&f.db.pool)
        .await
        .unwrap();
    // A role that exists and one that is gone.
    let role_name = unique("analysts");
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role_name)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let gone_role = Uuid::new_v4();
    legacy(
        &f,
        ana,
        "role:assign",
        "grant",
        &role.to_string(),
        json!({"principal_type": "user", "principal_id": ana, "role_id": role}),
    )
    .await;
    legacy(
        &f,
        ana,
        "role:unassign",
        "grant",
        &gone_role.to_string(),
        json!({"principal_type": "user", "principal_id": ana, "role_id": gone_role}),
    )
    .await;
    // A catalog-column tag on an egret catalog.
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'Chg') RETURNING id")
            .bind(unique("chg-ws"))
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AKIA') RETURNING id",
    )
    .bind(unique("chg-sc"))
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("chg_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let ice = unique("chg_ice");
    sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2)",
    )
    .bind(&ice)
    .bind(wh)
    .execute(&f.db.pool)
    .await
    .unwrap();
    let tag: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, 'PII') RETURNING id")
            .bind(unique("pii"))
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    legacy(
        &f,
        ana,
        "tag:assign",
        "catalog",
        &format!("{wh}:sales.orders.email"),
        json!({"tag_id": tag}),
    )
    .await;

    let token = session(&f, &["policy:read", "tag:read"]).await;
    let st = state(&f, Some(90)).await;
    let (status, body) = changes(&st, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let by = |a: &str| -> Vec<&Value> {
        body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["action"] == a)
            .collect()
    };
    let created = by("perm_grant:create")[0];
    assert_eq!(created["actor"], json!({"userId": ana, "label": "Ana"}));
    assert_eq!(
        created["impersonator"],
        json!({"userId": admin, "label": "Admin"})
    );
    assert_eq!(
        created["sentence"],
        "Admin as Ana created a grant: allow read on ice.s.t to group g1"
    );
    assert_eq!(created["target"], json!({"type": "grant", "id": live}));
    assert_eq!(created["targetExists"], true);
    let removed = by("perm_grant:delete")[0];
    assert_eq!(removed["actor"], json!({"userId": gone, "label": null}));
    assert_eq!(
        removed["sentence"],
        "A deleted user deleted a grant: allow read on ice.s.t to group g1"
    );
    assert_eq!(removed["targetExists"], false);
    let assigned = by("role:assign")[0];
    assert_eq!(
        assigned["sentence"],
        format!("Ana gave role {role_name} to Ana")
    );
    assert_eq!(
        assigned["target"],
        json!({"type": "role", "name": role_name})
    );
    let unassigned = by("role:unassign")[0];
    assert_eq!(
        unassigned["sentence"],
        format!("Ana removed role {gone_role} (deleted) from Ana")
    );
    assert_eq!(unassigned["targetExists"], false);
    let tagged = by("tag:assign")[0];
    assert_eq!(
        tagged["sentence"],
        format!("Ana tagged {ice}.sales.orders.email with PII")
    );
    assert_eq!(
        tagged["target"],
        json!({"type": "object", "catalog": ice, "schema": "sales", "object": "orders"})
    );
    assert_eq!(body["retentionDays"], 90);
}

#[tokio::test]
async fn every_action_has_its_sentence() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let ana = user(&f, "Ana").await;
    let p = principal(ana, None);
    let rule = json!({"name": "pii-hash", "kind": "mask", "selector": "ice.s.t.email",
                      "function": "MASK_HASH", "exempt": [], "exempt_exclude": []});
    let filter = json!({"name": "eu-only", "kind": "row_filter", "selector": "tag:region",
                        "function": "region = 'eu'", "exempt": [], "exempt_exclude": []});
    let clearance = json!({"tag_key": "pii", "required_roles": ["r1"], "required_groups": ["g1"]});
    let pol = json!({"service": "trino-prod", "name": "p1", "policy_type": "access",
                     "enabled": true, "resources": {}});
    let id = || Uuid::new_v4().to_string();
    let cases: Vec<(&'static str, &'static str, Value, &str)> = vec![
        (
            action::GRANT_CREATE,
            "perm_grant",
            json!({"before": null, "after": grant_summary()}),
            "Ana created a grant: allow read on ice.s.t to group g1",
        ),
        (
            action::GRANT_UPDATE,
            "perm_grant",
            json!({"before": grant_summary(), "after": grant_summary()}),
            "Ana changed a grant: allow read on ice.s.t to group g1",
        ),
        (
            action::GRANT_DELETE,
            "perm_grant",
            json!({"before": grant_summary(), "after": null}),
            "Ana deleted a grant: allow read on ice.s.t to group g1",
        ),
        (
            action::RULE_CREATE,
            "perm_rule",
            json!({"before": null, "after": rule}),
            "Ana created rule pii-hash: mask MASK_HASH on ice.s.t.email",
        ),
        (
            action::RULE_UPDATE,
            "perm_rule",
            json!({"before": rule, "after": filter}),
            "Ana changed rule eu-only: row filter region = 'eu' on tag:region",
        ),
        (
            action::RULE_DELETE,
            "perm_rule",
            json!({"before": rule, "after": null}),
            "Ana deleted rule pii-hash: mask MASK_HASH on ice.s.t.email",
        ),
        (
            action::CLEARANCE_SET,
            "tag",
            json!({"before": null, "after": clearance}),
            "Ana set the clearance of classification pii: roles r1; groups g1",
        ),
        (
            action::CLEARANCE_CLEAR,
            "tag",
            json!({"before": clearance, "after": null}),
            "Ana removed the clearance of classification pii",
        ),
        (
            action::RANGER_POLICY_CREATE,
            "ranger_policy",
            json!({"before": null, "after": pol}),
            "Ana created hand-written policy p1 on profile trino-prod",
        ),
        (
            action::RANGER_POLICY_UPDATE,
            "ranger_policy",
            json!({"before": pol, "after": pol}),
            "Ana changed hand-written policy p1 on profile trino-prod",
        ),
        (
            action::RANGER_POLICY_DELETE,
            "ranger_policy",
            json!({"before": pol, "after": null}),
            "Ana deleted hand-written policy p1 on profile trino-prod",
        ),
        (
            action::RANGER_POLICY_ENABLE,
            "ranger_policy",
            json!({"before": pol, "after": pol}),
            "Ana enabled hand-written policy p1 on profile trino-prod",
        ),
        (
            action::RANGER_POLICY_DISABLE,
            "ranger_policy",
            json!({"before": pol, "after": pol}),
            "Ana disabled hand-written policy p1 on profile trino-prod",
        ),
        (
            action::MIGRATION_APPLY,
            "policy_profile",
            json!({"service": "trino-prod", "policies": ["p1", "p2"], "created_grants": [], "created_rules": []}),
            "Ana migrated p1, p2 of profile trino-prod into egret grants and rules",
        ),
        (
            action::MIGRATION_UNDO,
            "policy_profile",
            json!({"service": "trino-prod", "policies": ["p1"]}),
            "Ana undid the migration of p1 of profile trino-prod",
        ),
        (
            action::PROFILE_COMPILE,
            "policy_profile",
            json!({"service": "trino-prod", "outcome": "ok", "created": 2, "updated": 1, "deleted": 0, "unchanged": 4, "skipped": 0}),
            "Ana compiled profile trino-prod: 2 created, 1 updated, 0 deleted",
        ),
    ];
    for (a, rt, summary, _) in &cases {
        record(&f, &p, a, rt, &id(), summary.clone()).await;
    }
    record(
        &f,
        &p,
        action::PROFILE_COMPILE,
        "policy_profile",
        "123456789",
        json!({"service": "sr-prod", "outcome": "error", "error": "rule overlap: a and b\nmore"}),
    )
    .await;

    let token = session(&f, &["policy:read", "tag:read"]).await;
    let st = state(&f, Some(90)).await;
    let (status, body) = changes(&st, &token, "?limit=100").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), cases.len() + 1);
    // Newest first: the error compile, then the cases in reverse.
    assert_eq!(
        entries[0]["sentence"],
        "Ana compiled profile sr-prod; the compile failed: rule overlap: a and b"
    );
    for (e, (a, _, _, sentence)) in entries[1..].iter().zip(cases.iter().rev()) {
        assert_eq!(e["action"], *a);
        assert_eq!(e["sentence"], *sentence, "{a}");
    }
}

#[tokio::test]
async fn pages_follow_the_cursor_without_overlap() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let ana = user(&f, "Ana").await;
    let p = principal(ana, None);
    for _ in 0..30 {
        record(
            &f,
            &p,
            action::GRANT_CREATE,
            "perm_grant",
            &Uuid::new_v4().to_string(),
            json!({"before": null, "after": grant_summary()}),
        )
        .await;
    }
    let token = session(&f, &["policy:read"]).await;
    let st = state(&f, Some(90)).await;
    let (status, page1) = changes(&st, &token, "?limit=25").await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_eq!(page1["entries"].as_array().unwrap().len(), 25);
    let cursor = page1["nextCursor"].as_str().expect("a cursor").to_string();
    let (status, page2) = changes(
        &st,
        &token,
        &format!("?limit=25&cursor={}", urlencode(&cursor)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(page2["entries"].as_array().unwrap().len(), 5);
    assert_eq!(page2["nextCursor"], Value::Null);
    let mut ids: Vec<i64> = page1["entries"]
        .as_array()
        .unwrap()
        .iter()
        .chain(page2["entries"].as_array().unwrap())
        .map(|e| e["id"].as_i64().unwrap())
        .collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n, "no duplicate across pages");
    assert_eq!(n, 30);
    let (status, _) = changes(&st, &token, "?cursor=nonsense").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

fn urlencode(s: &str) -> String {
    s.replace('+', "%2B")
        .replace(':', "%3A")
        .replace(',', "%2C")
}

#[tokio::test]
async fn retention_is_reported_as_configured() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let token = session(&f, &["tag:read"]).await;
    let (_, body) = changes(&state(&f, Some(90)).await, &token, "").await;
    assert_eq!(body["retentionDays"], 90, "{body}");
    let (_, body) = changes(&state(&f, None).await, &token, "").await;
    assert_eq!(body["retentionDays"], Value::Null, "{body}");
    assert_eq!(body["entries"], json!([]));
}

/// Review M3: a summary over 16 KiB is stored as `{"truncated": true, …}`;
/// its sentence says what happened and that the details were not kept,
/// never a sentence with empty slots.
#[tokio::test]
async fn a_truncated_event_reads_as_an_honest_generic_sentence() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let ana = user(&f, "Ana").await;
    let p = principal(ana, None);
    let huge = json!({"before": null, "after": {"blob": "x".repeat(20 * 1024)}});
    let gid = Uuid::new_v4().to_string();
    record(
        &f,
        &p,
        action::GRANT_CREATE,
        "perm_grant",
        &gid,
        huge.clone(),
    )
    .await;
    record(
        &f,
        &p,
        action::RANGER_POLICY_UPDATE,
        "ranger_policy",
        "42",
        huge.clone(),
    )
    .await;
    record(
        &f,
        &p,
        action::MIGRATION_APPLY,
        "policy_profile",
        "987654321",
        huge,
    )
    .await;
    let token = session(&f, &["policy:read"]).await;
    let st = state(&f, Some(90)).await;
    let (status, body) = changes(&st, &token, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sentences: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["sentence"].as_str().unwrap())
        .collect();
    assert_eq!(
        sentences,
        [
            "Ana migrated policies of a profile; the change was too large to record its details.",
            "Ana changed a hand-written policy; the change was too large to record its details.",
            "Ana created a grant; the change was too large to record its details.",
        ]
    );
    assert_eq!(body["entries"][2]["detail"]["truncated"], true);
}

/// DG #3 × F11: engine-grant events are Policy-family and read as sentences
/// naming the capability, the engine and the principals.
#[tokio::test]
async fn engine_grant_events_render_in_the_feed() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let ana = user(&f, "Ana").await;
    let p = principal(ana, None);
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec) \
         VALUES ($1, 'trino', 'egret', '{}'::jsonb) RETURNING id",
    )
    .bind(unique("chg-eng"))
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let name: String = sqlx::query_scalar("SELECT name FROM engine_mgmt.engines WHERE id = $1")
        .bind(engine)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let detail = json!({
        "engineId": engine, "capability": "trino.set_system_session_property",
        "values": ["query_max_memory"], "principals": [{"kind": "user", "ref": "ua"}],
        "services": [7]
    });
    let gid = Uuid::new_v4().to_string();
    record(
        &f,
        &p,
        action::ENGINE_GRANT_CREATE,
        "engine_grant",
        &gid,
        json!({"before": null, "after": detail}),
    )
    .await;
    record(
        &f,
        &p,
        action::ENGINE_GRANT_DELETE,
        "engine_grant",
        &gid,
        json!({"before": detail, "after": null}),
    )
    .await;
    let tag_reader = session(&f, &["tag:read"]).await;
    let policy_reader = session(&f, &["policy:read"]).await;
    let st = state(&f, Some(90)).await;
    let (_, body) = changes(&st, &tag_reader, "").await;
    assert_eq!(
        body["entries"],
        json!([]),
        "engine grants are Policy-family"
    );
    let (status, body) = changes(&st, &policy_reader, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let e = &body["entries"];
    assert_eq!(
        e[1]["sentence"],
        format!("Ana created an engine grant: trino.set_system_session_property (query_max_memory) on engine {name} to user ua")
    );
    assert_eq!(
        e[0]["sentence"],
        format!("Ana deleted an engine grant: trino.set_system_session_property (query_max_memory) on engine {name} to user ua")
    );
    assert_eq!(
        e[1]["target"],
        json!({"type": "engine", "id": engine, "name": name})
    );
    assert_eq!(e[1]["targetExists"], false, "the grant row does not exist");
}

// ── user:delete (username-takeover spec §4.4, decision D4) ──────────────

async fn delete_user(st: &CpState, token: &str, id: Uuid) -> StatusCode {
    let req = HttpRequest::builder()
        .method("DELETE")
        .uri(format!("/users/{id}"))
        .header("Cookie", format!("egret_session={token}"))
        .body(Body::empty())
        .unwrap();
    control_plane_router()
        .with_state(st.clone())
        .oneshot(req)
        .await
        .unwrap()
        .status()
}

/// A user with a minted username, the one a later audit must be able to name.
async fn named_user(f: &Fixture, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local', $1, 'Bob', $2) \
         RETURNING id",
    )
    .bind(format!("{username}@egret.local"))
    .bind(username)
    .fetch_one(&f.db.pool)
    .await
    .expect("user")
}

async fn user_delete_events(f: &Fixture, id: Uuid) -> Vec<(String, Option<String>, Value)> {
    sqlx::query_as(
        "SELECT resource_type, resource_id, detail FROM audit.audit_log \
          WHERE kind = 'mutation' AND action = 'user:delete' AND resource_id = $1",
    )
    .bind(id.to_string())
    .fetch_all(&f.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_user_delete_records_who_was_deleted() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let bob_name = unique("bob");
    let bob = named_user(&f, &bob_name).await;
    let admin = session(&f, &["user:delete"]).await;
    let st = state(&f, Some(90)).await;

    assert_eq!(delete_user(&st, &admin, bob).await, StatusCode::NO_CONTENT);

    let events = user_delete_events(&f, bob).await;
    assert_eq!(events.len(), 1, "{events:?}");
    let (resource_type, resource_id, detail) = &events[0];
    assert_eq!(resource_type, "user");
    assert_eq!(resource_id.as_deref(), Some(bob.to_string().as_str()));
    assert_eq!(detail["before"]["username"], json!(bob_name));
    assert_eq!(
        detail["before"]["email"],
        json!(format!("{bob_name}@egret.local"))
    );
    assert_eq!(detail["before"]["name"], json!("Bob"));
    assert_eq!(detail["before"]["provider"], json!("local"));
    assert_eq!(detail["after"], Value::Null);
}

/// The event is written in the delete's own transaction: a delete that is
/// refused at the row DELETE (a content-ownership FK, 23503 → 409) leaves no
/// event naming a user who still exists.
#[tokio::test]
async fn a_refused_user_delete_leaves_no_event() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let bob = named_user(&f, &unique("bob")).await;
    // A table of this test's own, with an FK to users and no ON DELETE rule.
    sqlx::query("CREATE TABLE udel_blocker (owner uuid NOT NULL REFERENCES users(id))")
        .execute(&f.db.pool)
        .await
        .expect("blocker table");
    sqlx::query("INSERT INTO udel_blocker VALUES ($1)")
        .bind(bob)
        .execute(&f.db.pool)
        .await
        .expect("blocker row");
    let admin = session(&f, &["user:delete"]).await;
    let st = state(&f, Some(90)).await;

    let status = delete_user(&st, &admin, bob).await;
    sqlx::query("DROP TABLE udel_blocker")
        .execute(&f.db.pool)
        .await
        .expect("drop blocker");
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(user_delete_events(&f, bob).await.is_empty());
}

/// D4: the event carries the deleted person's email, so it is its own family,
/// read with `user:read`. A `policy:read` reader does not see it.
#[tokio::test]
async fn a_user_delete_event_is_read_with_user_read_only() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let bob_name = unique("bob");
    let bob = named_user(&f, &bob_name).await;
    let admin = session(&f, &["user:delete"]).await;
    let policy_reader = session(&f, &["policy:read"]).await;
    let tag_reader = session(&f, &["tag:read"]).await;
    let user_reader = session(&f, &["policy:read", "user:read"]).await;
    let st = state(&f, Some(90)).await;
    assert_eq!(delete_user(&st, &admin, bob).await, StatusCode::NO_CONTENT);

    for token in [&policy_reader, &tag_reader] {
        let (status, body) = changes(&st, token, "").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            !actions(&body).contains(&"user:delete".to_string()),
            "a reader without user:read sees a deleted user's email: {body}"
        );
    }
    let (status, body) = changes(&st, &user_reader, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entry = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "user:delete")
        .unwrap_or_else(|| panic!("no user:delete entry: {body}"));
    assert!(
        entry["sentence"]
            .as_str()
            .unwrap()
            .ends_with(&format!(" deleted user {bob_name}")),
        "{entry}"
    );
}

/// Rename spec §5.7: a classification move or purge reads with the catalog
/// client that made it, to a `tag:read` reader only. A client deleted since
/// is named by its `client_id`.
#[tokio::test]
async fn classification_moves_read_with_their_catalog_client() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let tag = |name: &str| json!({"id": Uuid::new_v4(), "name": name});
    let client = |id: Option<Uuid>, name: Option<&str>| {
        json!({"kind": "catalog_client", "id": id, "client_id": "trino-main-7f3a",
               "name": name, "consumer_type": "engine"})
    };
    let at = |table: &str, column: Option<&str>| json!({"schema": "sales", "table": table, "column": column});
    let rid = format!("{}:sales.orders", Uuid::new_v4());
    let mut conn = f.db.pool.acquire().await.unwrap();
    let events = [
        (
            action::CATALOG_TAG_MOVE,
            json!({"before": at("orders", None), "after": at("orders_v2", None),
                   "tags": [tag("PII")], "rows": 2, "kept": [], "trigger": "iceberg_rename",
                   "actor": client(Some(Uuid::new_v4()), Some("trino-main"))}),
        ),
        (
            action::CATALOG_TAG_MOVE,
            json!({"before": at("orders", Some("email")), "after": at("orders", Some("mail")),
                   "tags": [tag("PII"), tag("Finance")], "rows": 2, "kept": [],
                   "trigger": "iceberg_column_rename", "actor": client(None, None)}),
        ),
        (
            action::CATALOG_TAG_PURGE,
            json!({"schema": "sales", "rows": 12, "views": 1,
                   "trigger": "iceberg_drop_namespace",
                   "actor": client(Some(Uuid::new_v4()), Some("spark-etl"))}),
        ),
    ];
    for (a, detail) in events {
        sqlx::query(
            "INSERT INTO audit.audit_log (kind, action, resource_type, resource_id, detail) \
             VALUES ('mutation', $1, 'catalog', $2, $3)",
        )
        .bind(a)
        .bind(&rid)
        .bind(detail)
        .execute(&mut *conn)
        .await
        .expect("event");
    }

    // Sessions first: the PDP reads role grants when it is built.
    let tag_reader = session(&f, &["tag:read"]).await;
    let policy_reader = session(&f, &["policy:read"]).await;
    let st = state(&f, None).await;
    let (status, body) = changes(&st, &tag_reader, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sentences: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["sentence"].as_str().unwrap())
        .collect();
    assert_eq!(
        sentences,
        vec![
            "Catalog client spark-etl removed 12 classification assignments with namespace \
             sales (namespace dropped)",
            "Catalog client trino-main-7f3a (deleted) moved classifications PII and Finance \
             from sales.orders.email to sales.orders.mail (column renamed)",
            "Catalog client trino-main moved classification PII from sales.orders to \
             sales.orders_v2 (table renamed)",
        ]
    );

    // Live check F6: the Who column names the catalog client too, not
    // "egret" (`actor` stays null: a client is no user).
    let clients: Vec<&Value> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| &e["actorClient"])
        .collect();
    assert_eq!(
        clients,
        vec![
            &json!({"clientId": "trino-main-7f3a", "label": "spark-etl"}),
            &json!({"clientId": "trino-main-7f3a", "label": null}),
            &json!({"clientId": "trino-main-7f3a", "label": "trino-main"}),
        ]
    );
    assert!(body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["actor"].is_null()));

    let (status, body) = changes(&st, &policy_reader, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(actions(&body).is_empty(), "family Tag: {body}");
}

/// Grants-follow spec §5.7: an allow moved and the restrictions copied by a
/// rename read with the catalog client that made it, to a `policy:read`
/// reader only.
#[tokio::test]
async fn grant_moves_and_copies_read_with_their_catalog_client() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let actor = json!({"kind": "catalog_client", "id": Uuid::new_v4(), "client_id": "trino-main-7f3a",
                       "name": "trino-main", "consumer_type": "engine"});
    let grant = |effect: &str, actions: &[&str], table: &str| {
        json!({"id": Uuid::new_v4(), "catalog": "lake", "schema": "sales", "table": table,
               "column": null, "effect": effect, "actions": actions,
               "include": ["group:contractors"], "exclude": []})
    };
    let rule = |name: &str, table: &str| {
        json!({"id": Uuid::new_v4(), "name": name, "kind": "mask",
               "selector": format!("lake.sales.{table}.email"), "function": "MASK_HASH",
               "exempt": [], "exempt_exclude": []})
    };
    let rename = json!({"from": "sales.orders", "to": "sales.orders_v2"});
    let events = [
        (
            action::GRANT_MOVE,
            "perm_grant",
            json!({"before": grant("allow", &["read"], "orders"),
                   "after": grant("allow", &["read"], "orders_v2"),
                   "rename": rename, "trigger": "iceberg_rename", "actor": actor}),
        ),
        (
            action::GRANT_COPY,
            "perm_grant",
            json!({"before": grant("deny", &["read", "write"], "orders"),
                   "after": grant("deny", &["read", "write"], "orders_v2"),
                   "rename": rename, "trigger": "iceberg_rename", "actor": actor}),
        ),
        (
            action::RULE_COPY,
            "perm_rule",
            json!({"before": rule("pii-email", "orders"),
                   "after": rule("pii-email (sales.orders_v2)", "orders_v2"),
                   "rename": rename, "trigger": "iceberg_rename", "actor": actor}),
        ),
    ];
    let mut conn = f.db.pool.acquire().await.unwrap();
    for (a, rt, detail) in events {
        sqlx::query(
            "INSERT INTO audit.audit_log (kind, action, resource_type, resource_id, detail) \
             VALUES ('mutation', $1, $2, $3, $4)",
        )
        .bind(a)
        .bind(rt)
        .bind(Uuid::new_v4().to_string())
        .bind(detail)
        .execute(&mut *conn)
        .await
        .expect("event");
    }

    let policy_reader = session(&f, &["policy:read"]).await;
    let tag_reader = session(&f, &["tag:read"]).await;
    let st = state(&f, None).await;
    let (status, body) = changes(&st, &policy_reader, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sentences: Vec<&str> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["sentence"].as_str().unwrap())
        .collect();
    assert_eq!(
        sentences,
        vec![
            "Catalog client trino-main copied mask rule pii-email from sales.orders.email to \
             sales.orders_v2.email as pii-email (sales.orders_v2); the original stays on \
             sales.orders.email (table renamed)",
            "Catalog client trino-main copied deny grant (read, write) from sales.orders to \
             sales.orders_v2; the original stays on sales.orders (table renamed)",
            "Catalog client trino-main moved allow grant (read) from sales.orders to \
             sales.orders_v2 (table renamed)",
        ]
    );
    let (status, body) = changes(&st, &tag_reader, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(actions(&body).is_empty(), "family Policy: {body}");
}

/// Grants-follow spec §5.7: an allow pattern a rename left behind.
#[tokio::test]
async fn a_left_allow_reads_with_its_catalog_client() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let actor = json!({"kind": "catalog_client", "id": Uuid::new_v4(), "client_id": "trino-main-7f3a",
                       "name": "trino-main", "consumer_type": "engine"});
    let detail = json!({
        "before": {"id": Uuid::new_v4(), "catalog": "lake", "schema": "sales", "table": "tmp_*",
                   "column": null, "effect": "allow", "actions": ["read"],
                   "include": ["group:analysts"], "exclude": []},
        "after": null, "rename": {"from": "sales.tmp_a", "to": "sales.a"},
        "trigger": "iceberg_rename", "actor": actor,
    });
    sqlx::query(
        "INSERT INTO audit.audit_log (kind, action, resource_type, resource_id, detail) \
         VALUES ('mutation', $1, 'perm_grant', $2, $3)",
    )
    .bind(action::GRANT_LEFT)
    .bind(Uuid::new_v4().to_string())
    .bind(detail)
    .execute(&f.db.pool)
    .await
    .expect("event");
    let policy_reader = session(&f, &["policy:read"]).await;
    let st = state(&f, None).await;
    let (status, body) = changes(&st, &policy_reader, "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["entries"][0]["sentence"],
        "Catalog client trino-main renamed sales.tmp_a to sales.a; allow grant on sales.tmp_* \
         no longer covers it"
    );
}
