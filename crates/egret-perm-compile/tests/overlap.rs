//! SP4d-4 Task 3: the compiler refuses mask / row-filter rules that may decide
//! one request differently — across the tag/resource boundary and through tag
//! assignments — names both RULES, and answers 409 with a structured body.
//!
//! The tag-ASSIGNMENT half (an assignment that completes a pair) is driven
//! through `egret-bi`'s `CatalogTagRepo` in `egret-bi/tests/catalog_tag_overlap.rs`,
//! because that is the crate that owns the write.
//!
//! # Why every test here holds one lock
//!
//! A Trino tag rule is catalog-UNSCOPED: every Trino profile in the database
//! compiles every tag rule, and a tag-rule write recompiles every
//! compiler-targeted profile. A pre-existing pair one test builds on purpose
//! (test 7) would therefore refuse another test's unrelated tag-rule write.
//! `egret-test-db` gives this binary a private database, and [`SERIAL`] runs
//! its tests one at a time, so each test sees only what it built.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!     cargo test -p egret-perm-compile --test overlap

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_perm::{PermStore, PrincipalSet, Rule, RuleKind, Scope, Selector};
use egret_perm_compile::{perm_router, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The editor's engine-binding seam; the mask/row-filter authoring paths do not
/// read it (they touch only `policy.authz_*`), so an empty reader is honest.
struct NoEngines;
#[async_trait::async_trait]
impl egret_policy::EngineBindingReader for NoEngines {
    async fn list_engine_bindings(
        &self,
    ) -> Result<Vec<egret_policy::EngineBinding>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn engine_catalog_names(
        &self,
        _e: Uuid,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_schemas(
        &self,
        _w: Uuid,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_tables(
        &self,
        _w: Uuid,
        _s: &str,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
    async fn warehouse_columns(
        &self,
        _w: Uuid,
        _s: &str,
        _t: &str,
    ) -> Result<Vec<String>, egret_policy::EngineReadError> {
        Ok(vec![])
    }
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Env {
    db: Db,
    perm: PermStore,
    policy: PolicyStore,
    app: axum::Router,
    token: String,
}

/// One profile with one engine bound to it and one registry catalog attached.
struct Profile {
    service_id: i64,
    warehouse_id: Uuid,
    catalog_id: Uuid,
    catalog: String,
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let perm = PermStore::new(db.pool.clone());
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
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
    let uid = UserRepo::new(db.clone())
        .insert_local(
            &format!("overlap_{}@egret.local", Uuid::new_v4().simple()),
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
    .execute(&db.pool)
    .await
    .expect("grant platform_admin");
    let token = sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    // The policy editor too (fix round 1, review C1): the hand-written half of
    // a compiled × hand-written pair is authored through `POST /policies`.
    let admin = egret_policy::policy_admin_router().with_state(egret_policy::PolicyAdminState {
        store: policy.clone(),
        engines: Arc::new(NoEngines),
        users: UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        sessions: sessions.clone(),
        engine_egret_url: "http://egret:8080".into(),
        pdp: pdp.clone(),
    });
    let app = perm_router()
        .with_state(PermState {
            perm: perm.clone(),
            policy: policy.clone(),
            sessions,
            pdp,
        })
        .merge(admin);
    Some(Env {
        db,
        perm,
        policy,
        app,
        token,
    })
}

/// `engine` is `"trino"` (with a tag service) or `"starrocks"`.
async fn profile(e: &Env, engine: &str) -> Profile {
    let pool = &e.db.pool;
    let config = if engine == "trino" {
        let tag = unique("ovl_tag");
        e.policy
            .create_service(&tag, "tag", &json!({}))
            .await
            .expect("tag service");
        json!({ "tagService": tag })
    } else {
        json!({})
    };
    let svc = e
        .policy
        .create_service(&unique(&format!("ovl_{engine}")), engine, &config)
        .await
        .expect("profile");
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name) VALUES ($1,'Overlap') RETURNING id",
    )
    .bind(unique("ovl-ws"))
    .fetch_one(pool)
    .await
    .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-overlap','AKIATEST') RETURNING id",
    )
    .bind(unique("ovl-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("ovl_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,$2,'egret','{}'::jsonb,$3) RETURNING id",
    )
    .bind(unique("ovl-eng"))
    .bind(engine)
    .bind(svc.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog = unique("ovl_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog)
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach");
    Profile {
        service_id: svc.id,
        warehouse_id,
        catalog_id,
        catalog,
    }
}

/// A tag (`key` and `name` both `label-<hex>`) assigned on `sales.orders.<column>`
/// of `p`'s warehouse, straight into the table (the assignment route is
/// `egret-bi`'s). Returns the tag key.
async fn tag_on(e: &Env, p: &Profile, label: &str, columns: &[&str]) -> String {
    let key = format!("{label}-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
        .bind(&key)
        .fetch_one(&e.db.pool)
        .await
        .expect("tag");
    for column in columns {
        sqlx::query(
            "INSERT INTO catalog_tag_assignments \
                 (tag_id, warehouse_id, schema_name, table_name, column_name) \
             VALUES ($1, $2, 'sales', 'orders', $3)",
        )
        .bind(id)
        .bind(p.warehouse_id)
        .bind(column)
        .execute(&e.db.pool)
        .await
        .expect("assignment");
    }
    key
}

async fn send(e: &Env, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={}", e.token));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = e.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

fn exempt(users: &[&str]) -> Value {
    json!({
        "include": users.iter().map(|u| json!({"kind": "user", "ref": u})).collect::<Vec<_>>(),
        "exclude": []
    })
}

fn path_mask(name: &str, p: &Profile, table: &str, column: &str, mask: &str) -> Value {
    json!({
        "name": name,
        "kind": { "type": "mask", "mask_type": mask, "value_expr": null },
        "selector": { "type": "path", "catalog_id": p.catalog_id,
                      "schema": "sales", "table": table, "column": column },
        "exempt": exempt(&[])
    })
}

fn tag_mask(name: &str, key: &str, mask: &str, exempted: &[&str]) -> Value {
    json!({
        "name": name,
        "kind": { "type": "mask", "mask_type": mask, "value_expr": null },
        "selector": { "type": "tag", "key": key },
        "exempt": exempt(exempted)
    })
}

/// `perm.rules` names on `p`'s catalog (path rules) — a scoped population.
async fn path_rules(e: &Env, p: &Profile) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM perm.rules WHERE catalog_id = $1 ORDER BY name")
        .bind(p.catalog_id)
        .fetch_all(&e.db.pool)
        .await
        .expect("rules")
}

async fn rule_named(e: &Env, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM perm.rules WHERE name = $1")
        .bind(name)
        .fetch_one(&e.db.pool)
        .await
        .expect("count")
}

/// Assert `body` is the structured overlap 409 naming `names`.
fn assert_overlap(status: StatusCode, body: &Value, names: &[&str]) {
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rule_overlap", "{body}");
    let rules = body["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 2, "{body}");
    let error = body["error"].as_str().expect("error");
    for n in names {
        assert!(
            rules.iter().any(|r| r["name"] == *n),
            "rules must name {n:?}: {body}"
        );
        assert!(error.contains(&format!("rule {n:?}")), "{error}");
    }
    for banned in ["random", "undefined", "may change between queries"] {
        assert!(!error.contains(banned), "{banned}: {error}");
    }
}

/// Test 1. The second rule on an intersecting path is refused, naming the
/// first by NAME, and nothing is stored.
#[tokio::test]
async fn a_second_rule_on_an_intersecting_path_is_409_naming_the_first() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "trino").await;
    let (a_name, b_name) = (unique("mask emails"), unique("null orders email"));
    let (status, a) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&a_name, &p, "*", "email", "MASK")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{a}");
    let a_id = a["id"].as_str().expect("id").to_string();

    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&b_name, &p, "orders", "email", "MASK_NULL")),
    )
    .await;
    assert_overlap(status, &body, &[&a_name, &b_name]);
    assert!(
        body["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == a_id.as_str()),
        "rules carries A's id: {body}"
    );
    assert_eq!(body["kind"], "mask");
    assert_eq!(
        path_rules(&e, &p).await,
        vec![a_name.clone()],
        "only A is stored"
    );
}

/// Test 2: after the refusal, a PATCH that makes the two AGREE lets the second
/// rule in (an agreeing tie is admitted). On StarRocks, with a `*` column —
/// the other way a path rule is broader than another.
#[tokio::test]
async fn a_patch_that_makes_two_rules_agree_succeeds() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "starrocks").await;
    let (a_name, b_name) = (unique("all orders cols"), unique("orders email"));
    let (status, a) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&a_name, &p, "orders", "*", "MASK")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{a}");
    let a_id = a["id"].as_str().expect("id").to_string();
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&b_name, &p, "orders", "email", "MASK_NULL")),
    )
    .await;
    assert_overlap(status, &body, &[&a_name, &b_name]);
    // Today the broader rule is served; the sentence says so.
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains(&format!("Today Ranger serves rule {a_name:?}")),
        "{body}"
    );
    let (status, body) = send(
        &e,
        "PATCH",
        &format!("/perm/rules/{a_id}"),
        Some(path_mask(&a_name, &p, "orders", "*", "MASK_NULL")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&b_name, &p, "orders", "email", "MASK_NULL")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(rule_named(&e, &b_name).await, 1);
}

