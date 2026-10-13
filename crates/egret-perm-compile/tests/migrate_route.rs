//! `POST /perm/policies/services/{id}/migrate` and `…/migrate/undo` (SP4d-3
//! Task 7), driven through the real router against the real schema.
//!
//! Each test builds its OWN profiles, engines, catalog and warehouse, and
//! every read is scoped to those ids — the test database is shared. The
//! profiles carry no `tagService`, so tag rules and clearances other tests
//! leave in the shared database cannot compile into them (a Trino tag policy
//! needs a tag service), and every grant/rule the recompile sees is on this
//! fixture's own catalog. That isolation is what makes "the gate found no
//! disagreement" a statement about the migration and not about the
//! neighbours.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-perm-compile --test migrate_route

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    db: Db,
    policy: PolicyStore,
    app: axum::Router,
    token: String,
    /// The source profile, bound to one Trino engine serving `catalog`.
    p: i64,
    catalog: String,
    catalog_id: Uuid,
    warehouse_id: Uuid,
    dsn: String,
}

async fn trino_profile_with_engine(db: &Db, policy: &PolicyStore, catalog_id: Uuid) -> i64 {
    trino_profile_with_engine_and_config(db, policy, Some(catalog_id), json!({})).await
}

/// A Trino profile with one engine, attached to `catalog_id` when given.
async fn trino_profile_with_engine_and_config(
    db: &Db,
    policy: &PolicyStore,
    catalog_id: Option<Uuid>,
    config: Value,
) -> i64 {
    let svc = policy
        .create_service(&unique("mig_trino"), "trino", &config)
        .await
        .expect("create trino service");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{\"imageVersion\":\"470\"}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("mig-eng"))
    .bind(svc.id)
    .fetch_one(&db.pool)
    .await
    .expect("engine");
    if let Some(catalog_id) = catalog_id {
        sqlx::query(
            "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
        )
        .bind(engine_id)
        .bind(catalog_id)
        .execute(&db.pool)
        .await
        .expect("attach catalog");
    }
    svc.id
}

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();

    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("mig-ws"))
            .bind("Migrate WS")
            .fetch_one(&pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-migrate-test','AKIATEST') RETURNING id",
    )
    .bind(unique("mig-sc"))
    .fetch_one(&pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("mig_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(&pool)
    .await
    .expect("warehouse");
    let catalog = unique("mig_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog)
    .bind(warehouse_id)
    .fetch_one(&pool)
    .await
    .expect("registry catalog");
    let p = trino_profile_with_engine(&db, &policy, catalog_id).await;

    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let users = UserRepo::new(db.clone());
    let uid = users
        .insert_local(
            &format!("migrate_{}@egret.local", Uuid::new_v4().simple()),
            "Test",
            "x",
        )
        .await
        .expect("insert user")
        .0;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(uid)
    .execute(&pool)
    .await
    .expect("grant platform_admin");
    let token = sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    let app = perm_router().with_state(PermState {
        perm,
        policy: policy.clone(),
        sessions,
        pdp,
    });
    Some(Fixture {
        db,
        policy,
        app,
        token,
        p,
        catalog,
        catalog_id,
        warehouse_id,
        dsn,
    })
}

fn level(values: &[&str]) -> Value {
    json!({"values": values, "isExcludes": false, "isRecursive": false})
}

/// An access policy granting `user` select on `catalog.<schema>.orders.*`.
fn access_policy(catalog: &str, schema: &str, user: &str) -> Value {
    json!({
        "resources": {
            "catalog": level(&[catalog]),
            "schema":  level(&[schema]),
            "table":   level(&["orders"]),
            "column":  level(&["*"])
        },
        "policyItems": [{
            "users": [user], "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}]
        }],
        "denyPolicyItems": [],
        "isEnabled": true
    })
}

/// A path mask on `catalog.sales.orders.ssn` for everyone.
fn mask_policy(catalog: &str) -> Value {
    json!({
        "policyType": 1,
        "isEnabled": true,
        "resources": {
            "catalog": level(&[catalog]),
            "schema":  level(&["sales"]),
            "table":   level(&["orders"]),
            "column":  level(&["ssn"])
        },
        "dataMaskPolicyItems": [{
            "users": [], "groups": ["public"], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK_NULL"}
        }],
        "policyItems": [],
        "denyPolicyItems": []
    })
}

async fn insert(f: &Fixture, service_id: i64, name: &str, managed_by: &str, body: Value) -> i64 {
    let mut body = body;
    body["name"] = json!(name);
    sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, true, $4) RETURNING id",
    )
    .bind(service_id)
    .bind(name)
    .bind(body)
    .bind(managed_by)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert policy")
}

