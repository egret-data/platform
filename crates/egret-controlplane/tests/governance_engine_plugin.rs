//! `GET /api/control-plane/governance/engines/{id}/plugin` (DG #4a, spec
//! §4.2): an engine's plugin download state, judged by the same function as
//! the Governance Overview's drift findings, and an agreement test that the
//! two say the same thing for the same profile.
//!
//! The Overview's findings are global, so this binary's tests run one at a
//! time ([`SERIAL`]) and each starts by emptying the tables the sources read.
//! The database is private to this test process (`egret_test_db`).
//!
//! DB-gated; FAILS without a database.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{
    control_plane_router, CpState, ExposureEngine, MaterializedViewLister, MvRow,
    UnwiredCatalogLister,
};
use egret_metastore::Db;
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct NoMvLister;
#[async_trait::async_trait]
impl MaterializedViewLister for NoMvLister {
    async fn list(&self, _engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        panic!("never lists materialized views")
    }
}

struct Fixture {
    db: Db,
    users: UserRepo,
    sessions: SessionService,
    policy: PolicyStore,
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..10])
}

async fn setup() -> Fixture {
    let dsn = egret_test_db::url()
        .await
        .expect("a database is required (DATABASE_URL)");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    sqlx::query(
        "TRUNCATE engine_mgmt.engines, policy.ranger_plugin_downloads, perm.compile_runs, \
                  view_lineage_state, tags, registry.engine_catalog_attachments, \
                  registry.catalogs, perm.grants, perm.rules, policy.authz_services CASCADE",
    )
    .execute(&db.pool)
    .await
    .expect("empty the sources' tables");
    Fixture {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        policy,
        db,
    }
}

/// The router with a REAL `CedarPdp`, built after every role exists.
async fn state(f: &Fixture) -> CpState {
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
        audit_retention_days: Some(90),
    }
}

/// A session holding exactly `perms` (`["platform_admin"]` for an admin).
async fn session(f: &Fixture, perms: &[&str]) -> String {
    let uid = f
        .users
        .insert_local(&format!("{}@egret.local", unique("plg")), "T", "x")
        .await
        .expect("user");
    if perms == ["platform_admin"] {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
             SELECT 'user', $1, id, 'platform' FROM roles \
              WHERE scope = 'platform' AND name = 'platform_admin'",
        )
        .bind(uid.0)
        .execute(&f.db.pool)
        .await
        .expect("platform_admin");
    } else if !perms.is_empty() {
        let role: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id",
        )
        .bind(unique("plg_role"))
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
        .bind(uid.0)
        .bind(role)
        .execute(&f.db.pool)
        .await
        .expect("access grant");
    }
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

async fn get(st: &CpState, token: &str, uri: &str) -> (StatusCode, Value) {
    let req = HttpRequest::builder()
        .uri(uri)
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

async fn plugin(st: &CpState, token: &str, engine: Uuid) -> (StatusCode, Value) {
    get(st, token, &format!("/governance/engines/{engine}/plugin")).await
}

// ── fixtures ─────────────────────────────────────────────────────────────

async fn profile(f: &Fixture, name: &str, ty: &str) -> i64 {
    f.policy
        .create_service(name, ty, &json!({}))
        .await
        .expect("profile")
        .id
}

async fn engine(
    f: &Fixture,
    name: &str,
    ty: &str,
    origin: &str,
    status: &str,
    sid: Option<i64>,
) -> Uuid {
    let sql = if origin == "external" {
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, authz_service_id, \
                                          origin, external_host, external_port, ranger_plugin_version) \
         VALUES ($1, $2, '', '{}'::jsonb, $3, $4, 'external', 'h.example', 443, '2.8.0') RETURNING id"
    } else {
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, authz_service_id) \
         VALUES ($1, $2, 'egret', '{\"imageVersion\":\"483\"}'::jsonb, $3, $4) RETURNING id"
    };
    sqlx::query_scalar(sql)
        .bind(name)
        .bind(ty)
        .bind(status)
        .bind(sid)
        .fetch_one(&f.db.pool)
        .await
        .expect("engine")
}

async fn counters(f: &Fixture, sid: i64, policy_version: i64, tag_version: i64, ago_secs: i64) {
    sqlx::query(
        "UPDATE policy.authz_services SET policy_version = $2, tag_version = $3, \
                updated_at = now() - make_interval(secs => $4) WHERE id = $1",
    )
    .bind(sid)
    .bind(policy_version)
    .bind(tag_version)
    .bind(ago_secs as f64)
    .execute(&f.db.pool)
    .await
    .expect("counters");
}