/// Test 5, rule-after-assignment (the assignment-after-rule order is in
/// `egret-bi`): a path mask on a tagged column, then a tag mask on that tag.
#[tokio::test]
async fn a_tag_rule_and_a_path_rule_on_one_tagged_column_are_409() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "trino").await;
    let pii = tag_on(&e, &p, "pii", &["email"]).await;
    let other = tag_on(&e, &p, "other", &["sku"]).await;
    let path_name = unique("null email");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&path_name, &p, "orders", "email", "MASK_NULL")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // Accept-control: a tag mask on a tag that is not on `email`.
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(tag_mask(&unique("mask sku"), &other, "MASK", &[])),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let tag_name = unique("mask pii");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(tag_mask(&tag_name, &pii, "MASK", &[])),
    )
    .await;
    assert_overlap(status, &body, &[&tag_name, &path_name]);
    let error = body["error"].as_str().unwrap();
    assert!(
        error.contains(&format!("{}.sales.orders.email", p.catalog))
            && error.contains("tag phase")
            && error.contains(&format!("today Ranger serves rule {tag_name:?}")),
        "names the column and the winner: {error}"
    );
    assert_eq!(
        rule_named(&e, &tag_name).await,
        0,
        "the tag rule was not stored"
    );
}

/// Test 6: on StarRocks a tag rule compiles to per-column PATH policies
/// (`rules.rs`), so the same pair is two path policies in one download —
/// caught by the within-download rule alone.
#[tokio::test]
async fn the_same_pair_on_starrocks_is_caught_by_the_path_rule_alone() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "starrocks").await;
    let pii = tag_on(&e, &p, "pii", &["email"]).await;
    let path_name = unique("null email");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&path_name, &p, "orders", "email", "MASK_NULL")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let tag_name = unique("mask pii");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(tag_mask(&tag_name, &pii, "MASK", &[])),
    )
    .await;
    assert_overlap(status, &body, &[&tag_name, &path_name]);
    assert!(
        body["error"].as_str().unwrap().contains("column=[email]"),
        "the within-download sentence: {body}"
    );
}