async fn post(f: &Fixture, uri: String, body: Value) -> (StatusCode, Value) {
    let request = HttpRequest::builder()
        .method("POST")
        .uri(uri)
        .header("Cookie", format!("egret_session={}", f.token))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = f.app.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

async fn migrate(
    f: &Fixture,
    service_id: i64,
    names: &[&str],
    dry_run: bool,
) -> (StatusCode, Value) {
    post(
        f,
        format!("/perm/policies/services/{service_id}/migrate"),
        json!({"policy_names": names, "dry_run": dry_run}),
    )
    .await
}

async fn undo(f: &Fixture, service_id: i64, names: &[&str]) -> (StatusCode, Value) {
    post(
        f,
        format!("/perm/policies/services/{service_id}/migrate/undo"),
        json!({"policy_names": names}),
    )
    .await
}

async fn grants_on_catalog(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM perm.grants WHERE catalog_id = $1")
        .bind(f.catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("count grants")
}

async fn rules_on_catalog(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM perm.rules WHERE catalog_id = $1")
        .bind(f.catalog_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("count rules")
}

async fn provenance(f: &Fixture, service_id: i64) -> Vec<(String, i32)> {
    sqlx::query_as(
        "SELECT policy_name, probes_proved FROM perm.migrated_from WHERE service_id = $1 \
         ORDER BY policy_name",
    )
    .bind(service_id)
    .fetch_all(&f.db.pool)
    .await
    .expect("provenance")
}

async fn enabled(f: &Fixture, policy_id: i64) -> bool {
    sqlx::query_scalar("SELECT enabled FROM policy.authz_policies WHERE id = $1")
        .bind(policy_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("enabled")
}

/// Policy names in the download the profile's engine receives — through the
/// download route's own assembler.
async fn downloaded_names(f: &Fixture, service_id: i64) -> Vec<String> {
    f.policy
        .enabled_policies_for_service(service_id)
        .await
        .expect("download")
        .iter()
        .map(|p| p["name"].as_str().expect("name").to_string())
        .collect()
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {v}"))
        .keys()
        .cloned()
        .collect();
    k.sort();
    k
}

// ── 1. happy path ────────────────────────────────────────────────────────

#[tokio::test]
async fn a_migratable_policy_is_migrated_proved_and_switched_off() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["equivalent"], true, "{body}");
    assert_eq!(body["committed"], true, "{body}");
    eprintln!(
        "MEASURE happy probe_evaluations={}",
        body["probe_evaluations"]
    );

    // The response names the created objects.
    let grant_ids = body["policies"][0]["grant_ids"]
        .as_array()
        .expect("grant_ids")
        .clone();
    assert_eq!(grant_ids.len(), 1, "{body}");
    assert_eq!(body["policies"][0]["policy_name"], json!(name));
    let gid: Uuid = grant_ids[0].as_str().unwrap().parse().unwrap();
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.grants WHERE id = $1")
        .bind(gid)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(stored, 1, "perm.grants gained the named row");

    assert!(!enabled(&f, id).await, "the original is switched off");
    let prov = provenance(&f, f.p).await;
    assert_eq!(prov.len(), 1, "{prov:?}");
    assert_eq!(prov[0].0, name);
    assert!(prov[0].1 > 0, "probes_proved > 0: {prov:?}");
    assert_eq!(
        i64::from(prov[0].1),
        body["probe_evaluations"].as_i64().unwrap()
    );

    let names = downloaded_names(&f, f.p).await;
    assert!(
        !names.contains(&name),
        "the download no longer serves it: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.starts_with("perm-grant-")),
        "the download serves the compiled grant: {names:?}"
    );
    // The artifact diff names both sides, and the generated-profile notice is there.
    let prof = &body["profiles"][0];
    assert_eq!(prof["service_id"], f.p, "{body}");
    assert_eq!(prof["removed_policies"], json!([name]), "{body}");
    assert_eq!(prof["plugin_versions"], json!(["ranger_2_5_0"]), "{body}");
    assert!(
        body["notices"][0]
            .as_str()
            .unwrap()
            .contains("can no longer be created"),
        "{body}"
    );
}

// ── 2. dry run writes nothing ────────────────────────────────────────────

#[tokio::test]
async fn a_dry_run_reports_the_same_shape_and_writes_nothing() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let (status, body) = migrate(&f, f.p, &[&name], true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["equivalent"], true, "{body}");
    assert_eq!(body["committed"], false, "{body}");
    assert_eq!(body["dry_run"], true, "{body}");
    assert_eq!(
        body["policies"][0]["grant_ids"].as_array().unwrap().len(),
        1
    );

    // All three, separately.
    assert_eq!(grants_on_catalog(&f).await, 0, "perm.grants unchanged");
    assert!(
        provenance(&f, f.p).await.is_empty(),
        "perm.migrated_from unchanged"
    );
    assert!(enabled(&f, id).await, "the original is still on");
    assert!(downloaded_names(&f, f.p).await.contains(&name));

    // Identical response shape: the real migration answers with the same keys.
    let (status2, real) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status2, StatusCode::OK, "{real}");
    assert_eq!(keys(&body), keys(&real));
    assert_eq!(body["probe_evaluations"], real["probe_evaluations"]);
}

// ── 3. a non-equivalent migration rolls back ─────────────────────────────

