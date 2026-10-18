//! Governance events for the permission model's writes (DG #3, spec §3.2):
//! grants, rules, migrations and the manual compile each record one
//! `audit.audit_log` row in their own transaction; a preview, a refused
//! write, a dry run and an unproved migration record none.
//!
//! Driven through the real router with a real `CedarPdp` and a steward who
//! holds `policy:update` only. Every read is scoped to the fixture's own ids.
//! DB-gated; FAILS without a database unless `EGRET_ALLOW_DB_SKIP` is set.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
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
    steward: Uuid,
    /// A Trino profile (no tag service) bound to one engine serving `catalog`.
    p: i64,
    profile: String,
    catalog: String,
    catalog_id: Uuid,
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

    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'Gov') RETURNING id")
            .bind(unique("gov-ws"))
            .fetch_one(&pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-gov','AKIATEST') RETURNING id",
    )
    .bind(unique("gov-sc"))
    .fetch_one(&pool)
    .await
    .expect("storage connection");
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("gov_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .expect("warehouse");
    let catalog = unique("gov_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog)
    .bind(wh)
    .fetch_one(&pool)
    .await
    .expect("catalog");
    let profile = unique("gov_trino");
    let svc = policy
        .create_service(&profile, "trino", &json!({}))
        .await
        .expect("profile");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{\"imageVersion\":\"470\"}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("gov-eng"))
    .bind(svc.id)
    .fetch_one(&pool)
    .await
    .expect("engine");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine)
    .bind(catalog_id)
    .execute(&pool)
    .await
    .expect("attach");

    sqlx::query(
        "INSERT INTO users (provider, email, username) \
         SELECT 'local', n || '@perm-gov-events.egret.local', n FROM unnest(ARRAY['alice','bob']) n \
         ON CONFLICT DO NOTHING",
    )
    .execute(&pool)
    .await
    .expect("fixture usernames");

    // The steward: exactly `policy:update`.
    let steward: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'Steward') RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("gov_steward")))
    .fetch_one(&pool)
    .await
    .expect("steward");
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(unique("gov_steward_role"))
            .fetch_one(&pool)
            .await
            .expect("role");
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'policy:update')")
        .bind(role)
        .execute(&pool)
        .await
        .expect("permission");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user', $1, $2, 'platform')",
    )
    .bind(steward)
    .bind(role)
    .execute(&pool)
    .await
    .expect("access grant");

    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let token = sessions
        .create(egret_core::UserId(steward), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
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
        steward,
        p: svc.id,
        profile,
        catalog,
        catalog_id,
    })
}

async fn send(f: &Fixture, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={}", f.token));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = f.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

#[derive(Debug, sqlx::FromRow)]
struct Event {
    kind: String,
    actor_user_id: Option<Uuid>,
    action: String,
    resource_type: String,
    detail: Value,
}

async fn events(f: &Fixture, resource_id: &str) -> Vec<Event> {
    sqlx::query_as(
        "SELECT kind, actor_user_id, action, resource_type, detail FROM audit.audit_log \
          WHERE resource_id = $1 ORDER BY id",
    )
    .bind(resource_id)
    .fetch_all(&f.db.pool)
    .await
    .expect("events")
}

/// Governance events naming anything on this fixture's catalog or profile.
async fn fixture_event_count(f: &Fixture) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM audit.audit_log \
          WHERE kind = 'mutation' AND (detail::text LIKE '%' || $1 || '%' OR resource_id = $2)",
    )
    .bind(&f.catalog)
    .bind(f.p.to_string())
    .fetch_one(&f.db.pool)
    .await
    .expect("count")
}

fn grant(f: &Fixture, user: &str, column: Option<&str>) -> Value {
    json!({
        "effect": "allow",
        "actions": ["read"],
        "scope": { "catalog_id": f.catalog_id, "schema": "sales", "table": "orders", "column": column },
        "principals": { "include": [{ "kind": "user", "ref": user }], "exclude": [] }
    })
}

fn path_mask(f: &Fixture, name: &str, column: &str, mask: &str) -> Value {
    json!({
        "name": name,
        "kind": { "type": "mask", "mask_type": mask, "value_expr": null },
        "selector": { "type": "path", "catalog_id": f.catalog_id,
                      "schema": "sales", "table": "orders", "column": column },
        "exempt": { "include": [{ "kind": "user", "ref": "bob" }], "exclude": [] }
    })
}

fn assert_one(evs: &[Event], action: &str, resource_type: &str, f: &Fixture) -> Value {
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert_eq!(evs[0].kind, "mutation");
    assert_eq!(evs[0].action, action);
    assert_eq!(evs[0].resource_type, resource_type);
    assert_eq!(evs[0].actor_user_id, Some(f.steward));
    evs[0].detail.clone()
}