/// Test 7: a pair stored by a write that bypassed the routes wedges every
/// recompile of the profile (an unrelated grant), and the write that removes
/// it (deleting one rule) passes.
#[tokio::test]
async fn a_preexisting_pair_wedges_unrelated_writes_and_its_removal_is_allowed() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "starrocks").await;
    let rule = |name: &str, mask: &str| Rule {
        id: Uuid::nil(),
        name: name.to_string(),
        kind: RuleKind::Mask {
            mask_type: mask.to_string(),
            value_expr: None,
        },
        selector: Selector::Path(Scope {
            catalog_id: p.catalog_id,
            schema: Some("sales".into()),
            table: Some("orders".into()),
            column: Some("email".into()),
        }),
        exempt: PrincipalSet::default(),
    };
    let (a_name, b_name) = (unique("stored a"), unique("stored b"));
    let _a = e.perm.create_rule(&rule(&a_name, "MASK")).await.expect("a");
    let b = e
        .perm
        .create_rule(&rule(&b_name, "MASK_NULL"))
        .await
        .expect("b");

    let grant = json!({
        "effect": "allow",
        "actions": ["read"],
        "scope": { "catalog_id": p.catalog_id, "schema": "sales", "table": "orders", "column": null },
        "principals": { "include": [{ "kind": "user", "ref": "analyst" }], "exclude": [] }
    });
    let (status, body) = send(&e, "POST", "/perm/grants", Some(grant)).await;
    assert_overlap(status, &body, &[&a_name, &b_name]);
    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.grants WHERE catalog_id = $1")
        .bind(p.catalog_id)
        .fetch_one(&e.db.pool)
        .await
        .unwrap();
    assert_eq!(grants, 0, "the wedged grant write stored nothing");

    let (status, body) = send(&e, "DELETE", &format!("/perm/rules/{b}"), None).await;
    assert!(
        status.is_success(),
        "deleting one half removes the pair, so its recompile passes: {status} {body}"
    );
    assert_eq!(path_rules(&e, &p).await, vec![a_name]);
}