async fn poll(f: &Fixture, sid: i64, plugin: &str, kind: &str, held: i64, ago_secs: i64) {
    sqlx::query(
        "INSERT INTO policy.ranger_plugin_downloads \
             (service_id, plugin_id, kind, last_known_version, served_version, at) \
         VALUES ($1, $2, $3, $4, $4, now() - make_interval(secs => $5))",
    )
    .bind(sid)
    .bind(plugin)
    .bind(kind)
    .bind(held)
    .bind(ago_secs as f64)
    .execute(&f.db.pool)
    .await
    .expect("poll");
}

/// A provisioned running engine of `ty` bound to a new profile.
async fn live(f: &Fixture, name: &str, ty: &str) -> (i64, Uuid) {
    let sid = profile(f, name, ty).await;
    let eid = engine(
        f,
        &format!("{name}-eng"),
        ty,
        "provisioned",
        "running",
        Some(sid),
    )
    .await;
    (sid, eid)
}

fn kind<'a>(body: &'a Value, k: &str) -> &'a Value {
    body["kinds"]
        .as_array()
        .expect("kinds")
        .iter()
        .find(|x| x["kind"] == k)
        .unwrap_or_else(|| panic!("kind {k} missing: {body}"))
}

// ── tests ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_confirmed_trino_profile_is_up_to_date_and_lists_its_rows_newest_first() {
    let _serial = SERIAL.lock().await;
    let f = setup().await;
    let (sid, eid) = live(&f, "j_ok", "trino").await;
    counters(&f, sid, 5, 2, 600).await;
    poll(&f, sid, "trino@a-j_ok", "policies", 5, 10).await;
    poll(&f, sid, "trino@a-j_ok", "tags", 2, 20).await;
    let st = state(&f).await;
    let token = session(&f, &["platform_admin"]).await;
    let (status, body) = plugin(&st, &token, eid).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["judged"]["state"], "judged", "{body}");
    assert_eq!(body["engine"]["id"], json!(eid));
    assert_eq!(body["engine"]["type"], "trino");
    assert_eq!(body["engine"]["origin"], "provisioned");
    assert_eq!(body["profile"]["name"], "j_ok");
    assert_eq!(body["profile"]["policyVersion"], 5);
    assert_eq!(body["sharedWith"], json!([]));
    let pol = kind(&body, "policies");
    assert_eq!(pol["state"], "up_to_date", "{body}");
    assert_eq!(pol["currentVersion"], 5);
    assert_eq!(pol["newestReportedVersion"], 5);
    assert_eq!(pol["notReported"], Value::Null);
    assert_eq!(
        pol["sentence"],
        "Plugins of profile j_ok report holding policy version 5."
    );
    assert_eq!(kind(&body, "tags")["state"], "up_to_date", "{body}");
    let rows = body["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 2, "{body}");
    assert_eq!(rows[0]["pluginId"], "trino@a-j_ok");
    assert_eq!(rows[0]["kind"], "policies", "newest first: {body}");
    assert_eq!(rows[1]["kind"], "tags");
}

