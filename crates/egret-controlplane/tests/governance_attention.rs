//! `GET /api/control-plane/governance/attention` (DG #3, spec §4): what needs
//! attention, per the reader's permissions, each source isolated in a
//! savepoint of one read-only snapshot.
//!
//! The findings are global, so this binary's tests run one at a time
//! ([`SERIAL`]) and each starts by emptying the tables the sources read. The
//! database is private to this test process (`egret_test_db`), so nothing
//! else is affected.
//!
//! DB-gated; FAILS without a database unless `EGRET_ALLOW_DB_SKIP` is set.

use std::collections::HashSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::governance_overview::{
    attention_via, begin_snapshot, Finding, Requires, Source, SourceError, SOURCES,
};
use egret_controlplane::{
    control_plane_router, CpState, ExposureEngine, MaterializedViewLister, MvRow,
    UnwiredCatalogLister,
};
use egret_metastore::Db;
use egret_policy::PolicyStore;
use futures_util::future::BoxFuture;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgConnection;
use tower::ServiceExt;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct NoMvLister;
#[async_trait::async_trait]
impl MaterializedViewLister for NoMvLister {
    async fn list(&self, _engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        panic!("the Overview never lists materialized views")
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

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    // Start from nothing the sources read: this database is this process's.
    sqlx::query(
        "TRUNCATE engine_mgmt.engines, policy.ranger_plugin_downloads, perm.compile_runs, \
                  view_lineage_state, tags, registry.engine_catalog_attachments, \
                  registry.catalogs, perm.grants, perm.rules, policy.authz_services CASCADE",
    )
    .execute(&db.pool)
    .await
    .expect("empty the sources' tables");
    Some(Fixture {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        policy,
        db,
    })
}

/// The control-plane router with a REAL `CedarPdp`, built after every role
/// the test needs exists (the PDP snapshots roles at build time).
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
        .insert_local(&format!("{}@egret.local", unique("att")), "T", "x")
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
        .bind(unique("att_role"))
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

async fn attention(st: &CpState, token: &str) -> (StatusCode, Value) {
    let req = HttpRequest::builder()
        .uri("/governance/attention")
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

fn kinds(body: &Value) -> Vec<String> {
    body["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .map(|f| f["kind"].as_str().unwrap().to_string())
        .collect()
}

fn finding<'a>(body: &'a Value, kind: &str) -> Option<&'a Value> {
    body["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|f| f["kind"] == kind)
}

fn labels(body: &Value, kind: &str) -> Vec<String> {
    finding(body, kind).map_or_else(Vec::new, |f| {
        f["sample"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["label"].as_str().unwrap().to_string())
            .collect()
    })
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

/// Set the profile's counters and when they last moved. After the engine
/// insert: the lineage trigger on `engine_mgmt.engines` bumps them.
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
             (service_id, plugin_id, kind, last_known_version, at) \
         VALUES ($1, $2, $3, $4, now() - make_interval(secs => $5))",
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

/// A provisioned running engine bound to a new profile of `ty`.
async fn live_profile(f: &Fixture, name: &str, ty: &str) -> i64 {
    let sid = profile(f, name, ty).await;
    engine(
        f,
        &format!("{name}-eng"),
        ty,
        "provisioned",
        "running",
        Some(sid),
    )
    .await;
    sid
}

// ── drift ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_drift_rules_follow_the_measured_ledger() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };

    // A pod that polled before the bump, and its successor that confirmed it:
    // the old row is ignored.
    let s = live_profile(&f, "d_ignored", "trino").await;
    counters(&f, s, 5, 1, 120).await;
    poll(&f, s, "trino@old-d_ignored", "policies", 4, 180).await;
    poll(&f, s, "trino@new-d_ignored", "policies", 5, 10).await;

    // Polled after the bump, still on the old version, bump 2 min ago.
    let s = live_profile(&f, "d_behind", "trino").await;
    counters(&f, s, 5, 1, 120).await;
    poll(&f, s, "trino@p-d_behind", "policies", 4, 30).await;

    // The same, bump 30 s ago: not yet.
    let s = live_profile(&f, "d_fresh", "trino").await;
    counters(&f, s, 5, 1, 30).await;
    poll(&f, s, "trino@p-d_fresh", "policies", 4, 10).await;

    // Live, but nothing has polled since the bump 3 min ago.
    let s = live_profile(&f, "d_nopoll", "trino").await;
    counters(&f, s, 5, 1, 120).await;
    poll(&f, s, "trino@p-d_nopoll", "policies", 4, 150).await;

    // Last poll 25 h ago: silent, not quiet.
    let s = live_profile(&f, "d_silent", "starrocks").await;
    counters(&f, s, 5, 1, 30 * 3600).await;
    poll(&f, s, "starrocks@p-d_silent", "policies", 5, 25 * 3600).await;

    // Never polled at all: silent.
    let s = live_profile(&f, "d_never", "starrocks").await;
    counters(&f, s, 5, 1, 30 * 3600).await;

    // Last poll 10 min ago: quiet.
    let s = live_profile(&f, "d_quiet", "starrocks").await;
    counters(&f, s, 5, 1, 3600).await;
    poll(&f, s, "starrocks@p-d_quiet", "policies", 5, 600).await;

    // Bound only to an external engine, never polled: never judged.
    let s = profile(&f, "d_external", "trino").await;
    engine(
        &f,
        "d_external-eng",
        "trino",
        "external",
        "running",
        Some(s),
    )
    .await;
    counters(&f, s, 5, 1, 30 * 3600).await;

    // (a) A rescheduled pod: the old id's rows stay with their last `at`.
    let s = live_profile(&f, "d_resched", "trino").await;
    counters(&f, s, 5, 1, 3600).await;
    poll(&f, s, "trino@old-pod-d_resched", "policies", 5, 1800).await;
    poll(&f, s, "trino@old-pod-d_resched", "tags", 1, 1800).await;
    poll(&f, s, "trino@new-pod-d_resched", "policies", 5, 10).await;
    poll(&f, s, "trino@new-pod-d_resched", "tags", 1, 20).await;

    // (b) Trino's tags row reports -1 right after a policy download.
    let s = live_profile(&f, "d_tags_reset", "trino").await;
    counters(&f, s, 5, 3, 120).await;
    poll(&f, s, "trino@p-d_tags_reset", "policies", 5, 10).await;
    poll(&f, s, "trino@p-d_tags_reset", "tags", -1, 20).await;

    // … while a tags row that reports a real, older version is behind.
    let s = live_profile(&f, "d_tags_stale", "trino").await;
    counters(&f, s, 5, 3, 120).await;
    poll(&f, s, "trino@p-d_tags_stale", "policies", 5, 10).await;
    poll(&f, s, "trino@p-d_tags_stale", "tags", 2, 20).await;

    // (c) StarRocks downloads no tags: a bumped tag version is never "behind".
    let s = live_profile(&f, "d_sr_notags", "starrocks").await;
    counters(&f, s, 5, 3, 120).await;
    poll(&f, s, "starrocks@p-d_sr_notags", "policies", 5, 10).await;

    let st = state(&f).await;
    let token = session(&f, &["platform_admin"]).await;
    let (status, body) = attention(&st, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mut behind = labels(&body, "plugin_behind");
    behind.sort();
    assert_eq!(behind, ["d_behind", "d_nopoll", "d_tags_stale"], "{body}");
    let mut silent = labels(&body, "plugin_silent");
    silent.sort();
    assert_eq!(silent, ["d_never", "d_silent"], "{body}");
    assert_eq!(labels(&body, "plugin_quiet"), ["d_quiet"], "{body}");
    assert_eq!(
        finding(&body, "plugin_silent").unwrap()["severity"],
        "critical"
    );
    assert_eq!(
        finding(&body, "plugin_behind").unwrap()["severity"],
        "critical"
    );
    assert_eq!(
        finding(&body, "plugin_quiet").unwrap()["severity"],
        "warning"
    );
    let quiet = finding(&body, "plugin_quiet").unwrap();
    assert_eq!(
        quiet["sentence"],
        "No plugin has polled profile d_quiet for 10 minutes."
    );
    assert_eq!(quiet["ref"], json!({"type": "profile", "name": "d_quiet"}));
    assert_eq!(quiet["sample"][0]["ref"]["type"], "engine");
    // Live check D12: one profile's sentence is the finding's; its sample does
    // not repeat it.
    assert_eq!(quiet["sample"][0]["detail"], Value::Null);
    let behind_details: Vec<&str> = finding(&body, "plugin_behind").unwrap()["sample"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            s["detail"]
                .as_str()
                .expect("several profiles: each says its own")
        })
        .collect();
    assert_eq!(behind_details.len(), 3);
    assert_eq!(quiet["sample"][0]["ref"]["name"], "d_quiet-eng");
    let tags_stale = finding(&body, "plugin_behind").unwrap()["sample"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["label"] == "d_tags_stale")
        .unwrap()["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        tags_stale.starts_with(
            "No plugin of profile d_tags_stale has reported holding tag version 3, set at "
        ),
        "{tags_stale}"
    );
}

#[tokio::test]
async fn an_engine_that_failed_is_reported_only_when_egret_provisions_it() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let failed = engine(&f, "e_failed", "trino", "provisioned", "failed", None).await;
    engine(&f, "e_external", "trino", "external", "failed", None).await;
    engine(&f, "e_running", "trino", "provisioned", "running", None).await;
    let st = state(&f).await;
    let (_, body) = attention(&st, &session(&f, &["platform_admin"]).await).await;
    let fnd = finding(&body, "engine_failed").expect("engine_failed");
    assert_eq!(fnd["count"], 1, "{body}");
    assert_eq!(
        fnd["sample"][0]["ref"],
        json!({"type": "engine", "id": failed, "name": "e_failed"})
    );
    assert_eq!(fnd["ref"], json!({"type": "engines"}));
}