/// Test 8: a migration whose produced rule overlaps an existing rule answers
/// the overlap refusal and writes nothing.
#[tokio::test]
async fn migration_of_a_policy_that_would_overlap_is_refused() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "trino").await;
    let existing = unique("existing ssn mask");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&existing, &p, "orders", "ssn", "MASK")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let lvl = |v: &str| json!({"values": [v], "isExcludes": false, "isRecursive": false});
    let original = unique("hand ssn mask");
    let body = json!({
        "name": original, "policyType": 1, "isEnabled": true,
        "resources": {"catalog": lvl(&p.catalog), "schema": lvl("sales"),
                      "table": lvl("orders"), "column": lvl("ssn")},
        "dataMaskPolicyItems": [{
            "users": [], "groups": ["public"], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK_NULL"}
        }],
        "policyItems": [], "denyPolicyItems": []
    });
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, true, 'operator') RETURNING id",
    )
    .bind(p.service_id)
    .bind(&original)
    .bind(&body)
    .fetch_one(&e.db.pool)
    .await
    .expect("insert original");

    let rules_before = path_rules(&e, &p).await;
    let (status, body) = send(
        &e,
        "POST",
        &format!("/perm/policies/services/{}/migrate", p.service_id),
        Some(json!({"policy_names": [original], "dry_run": false})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rule_overlap", "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains(&format!("rule {existing:?}")),
        "{body}"
    );
    assert_eq!(path_rules(&e, &p).await, rules_before, "no rule written");
    let migrated: i64 =
        sqlx::query_scalar("SELECT count(*) FROM perm.migrated_from WHERE policy_id = $1")
            .bind(id)
            .fetch_one(&e.db.pool)
            .await
            .unwrap();
    assert_eq!(migrated, 0, "no provenance written");
    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM policy.authz_policies WHERE id = $1")
            .bind(id)
            .fetch_one(&e.db.pool)
            .await
            .unwrap();
    assert!(enabled, "the original is still on");
    // The original beside `existing` is a stored compiled × hand-written pair,
    // which (fix round 1, C1) wedges every recompile of this profile — and a
    // tag rule in another test recompiles every Trino profile. Remove it.
    e.policy.delete_policy(id).await.expect("cleanup");
}

/// Test 9: tag ↔ path with the SAME mask but different exemptions — different
/// decisive arrays, so a user exempt from one and not the other is decided by
/// a tie-break.
#[tokio::test]
async fn different_exemptions_on_one_tagged_column_are_409() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "trino").await;
    let pii = tag_on(&e, &p, "pii", &["email"]).await;
    let path_name = unique("mask email");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&path_name, &p, "orders", "email", "MASK")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let tag_name = unique("mask pii but alice");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(tag_mask(&tag_name, &pii, "MASK", &["alice"])),
    )
    .await;
    assert_overlap(status, &body, &[&tag_name, &path_name]);
}