#[tokio::test]
async fn an_old_version_two_minutes_after_the_bump_is_behind_with_the_overviews_sentence() {
    let _serial = SERIAL.lock().await;
    let f = setup().await;
    let (sid, eid) = live(&f, "j_behind", "trino").await;
    counters(&f, sid, 5, 2, 120).await;
    poll(&f, sid, "trino@a-j_behind", "policies", 4, 10).await;
    poll(&f, sid, "trino@a-j_behind", "tags", 2, 10).await;
    let st = state(&f).await;
    let token = session(&f, &["platform_admin"]).await;
    let (_, body) = plugin(&st, &token, eid).await;
    let pol = kind(&body, "policies");
    assert_eq!(pol["state"], "behind", "{body}");
    let s = pol["sentence"].as_str().unwrap();
    assert!(
        s.starts_with(
            "No plugin of profile j_behind has reported holding policy version 5, set at "
        ),
        "{s}"
    );
    // The Overview says exactly the same sentence for this profile.
    let (_, att) = get(&st, &token, "/governance/attention").await;
    let fnd = att["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["kind"] == "plugin_behind")
        .unwrap_or_else(|| panic!("no plugin_behind: {att}"));
    assert_eq!(fnd["sentence"], pol["sentence"], "{att}");
}

#[tokio::test]
async fn a_starrocks_profile_with_no_tags_rows_reports_no_download() {
    let _serial = SERIAL.lock().await;
    let f = setup().await;
    let (sid, eid) = live(&f, "j_sr", "starrocks").await;
    counters(&f, sid, 5, 1, 600).await;
    poll(&f, sid, "starrocks@a-j_sr", "policies", 5, 10).await;
    let st = state(&f).await;
    let token = session(&f, &["platform_admin"]).await;
    let (_, body) = plugin(&st, &token, eid).await;
    let tags = kind(&body, "tags");
    assert_eq!(tags["state"], "not_reported", "{body}");
    assert_eq!(tags["notReported"], "no_download", "{body}");
    assert_eq!(
        tags["sentence"],
        "No plugin of profile j_sr downloaded tags in 24 hours. StarRocks never downloads tags."
    );
    assert_eq!(kind(&body, "policies")["state"], "up_to_date");
}

#[tokio::test]
async fn external_not_running_no_profile_and_unknown_engines() {
    let _serial = SERIAL.lock().await;
    let f = setup().await;
    // External, bound, with rows: not judged, rows present.
    let sid = profile(&f, "j_ext", "trino").await;
    let ext = engine(&f, "j_ext-eng", "trino", "external", "running", Some(sid)).await;
    counters(&f, sid, 5, 1, 600).await;
    poll(&f, sid, "trino@x-j_ext", "policies", 5, 10).await;
    // Provisioned but failed.
    let sid2 = profile(&f, "j_failed", "trino").await;
    let failed = engine(
        &f,
        "j_failed-eng",
        "trino",
        "provisioned",
        "failed",
        Some(sid2),
    )
    .await;
    // No profile.
    let lone = engine(&f, "j_lone", "trino", "provisioned", "running", None).await;
    let st = state(&f).await;
    let token = session(&f, &["platform_admin"]).await;

    let (status, body) = plugin(&st, &token, ext).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["judged"]["state"], "not_judged", "{body}");
    assert_eq!(body["judged"]["reason"], "external", "{body}");
    assert_eq!(
        body["judged"]["sentence"],
        "egret does not judge external engines: it does not run them and cannot tell whether \
         they should be polling."
    );
    assert_eq!(body["kinds"], json!([]), "{body}");
    assert_eq!(body["rows"].as_array().unwrap().len(), 1, "{body}");

    let (_, body) = plugin(&st, &token, failed).await;
    assert_eq!(body["judged"]["reason"], "not_running", "{body}");
    assert_eq!(body["kinds"], json!([]));

    let (_, body) = plugin(&st, &token, lone).await;
    assert_eq!(body["judged"]["reason"], "no_profile", "{body}");
    assert_eq!(body["profile"], Value::Null);
    assert_eq!(body["rows"], json!([]));

    let (status, _) = plugin(&st, &token, Uuid::new_v4()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_plugin_report_is_egret_perms_for_the_engine_row() {
    let _serial = SERIAL.lock().await;
    let f = setup().await;
    let (_, prov) = live(&f, "j_rep", "trino").await;
    let sid = profile(&f, "j_rep_ext", "starrocks").await;
    let ext = engine(
        &f,
        "j_rep_ext-eng",
        "starrocks",
        "external",
        "running",
        Some(sid),
    )
    .await;
    let st = state(&f).await;
    let token = session(&f, &["platform_admin"]).await;
    let (_, body) = plugin(&st, &token, prov).await;
    let want = egret_perm::ranger_plugin_report(
        "trino",
        "provisioned",
        None,
        &json!({"imageVersion": "483"}),
    );
    assert_eq!(body["plugin"]["version"], json!(want.version), "{body}");
    assert_eq!(body["plugin"]["modelled"], json!(want.modelled));
    assert_eq!(body["plugin"]["note"], json!(want.note));
    let (_, body) = plugin(&st, &token, ext).await;
    let want = egret_perm::ranger_plugin_report("starrocks", "external", Some("2.8.0"), &json!({}));
    assert_eq!(body["plugin"]["version"], json!(want.version), "{body}");
    assert_eq!(body["plugin"]["modelled"], json!(want.modelled));
    assert_eq!(body["plugin"]["note"], json!(want.note));
}

#[tokio::test]
async fn a_legacy_shared_profile_names_the_other_engines() {
    let _serial = SERIAL.lock().await;
    let f = setup().await;
    let (sid, eid) = live(&f, "j_shared", "trino").await;
    // A second engine bound by SQL (I2 refuses new shared binds).
    let other = engine(
        &f,
        "j_shared-b",
        "trino",
        "provisioned",
        "running",
        Some(sid),
    )
    .await;
    let st = state(&f).await;
    let token = session(&f, &["platform_admin"]).await;
    let (_, body) = plugin(&st, &token, eid).await;
    assert_eq!(
        body["sharedWith"],
        json!([{ "id": other, "name": "j_shared-b", "origin": "provisioned" }]),
        "{body}"
    );
}

#[tokio::test]
async fn it_needs_both_policy_read_and_engine_read() {
    let _serial = SERIAL.lock().await;
    let f = setup().await;
    let (_, eid) = live(&f, "j_perm", "trino").await;
    let policy_only = session(&f, &["policy:read"]).await;
    let engine_only = session(&f, &["engine:read"]).await;
    let both = session(&f, &["policy:read", "engine:read"]).await;
    let st = state(&f).await;
    assert_eq!(
        plugin(&st, &policy_only, eid).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        plugin(&st, &engine_only, eid).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(plugin(&st, &both, eid).await.0, StatusCode::OK);
}

/// The agreement test (spec §9): for each drift fixture and a clean one, the
/// Overview raises `plugin_silent` / `plugin_quiet` / `plugin_behind` for the
/// profile exactly when this route reports `silent` / `quiet` / `behind` for
/// either kind.
#[tokio::test]
async fn the_overview_and_the_plugin_route_agree_on_every_drift_kind() {
    let _serial = SERIAL.lock().await;
    let f = setup().await;
    let mut engines = Vec::new();

    let (s, e) = live(&f, "a_silent", "starrocks").await;
    counters(&f, s, 5, 1, 30 * 3600).await;
    poll(&f, s, "starrocks@a_silent", "policies", 5, 25 * 3600).await;
    engines.push(("a_silent", e));

    let (s, e) = live(&f, "a_quiet", "starrocks").await;
    counters(&f, s, 5, 1, 3600).await;
    poll(&f, s, "starrocks@a_quiet", "policies", 5, 600).await;
    engines.push(("a_quiet", e));

    let (s, e) = live(&f, "a_tags_quiet", "trino").await;
    counters(&f, s, 5, 1, 3600).await;
    poll(&f, s, "trino@a_tags_quiet", "policies", 5, 10).await;
    poll(&f, s, "trino@a_tags_quiet", "tags", 1, 600).await;
    engines.push(("a_tags_quiet", e));

    let (s, e) = live(&f, "a_behind", "trino").await;
    counters(&f, s, 5, 3, 120).await;
    poll(&f, s, "trino@a_behind", "policies", 5, 10).await;
    poll(&f, s, "trino@a_behind", "tags", 2, 20).await;
    engines.push(("a_behind", e));

    let (s, e) = live(&f, "a_clean", "trino").await;
    counters(&f, s, 5, 1, 600).await;
    poll(&f, s, "trino@a_clean", "policies", 5, 10).await;
    poll(&f, s, "trino@a_clean", "tags", 1, 10).await;
    engines.push(("a_clean", e));

    let st = state(&f).await;
    let token = session(&f, &["platform_admin"]).await;
    let (status, att) = get(&st, &token, "/governance/attention").await;
    assert_eq!(status, StatusCode::OK, "{att}");
    let raised = |finding: &str, profile: &str| -> bool {
        att["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| x["kind"] == finding)
            .any(|x| {
                x["ref"] == json!({"type": "profile", "name": profile})
                    || x["sample"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|s| s["label"] == profile)
            })
    };
    let mut seen = std::collections::BTreeSet::new();
    for (name, eid) in engines {
        let (status, body) = plugin(&st, &token, eid).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let states: Vec<String> = body["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["state"].as_str().unwrap().to_string())
            .collect();
        for (finding, state) in [
            ("plugin_silent", "silent"),
            ("plugin_quiet", "quiet"),
            ("plugin_behind", "behind"),
        ] {
            let route = states.iter().any(|s| s == state);
            assert_eq!(
                raised(finding, name),
                route,
                "{name}: Overview {finding} vs route {state}; route={body}; overview={att}"
            );
            if route {
                seen.insert(format!("{name}:{state}"));
            }
        }
    }
    // Every fixture raised what it was built for, so the agreement is not
    // vacuous.
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        [
            "a_behind:behind",
            "a_quiet:quiet",
            "a_silent:silent",
            "a_tags_quiet:quiet"
        ],
    );
}