#[tokio::test]
async fn count_is_the_total_and_the_sample_is_the_first_five() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    for name in ["c_g", "c_c", "c_a", "c_e", "c_b", "c_f", "c_d"] {
        engine(&f, name, "starrocks", "provisioned", "failed", None).await;
    }
    let st = state(&f).await;
    let (_, body) = attention(&st, &session(&f, &["platform_admin"]).await).await;
    let fnd = finding(&body, "engine_failed").expect("engine_failed");
    assert_eq!(fnd["count"], 7);
    assert_eq!(
        labels(&body, "engine_failed"),
        ["c_a", "c_b", "c_c", "c_d", "c_e"]
    );
    assert_eq!(fnd["sentence"], "7 engines egret provisions failed.");
}

#[tokio::test]
async fn compile_failures_lineage_attachments_tags_and_catalogs_each_have_a_finding() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    // compile_failed: the error's first line.
    let sid = profile(&f, "k_broken", "trino").await;
    sqlx::query(
        "INSERT INTO perm.compile_runs (service_id, outcome, attempted_at, error) \
         VALUES ($1, 'error', now(), 'rule overlap: a and b\nsecond line')",
    )
    .bind(sid)
    .execute(&f.db.pool)
    .await
    .unwrap();
    let ok = profile(&f, "k_fine", "trino").await;
    sqlx::query(
        "INSERT INTO perm.compile_runs (service_id, outcome, attempted_at, last_ok_at) \
         VALUES ($1, 'ok', now(), now())",
    )
    .bind(ok)
    .execute(&f.db.pool)
    .await
    .unwrap();
    // lineage_failed and a computed one.
    sqlx::query(
        "INSERT INTO view_lineage_state (view_catalog, view_schema, view_name, state, detail) \
         VALUES ('ice', 's', 'v_bad', 'failed', 'cannot plan'), ('ice', 's', 'v_ok', 'computed', NULL)",
    )
    .execute(&f.db.pool)
    .await
    .unwrap();
    // A catalog attached with a failed apply, and one attached to nothing.
    let eng = engine(&f, "k_eng", "trino", "provisioned", "running", None).await;
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) \
         VALUES ('k_ext', 'tpch', '{}'::jsonb) RETURNING id",
    )
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id, apply_status, apply_error) \
         VALUES ($1, $2, 'failed', 'connection refused')",
    )
    .bind(eng)
    .bind(cat)
    .execute(&f.db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO registry.catalogs (name, kind, config) VALUES ('k_lonely', 'tpch', '{}'::jsonb)")
        .execute(&f.db.pool)
        .await
        .unwrap();
    // A tag with no clearance, one with an empty clearance, one gated.
    for (key, roles) in [
        ("k_open", None),
        ("k_empty", Some(vec![])),
        ("k_gated", Some(vec!["r1"])),
    ] {
        let id: Uuid =
            sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
                .bind(key)
                .fetch_one(&f.db.pool)
                .await
                .unwrap();
        if let Some(roles) = roles {
            sqlx::query(
                "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
                 VALUES ($1, $2, '{}')",
            )
            .bind(id)
            .bind(&roles)
            .execute(&f.db.pool)
            .await
            .unwrap();
        }
    }

    let st = state(&f).await;
    let (_, body) = attention(&st, &session(&f, &["platform_admin"]).await).await;
    let c = finding(&body, "compile_failed").expect("compile_failed");
    assert_eq!(c["count"], 1, "{body}");
    assert_eq!(c["sample"][0]["label"], "k_broken");
    let detail = c["sample"][0]["detail"].as_str().unwrap();
    assert!(detail.starts_with("rule overlap: a and b ("), "{detail}");
    assert!(!detail.contains("second line"), "{detail}");
    assert!(detail.contains("last success none recorded"), "{detail}");

    assert_eq!(labels(&body, "lineage_failed"), ["ice.s.v_bad"]);
    // Live check D3: singular agreement.
    assert_eq!(
        finding(&body, "lineage_failed").unwrap()["sentence"],
        "egret could not compute the lineage of 1 view, so the tables it reads are not known."
    );
    assert_eq!(
        finding(&body, "lineage_failed").unwrap()["ref"],
        json!({"type": "object", "catalog": "ice", "schema": "s", "object": "v_bad"})
    );
    let a = finding(&body, "catalog_apply_failed").expect("catalog_apply_failed");
    assert_eq!(a["sample"][0]["label"], "k_ext on k_eng");
    assert_eq!(a["sample"][0]["detail"], "connection refused");
    // Live check D2.
    assert_eq!(
        a["sentence"],
        "1 catalog attachment could not be applied to its engine."
    );
    assert_eq!(labels(&body, "catalogs_without_engine"), ["k_lonely"]);
    assert_eq!(
        labels(&body, "tags_without_clearance"),
        ["k_empty", "k_open"]
    );
    assert_eq!(
        finding(&body, "tags_without_clearance").unwrap()["sentence"],
        "2 classifications have no clearance: they gate nothing."
    );
    assert_eq!(
        body["counts"],
        json!({"catalogs": 2, "grants": 0, "rules": 0, "tags": 3})
    );
}