// ── fix round 1, review C1: compiled × hand-written ─────────────────────────

/// A hand-written path mask, as the policy editor's `POST /policies` takes it.
fn hand_path_mask(name: &str, profile: &str, p: &Profile, column: &str, mask: &str) -> Value {
    json!({
        "kind": "masking", "name": name, "service": profile, "isEnabled": true,
        "resources": {"catalog": {"values": [p.catalog]}, "schema": {"values": ["sales"]},
                      "table": {"values": ["orders"]}, "column": {"values": [column]}},
        "items": [{"groups": ["public"], "maskType": mask}]
    })
}

async fn profile_name(e: &Env, p: &Profile) -> String {
    sqlx::query_scalar("SELECT name FROM policy.authz_services WHERE id = $1")
        .bind(p.service_id)
        .fetch_one(&e.db.pool)
        .await
        .expect("profile name")
}

/// Assert the structured 409 names hand-written `policy` (no rule id) and the
/// compiled `rule` (with an id), each by the label the steward edits.
fn assert_mixed_overlap(status: StatusCode, body: &Value, policy: &str, rule: &str) {
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rule_overlap", "{body}");
    let rules = body["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), 2, "{body}");
    assert!(
        rules
            .iter()
            .any(|r| r["name"] == policy && r["id"].is_null()),
        "the hand-written member, with no rule id: {body}"
    );
    assert!(
        rules
            .iter()
            .any(|r| r["name"] == rule && r["id"].is_string()),
        "the compiled member, with its rule id: {body}"
    );
    let error = body["error"].as_str().expect("error");
    assert!(
        error.contains(&format!("policy {policy:?}")) && error.contains(&format!("rule {rule:?}")),
        "{error}"
    );
    for banned in ["random", "undefined", "may change between queries"] {
        assert!(!error.contains(banned), "{banned}: {error}");
    }
}

/// C1, path: a hand-written mask stored through the editor, then a path rule
/// on the same column that masks differently. The rule write completes a
/// compiled × hand-written pair, so its recompile refuses it, naming both.
#[tokio::test]
async fn a_rule_completing_a_pair_with_a_hand_written_mask_is_409_naming_both() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "trino").await;
    let svc = profile_name(&e, &p).await;
    let hand = unique("hand email mask");
    let (status, body) = send(
        &e,
        "POST",
        "/policies",
        Some(hand_path_mask(&hand, &svc, &p, "email", "MASK")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the editor stores H: {body}");

    let rule = unique("null email");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&rule, &p, "orders", "email", "MASK_NULL")),
    )
    .await;
    assert_mixed_overlap(status, &body, &hand, &rule);
    assert_eq!(body["kind"], "mask");
    assert!(path_rules(&e, &p).await.is_empty(), "no rule stored");
}