#[tokio::test]
async fn a_migration_the_gate_cannot_prove_is_returned_with_its_disagreements_and_rolled_back() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    // A sibling the evaluator cannot decide (it is active only at set times):
    // every probe it reaches is `Undecidable` on both sides, which the gate
    // treats as a disagreement — fail closed.
    let mut scheduled = access_policy(&f.catalog, "sales", "bob");
    scheduled["validitySchedules"] =
        json!([{"startTime": "2026/01/01 00:00:00", "endTime": "2027/01/01 00:00:00"}]);
    insert(&f, f.p, &unique("bob, office hours"), "operator", scheduled).await;

    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["equivalent"], false, "{body}");
    assert_eq!(body["committed"], false, "{body}");
    let ds = body["disagreements"].as_array().expect("disagreements");
    assert!(!ds.is_empty(), "{body}");
    let d = ds
        .iter()
        .find(|d| d["user"] == "alice" && d["access_type"] == "select")
        .unwrap_or_else(|| panic!("no alice/select disagreement: {body}"));
    assert_eq!(d["service_id"], f.p);
    assert_eq!(d["plugin_version"], "ranger_2_5_0");
    assert!(
        d["resource"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{}.sales.orders", f.catalog)),
        "{d}"
    );
    let sentence = d["sentence"].as_str().unwrap();
    assert!(
        sentence.contains("Ranger 2.5.0") && sentence.contains("alice"),
        "{sentence}"
    );

    // AND nothing was written.
    assert_eq!(grants_on_catalog(&f).await, 0, "no grant left behind");
    assert!(
        provenance(&f, f.p).await.is_empty(),
        "no provenance left behind"
    );
    assert!(enabled(&f, id).await, "the original is still on");
    assert!(downloaded_names(&f, f.p).await.contains(&name));
}

// ── 4. a second migration is 409 ─────────────────────────────────────────

#[tokio::test]
async fn migrating_the_same_policy_twice_is_409_with_one_provenance_row() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // 409, not the 400 "switched off" the disabled original would otherwise get.
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("already migrated"),
        "{body}"
    );
    assert_eq!(provenance(&f, f.p).await.len(), 1);
    assert_eq!(grants_on_catalog(&f).await, 1);
}

// ── 5. undo restores ─────────────────────────────────────────────────────

#[tokio::test]
async fn undo_deletes_the_objects_restores_the_original_and_drops_provenance() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(grants_on_catalog(&f).await, 1);

    let (status, body) = undo(&f, f.p, &[&name]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["policies"][0]["grants_deleted"], 1, "{body}");
    assert_eq!(grants_on_catalog(&f).await, 0, "the model objects are gone");
    assert!(enabled(&f, id).await, "the original is back on");
    assert!(
        provenance(&f, f.p).await.is_empty(),
        "the provenance row is gone"
    );
    let names = downloaded_names(&f, f.p).await;
    assert!(names.contains(&name), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("perm-grant-")),
        "the compiled grant is reaped: {names:?}"
    );

    // And a second undo has nothing on record.
    let (status, _) = undo(&f, f.p, &[&name]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── every affected profile ───────────────────────────────────────────────

#[tokio::test]
async fn a_migration_that_widens_another_profile_serving_the_catalog_is_refused_on_that_profile() {
    let Some(f) = setup().await else { return };
    // A SECOND profile whose engine also serves the catalog: the migrated
    // grant compiles into it too, where alice had nothing.
    let q = trino_profile_with_engine(&f.db, &f.policy, f.catalog_id).await;
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let (status, body) = migrate(&f, f.p, &[&name], true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let ds = body["disagreements"].as_array().expect("disagreements");
    assert!(!ds.is_empty(), "{body}");
    assert!(
        ds.iter().all(|d| d["service_id"] == q),
        "only Q changes: {body}"
    );
    assert!(
        ds.iter().any(|d| d["user"] == "alice"
            && d["access_type"] == "select"
            && d["after"].as_str().unwrap().starts_with("allowed")),
        "{body}"
    );
    // The preview lists BOTH profiles, and Q's download gains the grant.
    let profiles = body["profiles"].as_array().unwrap();
    assert_eq!(profiles.len(), 2, "{body}");
    let qp = profiles
        .iter()
        .find(|p| p["service_id"] == q)
        .expect("Q listed");
    assert!(qp["disagreements"].as_u64().unwrap() > 0, "{qp}");
    assert!(
        qp["added_policies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().starts_with("perm-grant-")),
        "{qp}"
    );
    assert_eq!(grants_on_catalog(&f).await, 0);
    assert!(enabled(&f, id).await);
}

// ── refusals, in order ───────────────────────────────────────────────────

#[tokio::test]
async fn a_rule_name_that_is_taken_is_a_clean_409_and_nothing_is_written() {
    let Some(f) = setup().await else { return };
    let name = unique("mask ssn");
    let id = insert(&f, f.p, &name, "operator", mask_policy(&f.catalog)).await;
    // A rule already holding that name (on this fixture's catalog, on a
    // table no policy names).
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, schema_name, \
                                 table_name, column_name) \
         VALUES ($1, 'mask', 'MASK_NULL', 'path', $2, 'other', 'other', 'other')",
    )
    .bind(&name)
    .bind(f.catalog_id)
    .execute(&f.db.pool)
    .await
    .expect("pre-existing rule");
    let before = rules_on_catalog(&f).await;

    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains(&format!("{name:?}")),
        "{body}"
    );
    assert_eq!(rules_on_catalog(&f).await, before);
    assert!(enabled(&f, id).await);
    assert!(provenance(&f, f.p).await.is_empty());
}

#[tokio::test]
async fn a_path_mask_migrates_into_a_rule() {
    let Some(f) = setup().await else { return };
    let name = unique("mask ssn");
    let id = insert(&f, f.p, &name, "operator", mask_policy(&f.catalog)).await;
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    eprintln!(
        "MEASURE mask probe_evaluations={}",
        body["probe_evaluations"]
    );
    assert_eq!(body["policies"][0]["rule_ids"].as_array().unwrap().len(), 1);
    assert_eq!(rules_on_catalog(&f).await, 1);
    assert!(!enabled(&f, id).await);
}