fn level(values: &[&str]) -> Value {
    json!({"values": values, "isExcludes": false, "isRecursive": false})
}

async fn hand_written(f: &Fixture, sid: i64, name: &str, body: Value) {
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, true, 'operator')",
    )
    .bind(sid)
    .bind(name)
    .bind({
        let mut body = body;
        body["name"] = json!(name);
        body
    })
    .execute(&f.db.pool)
    .await
    .expect("hand-written policy");
}

#[tokio::test]
async fn policy_warnings_overlaps_and_unmatched_principals_are_reported() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    // Two profiles with access policies that stop above `column` and allow select.
    let mut expected = 0;
    for (name, n) in [("w_one", 1), ("w_two", 2)] {
        let sid = profile(&f, name, "trino").await;
        for i in 0..n {
            hand_written(
                &f,
                sid,
                &format!("{name}-p{i}"),
                json!({
                    "policyType": 0, "isEnabled": true,
                    "resources": {"catalog": level(&[&format!("c{i}")])},
                    "policyItems": [{"users": ["alice"], "groups": [], "roles": [],
                                     "accesses": [{"type": "select", "isAllowed": true}]}]
                }),
            )
            .await;
        }
        let mut conn = f.db.pool.acquire().await.unwrap();
        expected += egret_perm_compile::classify::profile_warnings(&mut conn, sid)
            .await
            .unwrap()
            .warning_count;
    }
    assert!(expected >= 3, "the fixture carries warnings: {expected}");

    // Two hand-written masks on one column that disagree.
    let sid = profile(&f, "o_masks", "trino").await;
    for (name, mask) in [("o_a", "MASK"), ("o_b", "MASK_NULL")] {
        hand_written(
            &f,
            sid,
            name,
            json!({
                "policyType": 1, "isEnabled": true,
                "resources": {"catalog": level(&["ice"]), "schema": level(&["s"]),
                              "table": level(&["t"]), "column": level(&["email"])},
                "dataMaskPolicyItems": [{"users": [], "groups": ["public"], "roles": [],
                    "accesses": [{"type": "select", "isAllowed": true}],
                    "dataMaskInfo": {"dataMaskType": mask}}],
                "policyItems": [], "denyPolicyItems": []
            }),
        )
        .await;
    }

    // A deny grant whose include names a group nobody has (written by SQL:
    // writes refuse it), and an allow grant naming the same (fails closed).
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ('u_cat', 'tpch', '{}'::jsonb) RETURNING id",
    )
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let mut grants = Vec::new();
    for effect in ["deny", "allow"] {
        let g: Uuid = sqlx::query_scalar(
            "INSERT INTO perm.grants (effect, actions, catalog_id) VALUES ($1::perm.effect, '{read}', $2) RETURNING id",
        )
        .bind(effect)
        .bind(cat)
        .fetch_one(&f.db.pool)
        .await
        .expect("grant");
        sqlx::query(
            "INSERT INTO perm.grant_principals (grant_id, mode, kind, ref) \
             VALUES ($1, 'include', 'group', 'ghost')",
        )
        .bind(g)
        .execute(&f.db.pool)
        .await
        .expect("grant principal");
        grants.push(g);
    }

    let st = state(&f).await;
    let (_, body) = attention(&st, &session(&f, &["platform_admin"]).await).await;
    let w = finding(&body, "policy_warnings").expect("policy_warnings");
    assert_eq!(w["count"], expected as i64, "{body}");
    assert_eq!(
        labels(&body, "policy_warnings"),
        ["w_two", "w_one"],
        "most first"
    );
    let o = finding(&body, "rule_overlaps").expect("rule_overlaps");
    assert_eq!(o["count"], 1, "{body}");
    assert_eq!(o["sample"][0]["label"], "o_a and o_b");
    // Live check D4: the coordinate as a path, not Ranger's resource map.
    assert_eq!(
        o["sample"][0]["detail"],
        "ice.s.t.email, in profile o_masks"
    );
    let u = finding(&body, "unresolved_principals_fail_open").expect("unresolved");
    assert_eq!(u["count"], 1, "only the deny fails open: {body}");
    // Live check D1: singular agreement.
    assert_eq!(
        u["sentence"],
        "1 stored principal reference matches nobody, so the compiled policy allows more than the model states."
    );
    assert_eq!(
        u["sample"][0]["ref"],
        json!({"type": "grant", "id": grants[0]})
    );
    assert_eq!(u["ref"], json!({"type": "unmatched"}));
    assert_eq!(body["counts"]["grants"], 2);
}