/// C1, tag: a hand-written tag mask in a manual profile's tag service, the
/// column carrying the tag, then a compiled tag rule on that tag that masks
/// differently. Tag rules compile into every engine-bound Trino profile's tag
/// service, so the rule lands beside H — refused.
#[tokio::test]
async fn a_tag_rule_completing_a_pair_with_a_hand_written_tag_mask_is_409() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "trino").await;
    let svc = profile_name(&e, &p).await;
    let pii = tag_on(&e, &p, "pii", &["email"]).await;
    let hand = unique("hand pii mask");
    let (status, body) = send(
        &e,
        "POST",
        "/policies",
        Some(json!({
            "kind": "tag-masking", "name": hand, "service": svc, "tag": pii,
            "isEnabled": true,
            "items": [{"groups": ["public"], "maskType": "MASK"}]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the editor stores H: {body}");
    let hand_id = body["id"].as_i64().expect("id");

    let rule = unique("null pii");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(tag_mask(&rule, &pii, "MASK_NULL", &[])),
    )
    .await;
    assert_mixed_overlap(status, &body, &hand, &rule);
    assert_eq!(
        rule_named(&e, &rule).await,
        0,
        "the tag rule was not stored"
    );
    // Leave no hand-written tag mask behind for this binary's other tag rules.
    e.policy.delete_policy(hand_id).await.expect("cleanup");
}

/// C1, migration: migrating X while ANOTHER hand-written mask Y, still on,
/// masks the same column differently. X's compiled twin pairs with Y — refused,
/// and nothing is written (no rule, no provenance, X still on). X itself is
/// switched off before the rule write, so the refusal names Y, not X.
#[tokio::test]
async fn a_migration_overlapping_another_hand_written_policy_is_refused() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "trino").await;
    let lvl = |v: &str| json!({"values": [v], "isExcludes": false, "isRecursive": false});
    let raw = |name: &str, mask: &str| {
        json!({
            "name": name, "policyType": 1, "isEnabled": true,
            "resources": {"catalog": lvl(&p.catalog), "schema": lvl("sales"),
                          "table": lvl("orders"), "column": lvl("ssn")},
            "dataMaskPolicyItems": [{
                "users": [], "groups": ["public"], "roles": [],
                "accesses": [{"type": "select", "isAllowed": true}],
                "dataMaskInfo": {"dataMaskType": mask}
            }],
            "policyItems": [], "denyPolicyItems": []
        })
    };
    // A stored hand × hand pair (the editor would refuse it), so by raw SQL.
    let (x, y) = (unique("hand ssn x"), unique("hand ssn y"));
    let mut ids = Vec::new();
    for (name, mask) in [(&x, "MASK_NULL"), (&y, "MASK")] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
             VALUES ($1, $2, $3, true, 'operator') RETURNING id",
        )
        .bind(p.service_id)
        .bind(name)
        .bind(raw(name, mask))
        .fetch_one(&e.db.pool)
        .await
        .expect("insert hand-written");
        ids.push(id);
    }

    let (status, body) = send(
        &e,
        "POST",
        &format!("/perm/policies/services/{}/migrate", p.service_id),
        Some(json!({"policy_names": [x], "dry_run": false})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rule_overlap", "{body}");
    let error = body["error"].as_str().unwrap();
    assert!(error.contains(&format!("policy {y:?}")), "names Y: {error}");
    assert!(
        !error.contains(&format!("policy {x:?}")),
        "the original is off, so it is not a member: {error}"
    );
    assert!(path_rules(&e, &p).await.is_empty(), "no rule written");
    let migrated: i64 =
        sqlx::query_scalar("SELECT count(*) FROM perm.migrated_from WHERE policy_id = $1")
            .bind(ids[0])
            .fetch_one(&e.db.pool)
            .await
            .unwrap();
    assert_eq!(migrated, 0, "no provenance written");
    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM policy.authz_policies WHERE id = $1")
            .bind(ids[0])
            .fetch_one(&e.db.pool)
            .await
            .unwrap();
    assert!(enabled, "the original is still on");
    for id in ids {
        e.policy.delete_policy(id).await.expect("cleanup");
    }
}

/// C1's two allow cells: a rule that decides a hand-written mask's column with
/// IDENTICAL items (an agreeing tie, admitted as everywhere), and a differing
/// rule on a column the hand-written mask does not reach.
#[tokio::test]
async fn agreeing_or_disjoint_rules_beside_a_hand_written_mask_are_admitted() {
    let _serial = SERIAL.lock().await;
    let Some(e) = env().await else { return };
    let p = profile(&e, "trino").await;
    let svc = profile_name(&e, &p).await;
    let hand = unique("hand email mask");
    let (status, body) = send(
        &e,
        "POST",
        "/policies",
        Some(hand_path_mask(&hand, &svc, &p, "email", "MASK")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let agreeing = unique("mask email too");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&agreeing, &p, "orders", "email", "MASK")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "identical items: {body}");

    let disjoint = unique("null sku");
    let (status, body) = send(
        &e,
        "POST",
        "/perm/rules",
        Some(path_mask(&disjoint, &p, "orders", "sku", "MASK_NULL")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "another column: {body}");
    let mut want = vec![agreeing, disjoint];
    want.sort();
    assert_eq!(path_rules(&e, &p).await, want);
}