#[tokio::test]
async fn grant_create_update_delete_each_record_one_event_with_before_and_after() {
    let Some(f) = setup().await else { return };
    let (status, body) = send(&f, "POST", "/perm/grants", Some(grant(&f, "alice", None))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().expect("id").to_string();
    let created = json!({
        "catalog": f.catalog, "schema": "sales", "table": "orders", "column": null,
        "effect": "allow", "actions": ["read"], "include": ["user:alice"], "exclude": []
    });
    let d = assert_one(
        &events(&f, &id).await,
        "perm_grant:create",
        "perm_grant",
        &f,
    );
    assert_eq!(d, json!({ "before": null, "after": created }));

    let (status, body) = send(
        &f,
        "PATCH",
        &format!("/perm/grants/{id}"),
        Some(grant(&f, "bob", Some("id"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let evs = events(&f, &id).await;
    assert_eq!(evs.len(), 2, "{evs:?}");
    assert_eq!(evs[1].action, "perm_grant:update");
    let patched = json!({
        "catalog": f.catalog, "schema": "sales", "table": "orders", "column": "id",
        "effect": "allow", "actions": ["read"], "include": ["user:bob"], "exclude": []
    });
    assert_eq!(
        evs[1].detail,
        json!({ "before": created, "after": patched })
    );

    let (status, body) = send(&f, "DELETE", &format!("/perm/grants/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let evs = events(&f, &id).await;
    assert_eq!(evs.len(), 3, "{evs:?}");
    assert_eq!(evs[2].action, "perm_grant:delete");
    assert_eq!(evs[2].resource_type, "perm_grant");
    assert_eq!(evs[2].detail, json!({ "before": patched, "after": null }));
}

#[tokio::test]
async fn rule_create_update_delete_each_record_one_event_with_before_and_after() {
    let Some(f) = setup().await else { return };
    let name = unique("gov mask");
    let (status, body) = send(
        &f,
        "POST",
        "/perm/rules",
        Some(path_mask(&f, &name, "email", "MASK")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().expect("id").to_string();
    let created = json!({
        "name": name, "kind": "mask",
        "selector": format!("{}.sales.orders.email", f.catalog),
        "function": "MASK", "exempt": ["user:bob"], "exempt_exclude": []
    });
    let d = assert_one(&events(&f, &id).await, "perm_rule:create", "perm_rule", &f);
    assert_eq!(d, json!({ "before": null, "after": created }));

    let (status, body) = send(
        &f,
        "PATCH",
        &format!("/perm/rules/{id}"),
        Some(path_mask(&f, &name, "email", "MASK_HASH")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut patched = created.clone();
    patched["function"] = json!("MASK_HASH");
    let evs = events(&f, &id).await;
    assert_eq!(evs.len(), 2, "{evs:?}");
    assert_eq!(evs[1].action, "perm_rule:update");
    assert_eq!(
        evs[1].detail,
        json!({ "before": created, "after": patched })
    );

    let (status, body) = send(&f, "DELETE", &format!("/perm/rules/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let evs = events(&f, &id).await;
    assert_eq!(evs.len(), 3, "{evs:?}");
    assert_eq!(evs[2].action, "perm_rule:delete");
    assert_eq!(evs[2].detail, json!({ "before": patched, "after": null }));
}

/// Two mask rules whose model rows overlap on `orders.email` while the stored
/// download holds them apart, so every recompile of the profile is refused.
async fn overlapping_model(f: &Fixture) -> Uuid {
    let (status, body) = send(
        f,
        "POST",
        "/perm/rules",
        Some(path_mask(f, &unique("gov a"), "email", "MASK")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = send(
        f,
        "POST",
        "/perm/rules",
        Some(path_mask(f, &unique("gov b"), "phone", "MASK_NULL")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let b: Uuid = body["id"].as_str().expect("id").parse().unwrap();
    sqlx::query("UPDATE perm.rules SET column_name = 'email' WHERE id = $1")
        .bind(b)
        .execute(&f.db.pool)
        .await
        .expect("move the model row only");
    b
}

#[tokio::test]
async fn a_preview_and_a_refused_write_record_nothing() {
    let Some(f) = setup().await else { return };
    let before = fixture_event_count(&f).await;
    let (status, body) = send(
        &f,
        "POST",
        "/perm/preview",
        Some(json!({ "grant": grant(&f, "alice", None) })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        fixture_event_count(&f).await,
        before,
        "a preview rolls back"
    );

    overlapping_model(&f).await;
    let before = fixture_event_count(&f).await;
    let (status, body) = send(&f, "POST", "/perm/grants", Some(grant(&f, "alice", None))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rule_overlap", "{body}");
    assert_eq!(
        fixture_event_count(&f).await,
        before,
        "a refused write rolls back"
    );
}

fn level(values: &[&str]) -> Value {
    json!({"values": values, "isExcludes": false, "isRecursive": false})
}

fn access_policy(catalog: &str, user: &str) -> Value {
    json!({
        "resources": {
            "catalog": level(&[catalog]), "schema": level(&["sales"]),
            "table": level(&["orders"]), "column": level(&["*"])
        },
        "policyItems": [{
            "users": [user], "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}]
        }],
        "denyPolicyItems": [],
        "isEnabled": true
    })
}

async fn hand_written(f: &Fixture, name: &str, mut body: Value) -> i64 {
    body["name"] = json!(name);
    sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, enabled, managed_by) \
         VALUES ($1, $2, $3, true, 'operator') RETURNING id",
    )
    .bind(f.p)
    .bind(name)
    .bind(body)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert policy")
}

#[tokio::test]
async fn a_proved_migration_and_its_undo_each_record_one_event() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    hand_written(&f, &name, access_policy(&f.catalog, "alice")).await;
    let pid = f.p.to_string();

    let (status, body) = send(
        &f,
        "POST",
        &format!("/perm/policies/services/{}/migrate", f.p),
        Some(json!({"policy_names": [name], "dry_run": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        events(&f, &pid).await.is_empty(),
        "a dry run records nothing"
    );

    let (status, body) = send(
        &f,
        "POST",
        &format!("/perm/policies/services/{}/migrate", f.p),
        Some(json!({"policy_names": [name], "dry_run": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["committed"], true, "{body}");
    let grant_id = body["policies"][0]["grant_ids"][0]
        .as_str()
        .unwrap()
        .to_string();
    let d = assert_one(
        &events(&f, &pid).await,
        "policy_migration:apply",
        "policy_profile",
        &f,
    );
    assert_eq!(
        d,
        json!({
            "service": f.profile, "policies": [name],
            "created_grants": [grant_id], "created_rules": []
        })
    );

    let (status, body) = send(
        &f,
        "POST",
        &format!("/perm/policies/services/{}/migrate/undo", f.p),
        Some(json!({"policy_names": [name]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let evs = events(&f, &pid).await;
    assert_eq!(evs.len(), 2, "{evs:?}");
    assert_eq!(evs[1].action, "policy_migration:undo");
    assert_eq!(evs[1].resource_type, "policy_profile");
    assert_eq!(
        evs[1].detail,
        json!({ "service": f.profile, "policies": [name] })
    );
}

#[tokio::test]
async fn an_unproved_migration_records_nothing() {
    let Some(f) = setup().await else { return };
    let name = unique("alice reads orders");
    hand_written(&f, &name, access_policy(&f.catalog, "alice")).await;
    // A sibling the evaluator cannot decide: the gate fails closed (422).
    let mut scheduled = access_policy(&f.catalog, "bob");
    scheduled["validitySchedules"] =
        json!([{"startTime": "2026/01/01 00:00:00", "endTime": "2027/01/01 00:00:00"}]);
    hand_written(&f, &unique("bob, office hours"), scheduled).await;
    let before = fixture_event_count(&f).await;

    let (status, body) = send(
        &f,
        "POST",
        &format!("/perm/policies/services/{}/migrate", f.p),
        Some(json!({"policy_names": [name], "dry_run": false})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(fixture_event_count(&f).await, before);
}

#[tokio::test]
async fn a_manual_compile_records_its_counts_and_a_refused_one_its_error() {
    let Some(f) = setup().await else { return };
    let pid = f.p.to_string();
    let (status, body) = send(&f, "POST", "/perm/grants", Some(grant(&f, "alice", None))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = send(&f, "POST", &format!("/perm/compile/{}", f.p), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["services"], json!([f.p]), "{body}");
    let d = assert_one(
        &events(&f, &pid).await,
        "profile:compile",
        "policy_profile",
        &f,
    );
    assert_eq!(
        d,
        json!({
            "service": f.profile, "outcome": "ok",
            "created": body["created"], "updated": body["updated"], "deleted": body["deleted"],
            "unchanged": body["unchanged"], "skipped": 0
        })
    );
    let run: (String, Option<String>) =
        sqlx::query_as("SELECT outcome, error FROM perm.compile_runs WHERE service_id = $1")
            .bind(f.p)
            .fetch_one(&f.db.pool)
            .await
            .expect("a compile run");
    assert_eq!(run, ("ok".to_string(), None));

    overlapping_model(&f).await;
    let (status, body) = send(&f, "POST", &format!("/perm/compile/{}", f.p), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "rule_overlap", "{body}");
    let evs = events(&f, &pid).await;
    assert_eq!(evs.len(), 2, "{evs:?}");
    assert_eq!(evs[1].action, "profile:compile");
    assert_eq!(evs[1].detail["service"], json!(f.profile));
    assert_eq!(evs[1].detail["outcome"], "error");
    let error = evs[1].detail["error"]
        .as_str()
        .expect("error text")
        .to_string();
    let run: (String, Option<String>) =
        sqlx::query_as("SELECT outcome, error FROM perm.compile_runs WHERE service_id = $1")
            .bind(f.p)
            .fetch_one(&f.db.pool)
            .await
            .expect("a compile run");
    assert_eq!(run, ("error".to_string(), Some(error)));

    let (status, _) = send(&f, "POST", "/perm/compile/999999999", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