#[tokio::test]
async fn refusals_answer_404_then_409_then_400() {
    let Some(f) = setup().await else { return };
    let (status, _) = migrate(&f, f.p, &["no such policy"], false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = migrate(&f, i64::MAX, &["x"], false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = migrate(&f, f.p, &[], false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let infra = insert(
        &f,
        f.p,
        &format!("egret-admin - {}", unique("x")),
        "infra",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    let infra_name: String =
        sqlx::query_scalar("SELECT name FROM policy.authz_policies WHERE id = $1")
            .bind(infra)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let (status, body) = migrate(&f, f.p, &[&infra_name], false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let mut blocked = access_policy(&f.catalog, "sales", "alice");
    blocked["policyItems"][0]["accesses"] = json!([{"type": "alter", "isAllowed": true}]);
    let blocked_name = unique("alice alters");
    insert(&f, f.p, &blocked_name, "operator", blocked).await;
    let (status, body) = migrate(&f, f.p, &[&blocked_name], false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("`alter`"),
        "{body}"
    );

    // A request naming both: the 409 class wins over the 400 class.
    let (status, body) = migrate(&f, f.p, &[&blocked_name, &infra_name], false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(grants_on_catalog(&f).await, 0);
}

#[tokio::test]
async fn a_fold_divergent_tag_assignment_refuses_the_migration() {
    let Some(f) = setup().await else { return };
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
            .bind(unique("phi"))
            .fetch_one(&f.db.pool)
            .await
            .expect("tag");
    // U+017F LATIN SMALL LETTER LONG S — the folds disagree on it.
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 'sales', 'ſhipments', '')",
    )
    .bind(tag_id)
    .bind(f.warehouse_id)
    .execute(&f.db.pool)
    .await
    .expect("assignment");
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let refusal = body["refusal"].as_str().expect("refusal");
    assert!(
        refusal.contains("ſhipments") && refusal.contains("case folding"),
        "{refusal}"
    );
    assert_eq!(grants_on_catalog(&f).await, 0);
    assert!(enabled(&f, id).await);
}

#[tokio::test]
async fn a_fold_divergent_policy_value_refuses_the_migration() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "ſales", "alice"),
    )
    .await;
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let refusal = body["refusal"].as_str().expect("refusal");
    assert!(refusal.contains("`ſales`"), "{refusal}");
    assert_eq!(grants_on_catalog(&f).await, 0);
    assert!(enabled(&f, id).await);
}

#[tokio::test]
async fn a_migrated_policy_is_classified_as_migrated_not_as_switched_off() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let request = HttpRequest::builder()
        .method("GET")
        .uri(format!("/perm/policies/services/{}/classify", f.p))
        .header("Cookie", format!("egret_session={}", f.token))
        .body(Body::empty())
        .unwrap();
    let resp = f.app.clone().oneshot(request).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let rows: Value = serde_json::from_slice(&bytes).unwrap();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("{rows}"));
    assert_eq!(row["enabled"], false);
    assert_eq!(row["migratable"], false);
    assert_eq!(
        row["migrated"]["grant_ids"].as_array().unwrap().len(),
        1,
        "{row}"
    );
    let blockers = row["blockers"].as_array().unwrap();
    assert_eq!(blockers.len(), 1, "{row}");
    let b = blockers[0].as_str().unwrap();
    assert!(b.contains("was migrated") && b.contains("undo"), "{b}");
    assert!(
        !b.contains("Switch it on"),
        "no switch-it-on advice for a migrated policy: {b}"
    );
}

// ── fix round 1 ──────────────────────────────────────────────────────────

