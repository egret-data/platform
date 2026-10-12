//! `GET /perm/policies/services/{service_id}/classify` (SP4d-3 Task 5), driven
//! through the real router against the real schema.
//!
//! The fixture is one Trino profile (with one engine and one attached catalog)
//! plus the tag service it names, each holding a policy of every shape the
//! classifier has to get right: every owner, a migratable and a blocked
//! operator policy, a switched-off one, a catalog that is registered but NOT
//! served by this profile (the lookup's scoping), an unknown policy type, and
//! bodies stored in the wrong kind of service. Every read is scoped to the
//! fixture's own service ids — the test database is shared.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-perm-compile --test classify_route

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
    app: axum::Router,
    token: String,
    trino_id: i64,
    tag_service_id: i64,
    /// Attached to the profile's engine.
    served_catalog: String,
    /// Registered, but attached to no engine of this profile.
    unserved_catalog: String,
    /// A tag whose key and name are the same string.
    tag_same: String,
    /// `(key, name)` of a tag whose name differs from its key.
    tag_differs: (String, String),
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
    let tag_service = policy
        .create_service(&unique("cls_tag"), "tag", &json!({}))
        .await
        .expect("create tag service");
    let trino = policy
        .create_service(
            &unique("cls_trino"),
            "trino",
            &json!({ "tagService": tag_service.name }),
        )
        .await
        .expect("create trino service");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("cls-ws"))
            .bind("Classify WS")
            .fetch_one(&pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-classify-test','AKIATEST') RETURNING id",
    )
    .bind(unique("cls-sc"))
    .fetch_one(&pool)
    .await
    .expect("storage connection");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("cls-eng"))
    .bind(trino.id)
    .fetch_one(&pool)
    .await
    .expect("engine");

    let mut catalogs = Vec::new();
    for _ in 0..2 {
        let warehouse_id: Uuid = sqlx::query_scalar(
            "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
             VALUES ($1,$2,$3) RETURNING id",
        )
        .bind(unique("cls_wh"))
        .bind(ws_id)
        .bind(sc_id)
        .fetch_one(&pool)
        .await
        .expect("warehouse");
        let name = unique("cls_cat");
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
             VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                     jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
             RETURNING id",
        )
        .bind(&name)
        .bind(warehouse_id)
        .fetch_one(&pool)
        .await
        .expect("registry catalog");
        catalogs.push((name, id));
    }
    // Only the first is attached to this profile's engine.
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalogs[0].1)
    .execute(&pool)
    .await
    .expect("attach catalog");

    let tag_same = unique("phi");
    let tag_differs = (unique("pii"), unique("PII"));
    for (key, name) in [(&tag_same, &tag_same), (&tag_differs.0, &tag_differs.1)] {
        sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $2)")
            .bind(key)
            .bind(name)
            .execute(&pool)
            .await
            .expect("tag");
    }

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
            &format!("classify_{}@egret.local", Uuid::new_v4().simple()),
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
        policy,
        sessions,
        pdp,
    });
    Some(Fixture {
        db,
        app,
        token,
        trino_id: trino.id,
        tag_service_id: tag_service.id,
        served_catalog: catalogs[0].0.clone(),
        unserved_catalog: catalogs[1].0.clone(),
        tag_same,
        tag_differs,
    })
}

fn level(values: &[&str]) -> Value {
    json!({"values": values, "isExcludes": false, "isRecursive": false})
}

/// An access policy granting `accesses` to alice on `catalog.sales.orders.*`.
fn access_policy(catalog: &str, accesses: &[&str]) -> Value {
    json!({
        "resources": {
            "catalog": level(&[catalog]),
            "schema":  level(&["sales"]),
            "table":   level(&["orders"]),
            "column":  level(&["*"])
        },
        "policyItems": [{
            "users": ["alice"], "groups": [], "roles": [],
            "accesses": accesses.iter()
                .map(|a| json!({"type": a, "isAllowed": true}))
                .collect::<Vec<_>>()
        }],
        "denyPolicyItems": [],
        "isEnabled": true
    })
}

/// A tag-addressed masking policy for everyone.
fn tag_mask_policy(tag: &str) -> Value {
    json!({
        "policyType": 1,
        "isEnabled": true,
        "resources": {"tag": level(&[tag])},
        "dataMaskPolicyItems": [{
            "users": [], "groups": ["public"], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK_NULL"}
        }],
        "policyItems": [],
        "denyPolicyItems": []
    })
}