// ── permissions ──────────────────────────────────────────────────────────

/// One fixture that fires every source a permission can hide.
async fn every_kind(f: &Fixture) {
    let s = live_profile(f, "p_silent", "trino").await;
    counters(f, s, 5, 1, 30 * 3600).await;
    engine(f, "p_failed", "trino", "provisioned", "failed", None).await;
    let eng = engine(f, "p_eng", "starrocks", "provisioned", "running", None).await;
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ('p_cat', 'tpch', '{}'::jsonb) RETURNING id",
    )
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id, apply_status, apply_error) \
         VALUES ($1, $2, 'failed', 'no')",
    )
    .bind(eng)
    .bind(cat)
    .execute(&f.db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO tags (key, name) VALUES ('p_tag', 'p_tag')")
        .execute(&f.db.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO perm.compile_runs (service_id, outcome, attempted_at, error) \
         VALUES ($1, 'error', now(), 'boom')",
    )
    .bind(s)
    .execute(&f.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO view_lineage_state (view_catalog, view_schema, view_name, state, detail) \
         VALUES ('p', 's', 'v', 'failed', 'x')",
    )
    .execute(&f.db.pool)
    .await
    .unwrap();
}

fn not_checked(body: &Value) -> Vec<(String, Value)> {
    body["notChecked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            (
                n["kind"].as_str().unwrap().to_string(),
                n["requires"].clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn each_finding_is_computed_only_for_a_reader_holding_its_permissions() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    every_kind(&f).await;
    let policy_reader = session(&f, &["policy:read"]).await;
    let tag_reader = session(&f, &["tag:read"]).await;
    let nobody = session(&f, &[]).await;
    let admin = session(&f, &["platform_admin"]).await;
    let st = state(&f).await;

    let (status, body) = attention(&st, &policy_reader).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let k = kinds(&body);
    for hidden in [
        "engine_failed",
        "plugin_silent",
        "plugin_behind",
        "plugin_quiet",
        "plugin_unmodelled",
        "catalog_apply_failed",
        "tags_without_clearance",
    ] {
        assert!(!k.contains(&hidden.to_string()), "{hidden}: {body}");
    }
    assert!(k.contains(&"compile_failed".to_string()), "{body}");
    assert!(k.contains(&"lineage_failed".to_string()), "{body}");
    let nc = not_checked(&body);
    for (kind, requires) in [
        ("engine_failed", json!(["engine:read"])),
        ("plugin_silent", json!(["policy:read", "engine:read"])),
        ("plugin_behind", json!(["policy:read", "engine:read"])),
        ("plugin_quiet", json!(["policy:read", "engine:read"])),
        ("plugin_unmodelled", json!(["engine:read"])),
        ("catalog_apply_failed", json!(["catalog_registry:read"])),
        ("tags_without_clearance", json!(["tag:read"])),
    ] {
        assert!(
            nc.contains(&(kind.to_string(), requires.clone())),
            "{kind}: {body}"
        );
    }
    assert_eq!(body["counts"]["tags"], Value::Null);
    assert_eq!(body["counts"]["grants"], 0);

    let (status, body) = attention(&st, &tag_reader).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let k = kinds(&body);
    for hidden in [
        "unresolved_principals_fail_open",
        "rule_overlaps",
        "plugin_silent",
        "compile_failed",
        "policy_warnings",
        "engine_failed",
    ] {
        assert!(!k.contains(&hidden.to_string()), "{hidden}: {body}");
    }
    assert!(k.contains(&"tags_without_clearance".to_string()), "{body}");
    assert!(k.contains(&"lineage_failed".to_string()), "{body}");
    assert_eq!(body["counts"]["grants"], Value::Null);
    assert_eq!(body["counts"]["rules"], Value::Null);
    assert_eq!(body["counts"]["tags"], 1);

    let (status, _) = attention(&st, &nobody).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = attention(&st, &admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["notChecked"], json!([]), "{body}");
    let k = kinds(&body);
    for shown in [
        "plugin_silent",
        "compile_failed",
        "engine_failed",
        "catalog_apply_failed",
        "tags_without_clearance",
        "lineage_failed",
    ] {
        assert!(k.contains(&shown.to_string()), "{shown}: {body}");
    }
    // Ordered critical, warning, info.
    let sev: Vec<&str> = body["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["severity"].as_str().unwrap())
        .collect();
    let rank = |s: &str| {
        ["critical", "warning", "info"]
            .iter()
            .position(|x| *x == s)
            .unwrap()
    };
    assert!(sev.windows(2).all(|w| rank(w[0]) <= rank(w[1])), "{sev:?}");
}

// ── isolation ────────────────────────────────────────────────────────────

fn divide_by_zero(c: &mut PgConnection) -> BoxFuture<'_, Result<Option<Finding>, SourceError>> {
    Box::pin(async move {
        sqlx::query("SELECT 1/0").execute(&mut *c).await?;
        Ok(None)
    })
}

fn write_something(c: &mut PgConnection) -> BoxFuture<'_, Result<Option<Finding>, SourceError>> {
    Box::pin(async move {
        sqlx::query("INSERT INTO tags (key, name) VALUES ('i_written', 'i_written')")
            .execute(&mut *c)
            .await?;
        Ok(None)
    })
}