/// Review I-1: the cap is checked on a COUNT, before any probe is built, and
/// the refusal is a 422 that writes nothing. Forty sibling policies make the
/// profile's own download large enough; the migrated policy is one of them.
#[tokio::test]
async fn a_profile_too_large_to_prove_is_refused_by_count_with_nothing_written() {
    let Some(f) = setup().await else { return };
    // Changed by pruning (SP4d-4 Task 6): one migrated policy among 40 siblings
    // now prunes to a few thousand probes and fits the cap, so the fixture has
    // to exceed the cap UNDER pruning: 20 migrated policies (each one's grant
    // is a changed policy whose coordinates stay) among 60 in the profile.
    for i in 0..40 {
        insert(
            &f,
            f.p,
            &unique(&format!("sibling {i}")),
            "operator",
            access_policy(&f.catalog, &format!("s{i}"), &format!("user{i}")),
        )
        .await;
    }
    let mut names = Vec::new();
    let mut first = 0;
    for i in 0..20 {
        let name = unique(&format!("migrated {i}"));
        let id = insert(
            &f,
            f.p,
            &name,
            "operator",
            access_policy(&f.catalog, &format!("m{i}"), &format!("migrant{i}")),
        )
        .await;
        if i == 0 {
            first = id;
        }
        names.push(name);
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();

    let started = std::time::Instant::now();
    let (status, body) = migrate(&f, f.p, &refs, false).await;
    let elapsed = started.elapsed();
    eprintln!("MEASURE cap refusal elapsed={elapsed:?}");
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let refusal = body["refusal"].as_str().expect("refusal");
    assert!(refusal.contains("probe evaluations"), "{refusal}");
    assert!(
        refusal.contains("the largest serves 60 policies"),
        "{refusal}"
    );
    assert!(refusal.contains("never cut short"), "{refusal}");
    assert_eq!(
        body["probe_evaluations"], 0,
        "nothing was evaluated: {body}"
    );
    assert_eq!(grants_on_catalog(&f).await, 0);
    assert!(provenance(&f, f.p).await.is_empty());
    assert!(enabled(&f, first).await);
}

/// SP4d-4 Task 6: the dry run and the real migration prove on the PRUNED set,
/// and say how much smaller it is than the full one.
#[tokio::test]
async fn the_dry_run_and_the_real_migration_prove_on_the_same_pruned_set() {
    let Some(f) = setup().await else { return };
    for i in 0..10 {
        insert(
            &f,
            f.p,
            &unique(&format!("sibling {i}")),
            "operator",
            access_policy(&f.catalog, &format!("s{i}"), &format!("user{i}")),
        )
        .await;
    }
    let name = unique("alice reads orders");
    insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let (status, dry) = migrate(&f, f.p, &[&name], true).await;
    assert_eq!(status, StatusCode::OK, "{dry}");
    let profile = &dry["profiles"][0];
    let (pruned, full) = (
        profile["probes"].as_u64().unwrap(),
        profile["probes_full"].as_u64().unwrap(),
    );
    assert!(
        pruned > 0 && pruned < full,
        "pruning removed probes: {pruned} of {full}"
    );
    assert!(
        profile.get("pruning_fallback").is_none(),
        "no fallback was needed: {profile}"
    );

    let (status, real) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{real}");
    assert_eq!(real["profiles"][0]["probes"], dry["profiles"][0]["probes"]);
    assert_eq!(
        real["profiles"][0]["probes_full"],
        dry["profiles"][0]["probes_full"]
    );
    assert_eq!(real["probe_evaluations"], dry["probe_evaluations"]);
}

/// SP4d-4 final review I-1: the route-level counterpart of `gate_timing.rs`.
/// The migration holds the profile lock across the recompile, the two
/// `artifact_via` reads, the cap count, the case build and `compare`; this
/// times a whole dry run of the request (which does all of it and rolls back)
/// on one profile of N single-user policies, growing N until the cap refuses.
/// Release only; the largest admitted request must stay within the 2.5 s the
/// cap's doc promises.
///
/// ```bash
/// DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
///   cargo test -p egret-perm-compile --release --test migrate_route \
///   a_dry_run_at_the_cap_holds_the_lock_no_longer_than_promised -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "timing harness; run with --release -- --ignored --nocapture"]
async fn a_dry_run_at_the_cap_holds_the_lock_no_longer_than_promised() {
    let Some(f) = setup().await else { return };
    let mut inserted = 0usize;
    let mut slowest_admitted = 0.0f64;
    for target in (60..=140).step_by(10) {
        while inserted < target {
            insert(
                &f,
                f.p,
                &unique(&format!("sibling {inserted}")),
                "operator",
                access_policy(
                    &f.catalog,
                    &format!("s{inserted}"),
                    &format!("user{inserted}"),
                ),
            )
            .await;
            inserted += 1;
        }
        let name = unique("migrated");
        insert(
            &f,
            f.p,
            &name,
            "operator",
            access_policy(&f.catalog, "sales", "alice"),
        )
        .await;
        let started = std::time::Instant::now();
        let (status, body) = migrate(&f, f.p, &[&name], true).await;
        let secs = started.elapsed().as_secs_f64();
        eprintln!(
            "MEASURE route dry run policies={} status={status} probes={} full={} elapsed={secs:.2}s",
            inserted + 1,
            body["profiles"][0]["probes"],
            body["profiles"][0]["probes_full"],
        );
        if status == StatusCode::OK {
            slowest_admitted = slowest_admitted.max(secs);
        } else {
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            break;
        }
        sqlx::query("DELETE FROM policy.authz_policies WHERE service_id = $1 AND name = $2")
            .bind(f.p)
            .bind(&name)
            .execute(&f.db.pool)
            .await
            .expect("remove the migrated candidate");
    }
    eprintln!("MEASURE route slowest admitted dry run = {slowest_admitted:.2}s");
    assert!(
        slowest_admitted <= 2.5,
        "the largest admitted request held the lock for {slowest_admitted:.2}s"
    );
}

async fn edit_original(f: &Fixture, id: i64, name: &str, body: Value) {
    let mut body = body;
    body["name"] = json!(name);
    f.policy
        .update_policy(id, name, &body, false)
        .await
        .expect("update the switched-off original");
}

/// Review I-2: undo after a migrated grant was edited is a 409 naming it, and
/// nothing changes.
#[tokio::test]
async fn undo_after_the_migrated_grant_was_edited_is_409_and_changes_nothing() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let gid: Uuid = body["policies"][0]["grant_ids"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    sqlx::query("UPDATE perm.grants SET table_name = 'invoices' WHERE id = $1")
        .bind(gid)
        .execute(&f.db.pool)
        .await
        .expect("edit the grant");

    let (status, body) = undo(&f, f.p, &[&name]).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let e = body["error"].as_str().unwrap();
    assert!(
        e.contains(&format!("grant {gid}")) && e.contains("edited"),
        "{e}"
    );
    assert!(e.contains("created by the migration"), "{e}");
    assert_eq!(grants_on_catalog(&f).await, 1, "the grant is still there");
    assert!(!enabled(&f, id).await, "the original is still off");
    assert_eq!(provenance(&f, f.p).await.len(), 1);
}

/// Re-review N-1: the hash check must be made UNDER the profile locks. A second
/// session holds the source profile's row lock (what every hand-written write
/// holds until it commits), undo starts and parks on it, and while it is parked
/// the migrated grant is edited and committed. Undo's first reading, taken
/// before the lock, saw an intact grant; only a reading made after the lock is
/// held can see the edit. It must answer 409 and delete nothing.
///
/// Driven without a sleep: the holder's backend pid is polled in
/// `pg_blocking_pids` until some backend is blocked by it, and only then is the
/// edit committed. (With the check before the lock, this test fails: undo
/// deletes the edited grant and answers 200.)
#[tokio::test]
async fn undo_whose_grant_was_edited_while_it_waited_for_the_lock_is_409_and_deletes_nothing() {
    use sqlx::Connection as _;
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let gid: Uuid = body["policies"][0]["grant_ids"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let mut holder = sqlx::PgConnection::connect(&f.dsn).await.expect("holder");
    let holder_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut holder)
        .await
        .expect("holder pid");
    sqlx::query("BEGIN").execute(&mut holder).await.unwrap();
    sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE")
        .bind(f.p)
        .execute(&mut holder)
        .await
        .expect("hold the profile lock");

    let (app, token, uri) = (
        f.app.clone(),
        f.token.clone(),
        format!("/perm/policies/services/{}/migrate/undo", f.p),
    );
    let body = json!({"policy_names": [&name]}).to_string();
    let undoing = tokio::spawn(async move {
        let request = HttpRequest::builder()
            .method("POST")
            .uri(uri)
            .header("Cookie", format!("egret_session={token}"))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(request).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).to_string())
    });

    let mut parked = false;
    for _ in 0..400 {
        parked = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
              WHERE $1 = ANY(pg_blocking_pids(pid)))",
        )
        .bind(holder_pid)
        .fetch_one(&f.db.pool)
        .await
        .expect("read pg_blocking_pids");
        if parked {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(parked, "undo never parked on the held profile lock");

    // Behind undo's back, as the PATCH that committed first would.
    sqlx::query("UPDATE perm.grants SET table_name = 'invoices' WHERE id = $1")
        .bind(gid)
        .execute(&f.db.pool)
        .await
        .expect("edit the grant under the parked undo");
    sqlx::query("COMMIT").execute(&mut holder).await.unwrap();

    let (status, text) = undoing.await.expect("undo task");
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(
        text.contains(&format!("grant {gid}")) && text.contains("edited"),
        "{text}"
    );
    assert_eq!(grants_on_catalog(&f).await, 1, "the edited grant survives");
    assert!(!enabled(&f, id).await, "the original is still off");
    assert_eq!(provenance(&f, f.p).await.len(), 1);
}