/// Insert a row directly, with its owner and switch — the guards the write
/// paths enforce are not what is under test here.
async fn insert(
    f: &Fixture,
    service_id: i64,
    name: &str,
    managed_by: &str,
    enabled: bool,
    body: Value,
) -> i64 {
    let mut body = body;
    body["name"] = json!(name);
    sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(service_id)
    .bind(name)
    .bind(body)
    .bind(enabled)
    .bind(managed_by)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert policy")
}

async fn classify(f: &Fixture, service_id: i64) -> (StatusCode, Value) {
    let request = HttpRequest::builder()
        .method("GET")
        .uri(format!("/perm/policies/services/{service_id}/classify"))
        .header("Cookie", format!("egret_session={}", f.token))
        .body(Body::empty())
        .unwrap();
    let resp = f.app.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

fn by_id(body: &Value, id: i64) -> &Value {
    body.as_array()
        .unwrap_or_else(|| panic!("not an array: {body}"))
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("policy {id} missing from {body}"))
}

fn blockers(p: &Value) -> Vec<String> {
    p["blockers"]
        .as_array()
        .unwrap_or_else(|| panic!("no blockers array in {p}"))
        .iter()
        .map(|b| b.as_str().expect("a blocker is a string").to_string())
        .collect()
}

fn assert_refused_with(p: &Value, needles: &[&str]) {
    assert_eq!(p["migratable"], false, "{p}");
    assert_eq!(p["proposed"], Value::Null, "{p}");
    let bs = blockers(p);
    assert!(
        bs.iter().any(|b| needles.iter().all(|n| b.contains(n))),
        "no blocker sentence contains all of {needles:?}: {bs:?}"
    );
}

#[tokio::test]
async fn every_policy_in_a_resource_profile_is_classified_in_id_order() {
    let Some(f) = setup().await else { return };
    let cat = f.served_catalog.clone();
    let p = f.trino_id;

    let infra = insert(
        &f,
        p,
        &format!("egret-admin - {}", unique("x")),
        "infra",
        true,
        access_policy(&cat, &["select"]),
    )
    .await;
    let compiler = insert(
        &f,
        p,
        &format!("perm-grant-{}", Uuid::new_v4()),
        "compiler",
        true,
        access_policy(&cat, &["select"]),
    )
    .await;
    let migratable = insert(
        &f,
        p,
        "alice reads orders",
        "operator",
        true,
        access_policy(&cat, &["select"]),
    )
    .await;
    let blocked = insert(
        &f,
        p,
        "alice alters orders",
        "operator",
        true,
        access_policy(&cat, &["select", "alter"]),
    )
    .await;
    // The body says enabled; the COLUMN says off, and the column is what the
    // engine's download reads.
    let switched_off = insert(
        &f,
        p,
        "alice used to read orders",
        "operator",
        false,
        access_policy(&cat, &["select"]),
    )
    .await;
    let unserved = insert(
        &f,
        p,
        "alice reads another profile's catalog",
        "operator",
        true,
        access_policy(&f.unserved_catalog, &["select"]),
    )
    .await;
    let mut unknown_type_body = access_policy(&cat, &["select"]);
    unknown_type_body["policyType"] = json!(7);
    let unknown_type = insert(
        &f,
        p,
        "a type egret has never seen",
        "operator",
        true,
        unknown_type_body,
    )
    .await;
    let mut no_column = access_policy(&cat, &["select"]);
    no_column["resources"]
        .as_object_mut()
        .unwrap()
        .remove("column");
    let missing_level = insert(&f, p, "saved by the editor", "operator", true, no_column).await;
    // The catalog is never omissible, and `*` names no catalog — so the
    // "set it to `*`" remedy the other levels get would be wrong here.
    let mut no_catalog = access_policy(&cat, &["select"]);
    no_catalog["resources"]
        .as_object_mut()
        .unwrap()
        .remove("catalog");
    let missing_catalog = insert(&f, p, "no catalog at all", "operator", true, no_catalog).await;
    let tag_here = insert(
        &f,
        p,
        "a tag mask in the resource profile",
        "operator",
        true,
        tag_mask_policy(&f.tag_same),
    )
    .await;

    let (status, body) = classify(&f, p).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Every policy of the profile, infra and compiler included, ordered by id.
    let ids: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    let mut want = vec![
        infra,
        compiler,
        migratable,
        blocked,
        switched_off,
        unserved,
        unknown_type,
        missing_level,
        missing_catalog,
        tag_here,
    ];
    want.sort_unstable();
    assert_eq!(ids, want, "{body}");

    let infra_row = by_id(&body, infra);
    assert_eq!(infra_row["managed_by"], "infra");
    assert_refused_with(infra_row, &["plumbing"]);
    let compiler_row = by_id(&body, compiler);
    assert_eq!(compiler_row["managed_by"], "compiler");
    assert_refused_with(compiler_row, &["permission compiler"]);

    let ok = by_id(&body, migratable);
    assert_eq!(ok["managed_by"], "operator");
    assert_eq!(ok["enabled"], true);
    assert_eq!(ok["migratable"], true, "{ok}");
    assert_eq!(ok["blockers"], json!([]), "{ok}");
    assert_eq!(ok["proposed"], json!({"grants": 1, "rules": 0}), "{ok}");

    // A sentence naming the value, not an enum name.
    let alter = by_id(&body, blocked);
    assert_refused_with(alter, &["`alter`", "Access type"]);
    assert!(
        blockers(alter)
            .iter()
            .all(|b| !b.contains("UnknownAccessType")),
        "{alter}"
    );

    let off = by_id(&body, switched_off);
    assert_eq!(off["enabled"], false, "{off}");
    assert_refused_with(off, &["switched off", "no migration on record"]);
    // Switched off by hand, never migrated: no marker.
    assert_eq!(off["migrated"], Value::Null, "{off}");
    assert_eq!(ok["migrated"], Value::Null, "{ok}");

    // Registered, but not served here: the scoped lookup refuses it.
    assert_refused_with(
        by_id(&body, unserved),
        &[&format!("`{}`", f.unserved_catalog), "serve"],
    );
    assert_refused_with(by_id(&body, unknown_type), &["type is 7"]);
    assert_refused_with(by_id(&body, missing_level), &["`column`", "editor", "`*`"]);
    let no_cat = by_id(&body, missing_catalog);
    assert_refused_with(no_cat, &["`catalog`", "served catalog"]);
    assert!(
        blockers(no_cat)
            .iter()
            .all(|b| !b.contains("Setting `catalog` to `*`")),
        "the catalog must not be told to become `*`: {no_cat}"
    );
    assert_refused_with(by_id(&body, tag_here), &["resource profile", "tag service"]);
}