const ALL: &[&str] = &[
    "policy:read",
    "tag:read",
    "engine:read",
    "catalog_registry:read",
];

#[tokio::test]
async fn a_failing_source_is_reported_and_every_other_source_still_answers() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    engine(&f, "i_failed", "trino", "provisioned", "failed", None).await;
    let st = state(&f).await;
    let held: HashSet<&str> = ALL.iter().copied().collect();
    let mut sources = vec![Source {
        kind: "failing",
        what: "a source that divides by zero",
        requires: Requires::All(&["policy:read"]),
        run: divide_by_zero,
    }];
    sources.extend(SOURCES.iter().map(|s| Source {
        kind: s.kind,
        what: s.what,
        requires: s.requires,
        run: s.run,
    }));
    let mut tx = begin_snapshot(&st).await.expect("snapshot");
    let r = attention_via(&mut tx, &held, &sources).await;
    tx.rollback().await.unwrap();
    let unavailable: Vec<&Finding> = r
        .findings
        .iter()
        .filter(|f| f.kind == "source_unavailable")
        .collect();
    assert_eq!(unavailable.len(), 1, "{:?}", r.findings);
    assert_eq!(unavailable[0].source.as_deref(), Some("failing"));
    // Review M1: a fixed sentence; the database's error is logged, not shown.
    assert_eq!(
        unavailable[0].sentence,
        "Could not check a source that divides by zero."
    );
    assert!(
        r.findings.iter().any(|f| f.kind == "engine_failed"),
        "a source after the failing one still answered: {:?}",
        r.findings
    );
    assert!(r.counts.catalogs.is_some());
}