/// Review I-2: undo after the switched-off original was edited is a 409.
#[tokio::test]
async fn undo_after_the_original_was_edited_is_409_and_changes_nothing() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    edit_original(&f, id, &name, access_policy(&f.catalog, "sales", "mallory")).await;

    let (status, body) = undo(&f, f.p, &[&name]).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let e = body["error"].as_str().unwrap();
    assert!(
        e.contains("the original policy") && e.contains("edited"),
        "{e}"
    );
    assert_eq!(grants_on_catalog(&f).await, 1);
    assert!(!enabled(&f, id).await);
    assert_eq!(provenance(&f, f.p).await.len(), 1);
}

/// Review I-3: provenance is keyed by policy id — a renamed original is still
/// reported as migrated, and is undone by its new name; the undo says the
/// profile stays generated.
#[tokio::test]
async fn a_renamed_original_stays_migrated_and_is_undone_by_its_new_name() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let renamed = unique("renamed");
    edit_original(
        &f,
        id,
        &renamed,
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let request = HttpRequest::builder()
        .method("GET")
        .uri(format!("/perm/policies/services/{}/classify", f.p))
        .header("Cookie", format!("egret_session={}", f.token))
        .body(Body::empty())
        .unwrap();
    let resp = f.app.clone().oneshot(request).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let rows: Value = serde_json::from_slice(&bytes).unwrap();
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap();
    assert_eq!(row["name"], json!(renamed));
    assert_eq!(row["migrated"]["policy_name"], json!(name), "{row}");
    assert!(
        row["blockers"][0]
            .as_str()
            .unwrap()
            .contains("was migrated"),
        "{row}"
    );

    let (status, body) = undo(&f, f.p, &[&renamed]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["notices"][0]
            .as_str()
            .unwrap()
            .contains("stays compiled"),
        "{body}"
    );
    assert!(enabled(&f, id).await);
    assert_eq!(grants_on_catalog(&f).await, 0);
    assert!(provenance(&f, f.p).await.is_empty());
}

/// I-3's ON DELETE choice: the migrated original cannot be deleted (undo
/// first), and deleting the whole profile still cascades.
#[tokio::test]
async fn a_migrated_original_cannot_be_deleted_but_its_profile_can() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let err = f.policy.delete_policy(id).await.expect_err("refused");
    assert!(
        matches!(&err, egret_policy::PolicyStoreError::CompilerOwned(m) if m.contains("undo the migration")),
        "{err:?}"
    );
    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(f.p)
        .execute(&f.db.pool)
        .await
        .expect("the profile delete cascades through provenance");
    assert!(provenance(&f, f.p).await.is_empty());
}