#[tokio::test]
async fn a_tag_service_is_classified_as_a_tag_service() {
    let Some(f) = setup().await else { return };
    let t = f.tag_service_id;

    let same = insert(
        &f,
        t,
        "mask phi",
        "operator",
        true,
        tag_mask_policy(&f.tag_same),
    )
    .await;
    let differs = insert(
        &f,
        t,
        "mask pii",
        "operator",
        true,
        tag_mask_policy(&f.tag_differs.0),
    )
    .await;
    let path_here = insert(
        &f,
        t,
        "a grant in the tag service",
        "operator",
        true,
        access_policy(&f.served_catalog, &["select"]),
    )
    .await;

    let (status, body) = classify(&f, t).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 3, "{body}");

    let ok = by_id(&body, same);
    assert_eq!(ok["migratable"], true, "{ok}");
    assert_eq!(ok["proposed"], json!({"grants": 0, "rules": 1}), "{ok}");

    // The REAL tag lookup: the name differs from the key, and both are named.
    assert_refused_with(
        by_id(&body, differs),
        &[
            &format!("`{}`", f.tag_differs.0),
            &format!("`{}`", f.tag_differs.1),
        ],
    );
    // The remedy leads with keeping it hand-written, and a rename is offered
    // only with its side effect stated: every policy naming the old name stops
    // covering the tag's columns.
    let differs_row = by_id(&body, differs);
    assert_refused_with(
        differs_row,
        &[
            "Keep the policy hand-written",
            &format!("every tag policy that names `{}`", f.tag_differs.1),
            "lose their masks or filters",
        ],
    );
    assert!(
        blockers(differs_row)
            .iter()
            .all(|b| !b.contains("or rewrite the policy")),
        "rewriting the policy to the other name is not equivalent: {differs_row}"
    );
    // A path body in a tag service — and only that: the catalog it names is
    // served by the profile that links this tag service, so no second,
    // misleading "unregistered catalog" sentence rides along.
    let path_row = by_id(&body, path_here);
    assert_refused_with(path_row, &["tag service", "resource profile"]);
    assert_eq!(blockers(path_row).len(), 1, "{path_row}");
}