#[tokio::test]
async fn the_snapshot_is_read_only() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let st = state(&f).await;
    let held: HashSet<&str> = ALL.iter().copied().collect();
    let sources = [Source {
        kind: "writer",
        what: "a source that writes",
        requires: Requires::Any(&["tag:read"]),
        run: write_something,
    }];
    let mut tx = begin_snapshot(&st).await.expect("snapshot");
    let r = attention_via(&mut tx, &held, &sources).await;
    tx.commit().await.unwrap();
    assert_eq!(r.findings.len(), 1, "{:?}", r.findings);
    assert_eq!(r.findings[0].kind, "source_unavailable");
    assert_eq!(
        r.findings[0].sentence,
        "Could not check a source that writes."
    );
    let written: i64 = sqlx::query_scalar("SELECT count(*) FROM tags WHERE key = 'i_written'")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(written, 0);
}

#[tokio::test]
async fn an_engine_on_a_plugin_egret_does_not_model_is_reported_with_its_note() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    // Every image egret knows is modelled today, so "unmodelled" on a
    // provisioned engine is an image egret has not measured (a floating tag).
    sqlx::query(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status) VALUES \
         ('m_floating', 'starrocks', 'egret', \
          '{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1-latest\"}}'::jsonb, 'running'), \
         ('m_pinned', 'trino', 'egret', '{\"imageVersion\":\"483\"}'::jsonb, 'running'), \
         ('m_rw', 'risingwave', 'egret', '{}'::jsonb, 'running')",
    )
    .execute(&f.db.pool)
    .await
    .expect("engines");
    // An external engine declaring a pair egret does not model.
    engine(&f, "m_external", "trino", "external", "running", None).await;
    sqlx::query(
        "UPDATE engine_mgmt.engines SET ranger_plugin_version = '2.9.0' WHERE name = 'm_external'",
    )
    .execute(&f.db.pool)
    .await
    .unwrap();
    let token = session(&f, &["engine:read", "tag:read"]).await;
    let st = state(&f).await;
    let (status, body) = attention(&st, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        labels(&body, "plugin_unmodelled"),
        ["m_external", "m_floating"],
        "{body}"
    );
    let fnd = finding(&body, "plugin_unmodelled").unwrap();
    assert_eq!(fnd["severity"], "warning");
    let floating = &fnd["sample"][1];
    assert!(
        floating["detail"]
            .as_str()
            .unwrap()
            .contains("starrocks/fe-ubuntu:4.1-latest"),
        "{floating}"
    );
    assert_eq!(floating["ref"]["type"], "engine");
    assert_eq!(fnd["ref"], json!({"type": "engines"}));
}