/// Review I-4: X serves the catalog, Y does not, both download tag service T.
/// X's recompile writes into T, so Y's download can change — Y is gated too.
#[tokio::test]
async fn a_profile_sharing_the_tag_service_is_gated_too() {
    let Some(f) = setup().await else { return };
    let t = f
        .policy
        .create_service(&unique("mig_tag"), "tag", &json!({}))
        .await
        .expect("tag service");
    let x = trino_profile_with_engine_and_config(
        &f.db,
        &f.policy,
        Some(f.catalog_id),
        json!({"tagService": t.name}),
    )
    .await;
    let y =
        trino_profile_with_engine_and_config(&f.db, &f.policy, None, json!({"tagService": t.name}))
            .await;
    let name = unique("alice reads orders");
    insert(
        &f,
        x,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    // The fixture's first profile `f.p` serves the catalog too; it is in the
    // set through the grant. Y is in it only through T.
    let (_, body) = migrate(&f, x, &[&name], true).await;
    let gated: Vec<i64> = body["profiles"]
        .as_array()
        .unwrap_or_else(|| panic!("{body}"))
        .iter()
        .map(|p| p["service_id"].as_i64().unwrap())
        .collect();
    assert!(
        gated.contains(&y),
        "Y downloads T and must be gated: {body}"
    );
    assert!(gated.contains(&x), "{body}");
    assert!(gated.contains(&f.p), "{body}");
    assert_eq!(gated.len(), 3, "{body}");
    assert_eq!(body["committed"], false);
}

// ── final review I-2: a profile recompiled outside the gated set ─────────

/// `write_grant` recomputes the profiles a grant reaches, inside the migration's
/// transaction, AFTER the gate's `affected` set was computed. A catalog attached
/// to another engine in between adds a profile that is recompiled but never
/// gated. That was a `debug_assert!`, compiled out of release builds: the
/// profile's new policy would commit ungated. It is now a refusal.
///
/// The race is forced deterministically: a trigger on `perm.grants`, scoped to
/// this fixture's catalog, attaches the catalog to profile Q's engine when the
/// FIRST migrated grant is inserted. The SECOND grant's `write_grant` then sees
/// Q and recompiles it. Q is not in `affected`, so the gate alone would prove P
/// and commit.
#[tokio::test]
async fn a_profile_recompiled_outside_the_gated_set_is_refused_and_nothing_is_written() {
    let Some(f) = setup().await else { return };
    let q = trino_profile_with_engine_and_config(&f.db, &f.policy, None, json!({})).await;
    let q_engine: Uuid =
        sqlx::query_scalar("SELECT id FROM engine_mgmt.engines WHERE authz_service_id = $1")
            .bind(q)
            .fetch_one(&f.db.pool)
            .await
            .expect("q engine");
    let a = unique("alice reads sales");
    let b = unique("alice reads hr");
    let a_id = insert(
        &f,
        f.p,
        &a,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;
    let b_id = insert(
        &f,
        f.p,
        &b,
        "operator",
        access_policy(&f.catalog, "hr", "alice"),
    )
    .await;

    let tag = Uuid::new_v4().simple().to_string();
    let func = format!("public.fw_attach_mid_migration_{tag}");
    let trig = format!("fw_attach_mid_migration_{tag}");
    // Audited: every interpolated value is a UUID this test minted.
    let create_fn = format!(
        "CREATE FUNCTION {func}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
           INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) \
           VALUES ('{q_engine}'::uuid, NEW.catalog_id) ON CONFLICT DO NOTHING; \
           RETURN NEW; END $$"
    );
    let create_trig = format!(
        "CREATE TRIGGER {trig} AFTER INSERT ON perm.grants FOR EACH ROW \
         WHEN (NEW.catalog_id = '{}'::uuid) EXECUTE FUNCTION {func}()",
        f.catalog_id
    );
    for sql in [&create_fn, &create_trig] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(&f.db.pool)
            .await
            .unwrap_or_else(|e| panic!("{e}\n{sql}"));
    }

    let (status, body) = migrate(&f, f.p, &[&a, &b], false).await;

    let drop_trig = format!("DROP TRIGGER {trig} ON perm.grants");
    let drop_fn = format!("DROP FUNCTION {func}()");
    for sql in [&drop_trig, &drop_fn] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(&f.db.pool)
            .await
            .unwrap_or_else(|e| panic!("{e}\n{sql}"));
    }

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["committed"], false, "{body}");
    let refusal = body["refusal"].as_str().expect("a refusal sentence");
    assert!(
        refusal.contains(&format!("recompiled profile(s) #{q},")) && refusal.contains("retry"),
        "{refusal}"
    );
    // Nothing written, the attachment included (it ran in the rolled-back tx).
    assert_eq!(grants_on_catalog(&f).await, 0);
    assert!(provenance(&f, f.p).await.is_empty());
    assert!(enabled(&f, a_id).await && enabled(&f, b_id).await);
    let attached: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM registry.engine_catalog_attachments WHERE engine_id = $1",
    )
    .bind(q_engine)
    .fetch_one(&f.db.pool)
    .await
    .expect("attachments");
    assert_eq!(
        attached, 0,
        "the trigger's attachment rolled back with the migration"
    );
}