#[tokio::test]
async fn an_unknown_profile_is_a_404() {
    let Some(f) = setup().await else { return };
    let (status, body) = classify(&f, i64::MAX).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    // The handler's own 404, not the router's empty fallback for a path it
    // does not know.
    assert_eq!(body, json!({"error": "not found"}));
}

/// SP4d-3 Task 7: a migrated original is `enabled = false` exactly like one
/// switched off by hand. The classifier tells them apart by provenance, and
/// gives the migrated one ONE sentence — migrated, and how to undo it — not
/// the switched-off advice to switch it on, which would enforce it twice.
#[tokio::test]
async fn a_migrated_original_is_reported_as_migrated_not_as_switched_off() {
    let Some(f) = setup().await else { return };
    let cat = f.served_catalog.clone();
    let p = f.trino_id;
    let by_hand = insert(
        &f,
        p,
        "switched off by hand",
        "operator",
        false,
        access_policy(&cat, &["select"]),
    )
    .await;
    let migrated = insert(
        &f,
        p,
        "migrated away",
        "operator",
        false,
        access_policy(&cat, &["select"]),
    )
    .await;
    let grant_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO perm.migrated_from \
           (service_id, policy_id, policy_name, grant_ids, grant_hashes, original_hash, \
            probes_proved, migrated_by) \
         VALUES ($1, $2, 'migrated away', ARRAY[$3]::uuid[], ARRAY['h']::text[], 'h', 330, \
                 'steward@egret.local')",
    )
    .bind(p)
    .bind(migrated)
    .bind(grant_id)
    .execute(&f.db.pool)
    .await
    .expect("provenance");

    let (status, body) = classify(&f, p).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let m = by_id(&body, migrated);
    assert_eq!(m["enabled"], false, "{m}");
    assert_eq!(m["migratable"], false, "{m}");
    assert_eq!(m["migrated"]["grant_ids"], json!([grant_id]), "{m}");
    assert_eq!(m["migrated"]["probes_proved"], 330, "{m}");
    assert_eq!(m["migrated"]["migrated_by"], "steward@egret.local", "{m}");
    let bs = blockers(m);
    assert_eq!(bs.len(), 1, "{m}");
    assert!(
        bs[0].contains("was migrated") && bs[0].contains("undo the migration"),
        "{bs:?}"
    );
    assert!(!bs[0].contains("Switch it on"), "{bs:?}");

    let h = by_id(&body, by_hand);
    assert_eq!(h["migrated"], Value::Null, "{h}");
    assert_refused_with(h, &["switched off", "Switch it on first"]);
}

// ── SP4d-4 Task 11 (B1): stored rows that stop above the leaf are FLAGGED ───

/// An access policy for `accesses` that stops at `table` (no `column` key).
fn table_stopping_policy(catalog: &str, accesses: &[&str]) -> Value {
    let mut p = access_policy(catalog, accesses);
    p["resources"].as_object_mut().unwrap().remove("column");
    p
}