/// Live check D14: a compile error is one long line (an overlap refusal is
/// ~600 characters); the sample shows its start, not all of it.
#[tokio::test]
async fn a_long_compile_error_is_shortened_in_its_sample() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let sid = profile(&f, "k_long", "trino").await;
    let long = format!(
        "recompiling profile \"k_long\" would emit {}",
        "x".repeat(600)
    );
    sqlx::query(
        "INSERT INTO perm.compile_runs (service_id, outcome, attempted_at, error) \
         VALUES ($1, 'error', now(), $2)",
    )
    .bind(sid)
    .bind(&long)
    .execute(&f.db.pool)
    .await
    .unwrap();
    let token = session(&f, &["policy:read"]).await;
    let st = state(&f).await;
    let (_, body) = attention(&st, &token).await;
    let detail = finding(&body, "compile_failed").unwrap()["sample"][0]["detail"]
        .as_str()
        .unwrap()
        .to_string();
    let (head, tail) = detail.split_once(" (failed ").expect("the times follow");
    assert!(head.ends_with('…'), "{head}");
    assert_eq!(head.chars().count(), 201, "{head}");
    assert!(head.starts_with("recompiling profile \"k_long\" would emit xxx"));
    assert!(tail.contains("last success none recorded"), "{tail}");
}

/// Review M6: a Trino profile whose policies are polled but whose tag poll
/// stopped 10 minutes ago has a quiet tag poll, which is what it is told; it
/// is not "behind" on its tag version.
#[tokio::test]
async fn a_tag_poll_gone_quiet_is_reported_as_quiet_not_behind() {
    let _serial = SERIAL.lock().await;
    let Some(f) = setup().await else { return };
    let s = live_profile(&f, "t_tags_quiet", "trino").await;
    counters(&f, s, 5, 3, 3600).await;
    poll(&f, s, "trino@p-t_tags_quiet", "policies", 5, 10).await;
    poll(&f, s, "trino@p-t_tags_quiet", "tags", 3, 600).await;
    let token = session(&f, &["policy:read", "engine:read"]).await;
    let st = state(&f).await;
    let (status, body) = attention(&st, &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(labels(&body, "plugin_behind").is_empty(), "{body}");
    assert_eq!(labels(&body, "plugin_quiet"), ["t_tags_quiet"], "{body}");
    assert_eq!(
        finding(&body, "plugin_quiet").unwrap()["sentence"],
        "No plugin has polled the tags of profile t_tags_quiet for 10 minutes, while its policies are polled."
    );
}