// ── an external engine declares the plugin version it runs ───────────────

/// Turn the engine bound to `f.p` into an external one declaring `version` —
/// the shape migration 0184's `engines_external_shape` allows.
async fn make_bound_engine_external(f: &Fixture, version: &str) {
    let res = sqlx::query(
        "UPDATE engine_mgmt.engines \
            SET origin = 'external', external_host = 'trino.example.com', external_port = 8080, \
                ranger_plugin_version = $2, namespace = '', spec = '{}'::jsonb \
          WHERE authz_service_id = $1",
    )
    .bind(f.p)
    .bind(version)
    .execute(&f.db.pool)
    .await
    .expect("make the bound engine external");
    assert_eq!(res.rows_affected(), 1);
}

#[tokio::test]
async fn an_external_engine_declaring_an_unmeasured_plugin_version_refuses_the_migration() {
    let Some(f) = setup().await else { return };
    make_bound_engine_external(&f, "2.6.0").await;
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let refusal = body["refusal"].as_str().expect("refusal");
    assert!(
        refusal.contains("2.6.0") && refusal.contains("not been verified"),
        "the sentence must name the declared version: {refusal}"
    );
    assert_eq!(grants_on_catalog(&f).await, 0, "nothing is written");
    assert!(enabled(&f, id).await, "the original stays on");
}

/// User ruling 2026-10-12: "modelled" is a (type, version) PAIR. A Trino engine
/// declaring 2.8.0 is a real Ranger release but not one the evaluator was read
/// against for Trino, so the gate refuses rather than evaluating it with the
/// wrong semantics. (The StarRocks half is in `tests/routes.rs`' explain test.)
#[tokio::test]
async fn an_external_trino_declaring_2_8_0_refuses_the_migration_naming_the_pair() {
    let Some(f) = setup().await else { return };
    make_bound_engine_external(&f, "2.8.0").await;
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let (status, body) = migrate(&f, f.p, &[&name], true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let refusal = body["refusal"].as_str().expect("refusal");
    assert!(
        refusal.contains("2.8.0") && refusal.contains("trino") && refusal.contains("Trino@2.5.0"),
        "the sentence must name the pair and what is modelled: {refusal}"
    );
    assert_eq!(grants_on_catalog(&f).await, 0, "nothing is written");
    assert!(enabled(&f, id).await, "the original stays on");
}

/// The accept cell of the pair: Trino declaring 2.5.0 is proved on 2.5.0.
#[tokio::test]
async fn an_external_trino_declaring_2_5_0_is_proved_on_2_5_0() {
    let Some(f) = setup().await else { return };
    make_bound_engine_external(&f, "2.5.0").await;
    let name = unique("alice reads orders");
    insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let (status, body) = migrate(&f, f.p, &[&name], true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["profiles"][0]["plugin_versions"],
        json!(["ranger_2_5_0"])
    );
}

/// Point the provisioned engine bound to `f.p` at a Trino `imageVersion`.
async fn pin_bound_trino_image(f: &Fixture, image_version: &str) {
    let res = sqlx::query(
        "UPDATE engine_mgmt.engines SET spec = jsonb_build_object('imageVersion', $2::text) \
          WHERE authz_service_id = $1",
    )
    .bind(f.p)
    .bind(image_version)
    .execute(&f.db.pool)
    .await
    .expect("pin the bound engine's image");
    assert_eq!(res.rows_affected(), 1);
}

/// A provisioned engine's plugin comes from its exact image. Trino 483 ships
/// 2.8.0, a pair the evaluator is not yet read against, so the gate refuses.
/// TEMPORARY: Task 5 proves Trino@2.8.0 and inverts this test.
#[tokio::test]
async fn a_provisioned_trino_on_an_unmodelled_image_refuses_the_migration() {
    let Some(f) = setup().await else { return };
    pin_bound_trino_image(&f, "483").await;
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let (status, body) = migrate(&f, f.p, &[&name], false).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let refusal = body["refusal"].as_str().expect("refusal");
    assert!(
        refusal.contains("2.8.0") && refusal.contains("not been verified"),
        "{refusal}"
    );
    assert_eq!(grants_on_catalog(&f).await, 0, "nothing is written");
    assert!(enabled(&f, id).await, "the original stays on");
}

/// A floating tag cannot say which plugin it ships, so the gate refuses with
/// the fix (pin the image) instead of assuming the type's plugin.
#[tokio::test]
async fn a_provisioned_trino_on_a_floating_image_refuses_the_migration_with_the_fix() {
    let Some(f) = setup().await else { return };
    pin_bound_trino_image(&f, "latest").await;
    let name = unique("alice reads orders");
    let id = insert(
        &f,
        f.p,
        &name,
        "operator",
        access_policy(&f.catalog, "sales", "alice"),
    )
    .await;

    let (status, body) = migrate(&f, f.p, &[&name], true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let refusal = body["refusal"].as_str().expect("refusal");
    assert!(
        refusal.contains("trinodb/trino:latest") && refusal.contains("pin"),
        "{refusal}"
    );
    assert_eq!(grants_on_catalog(&f).await, 0, "nothing is written");
    assert!(enabled(&f, id).await, "the original stays on");
}