fn warnings_of(p: &Value) -> Vec<Value> {
    p["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("no warnings array in {p}"))
        .clone()
}

async fn profile_warnings(f: &Fixture, service_id: i64) -> (StatusCode, Value) {
    let request = HttpRequest::builder()
        .method("GET")
        .uri(format!("/perm/policies/services/{service_id}/warnings"))
        .header("Cookie", format!("egret_session={}", f.token))
        .body(Body::empty())
        .unwrap();
    let resp = f.app.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn select_above_column_is_warned_and_insert_above_column_is_not() {
    let Some(f) = setup().await else { return };
    let cat = f.served_catalog.clone();
    let reads = insert(
        &f,
        f.trino_id,
        "t11-select",
        "operator",
        true,
        table_stopping_policy(&cat, &["select"]),
    )
    .await;
    // `all` implies `select` on Trino (read from the service-def).
    let all = insert(
        &f,
        f.trino_id,
        "t11-all",
        "operator",
        true,
        table_stopping_policy(&cat, &["all"]),
    )
    .await;
    // The control: the same shape granting only `insert` reads no column anyway.
    let writes = insert(
        &f,
        f.trino_id,
        "t11-insert",
        "operator",
        true,
        table_stopping_policy(&cat, &["insert"]),
    )
    .await;
    // The control: the full chain is not above the column.
    let full = insert(
        &f,
        f.trino_id,
        "t11-full",
        "operator",
        true,
        access_policy(&cat, &["select"]),
    )
    .await;
    // A compiler-owned row is not examined.
    let compiled = insert(
        &f,
        f.trino_id,
        "t11-compiled",
        "compiler",
        true,
        table_stopping_policy(&cat, &["select"]),
    )
    .await;

    let (status, body) = classify(&f, f.trino_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for id in [reads, all] {
        let w = warnings_of(by_id(&body, id));
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(w[0]["kind"], "select_stops_above_column");
        assert_eq!(w[0]["level"], "table");
        let sentence = w[0]["sentence"].as_str().unwrap();
        assert!(
            sentence.contains("lets nobody read a column")
                && sentence.contains("EARLIER")
                && sentence.contains("not the more specific one")
                && sentence.contains("2026-10-11-sp4d-4-task1-levels-result.md"),
            "{sentence}"
        );
    }
    for id in [writes, full, compiled] {
        assert!(warnings_of(by_id(&body, id)).is_empty(), "id {id}: {body}");
    }

    // The profile-level list counts exactly the two warned policies.
    let (status, w) = profile_warnings(&f, f.trino_id).await;
    assert_eq!(status, StatusCode::OK, "{w}");
    let warned: Vec<i64> = w["policies"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| [reads, all].contains(&p["id"].as_i64().unwrap()))
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    assert_eq!(warned, vec![reads, all], "{w}");
    assert!(w["warningCount"].as_u64().unwrap() >= 2, "{w}");
}

#[tokio::test]
async fn a_stored_gap_row_is_warned() {
    let Some(f) = setup().await else { return };
    let cat = f.served_catalog.clone();
    let mut gap = access_policy(&cat, &["select"]);
    gap["resources"].as_object_mut().unwrap().remove("schema");
    let id = insert(&f, f.trino_id, "t11-gap", "operator", true, gap).await;

    let (status, body) = classify(&f, f.trino_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let w = warnings_of(by_id(&body, id));
    assert_eq!(
        w.len(),
        1,
        "one gap, one warning (not also a stop-above warning): {w:?}"
    );
    assert_eq!(w[0]["kind"], "level_gap");
    assert_eq!(w[0]["missing"], "schema");
    assert!(w[0]["sentence"]
        .as_str()
        .unwrap()
        .contains("decides nothing"));
}

#[tokio::test]
async fn an_existing_row_is_never_rewritten() {
    let Some(f) = setup().await else { return };
    let cat = f.served_catalog.clone();
    let id = insert(
        &f,
        f.trino_id,
        "t11-readonly",
        "operator",
        true,
        table_stopping_policy(&cat, &["select"]),
    )
    .await;
    let read = |f: &Fixture| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_as::<_, (Value, i64)>(
                "SELECT policy_jsonb, version FROM policy.authz_policies WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read row")
        }
    };
    let before = read(&f).await;
    let (status, body) = classify(&f, f.trino_id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(warnings_of(by_id(&body, id)).len(), 1);
    let (status, _) = profile_warnings(&f, f.trino_id).await;
    assert_eq!(status, StatusCode::OK);
    let after = read(&f).await;
    // `Value ==` is order-independent; never compare through `to_string()`.
    assert_eq!(before, after, "classifying must not write the row");
    assert!(after.0["resources"].get("column").is_none());
}

#[tokio::test]
async fn the_missing_level_blocker_states_the_measured_fact() {
    let Some(f) = setup().await else { return };
    let cat = f.served_catalog.clone();
    let id = insert(
        &f,
        f.trino_id,
        "t11-sentence",
        "operator",
        true,
        table_stopping_policy(&cat, &["select"]),
    )
    .await;
    let (_, body) = classify(&f, f.trino_id).await;
    let p = by_id(&body, id);
    assert_refused_with(
        p,
        &[
            "stops above `column`",
            "does not apply it to `column`-level requests",
            "policy's warnings",
            "EARLIER",
        ],
    );
    assert!(
        blockers(p)
            .iter()
            .all(|b| !b.contains("how most editor-saved policies look")),
        "the old claim is gone: {p}"
    );
}
