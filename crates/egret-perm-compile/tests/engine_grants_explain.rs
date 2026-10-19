//! F11 Task 7: `POST /perm/explain/capability` (spec §5.4, E13) evaluates one
//! user and one capability on the artifact the engine's profile serves, with
//! `egret-ranger-eval`, and returns Undecidable as such.
//!
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grants_explain

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Env {
    db: Db,
    app: axum::Router,
    admin: String,
    nobody: String,
    policy: PolicyStore,
}

async fn insert_user(pool: &PgPool, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('local', $1, 'Engine explain test', $2) RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("egx")))
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("user")
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 10).await.expect("connect");
    db.migrate().await.expect("migrate");
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let admin_id = insert_user(&db.pool, None).await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(admin_id)
    .execute(&db.pool)
    .await
    .expect("platform_admin");
    let admin = sessions
        .create(egret_core::UserId(admin_id), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let nobody_id = insert_user(&db.pool, None).await;
    let nobody = sessions
        .create(egret_core::UserId(nobody_id), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("CedarPdp"),
    );
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("trino def");
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("starrocks def");
    egret_policy::seed_tag(&policy).await.expect("tag def");
    let app = perm_router().with_state(PermState {
        perm: PermStore::new(db.pool.clone()),
        policy: policy.clone(),
        sessions,
        pdp,
    });
    Some(Env {
        db,
        app,
        admin,
        nobody,
        policy,
    })
}

/// A profile of `ty` (a Trino one with a tag service) and an engine bound to it.
async fn bound(e: &Env, ty: &str) -> (i64, Uuid, Option<i64>) {
    let name = unique(&format!("egx-{ty}-"));
    let (config, tag) = if ty == "trino" {
        let tag_name = format!("{name}_tag");
        let tag = e
            .policy
            .create_service(&tag_name, "tag", &json!({}))
            .await
            .expect("tag service");
        (json!({ "tagService": tag_name }), Some(tag.id))
    } else {
        (json!({}), None)
    };
    let profile = e
        .policy
        .create_service(&name, ty, &config)
        .await
        .expect("profile")
        .id;
    // The default image of the type, so the profile's plugin version is known.
    let d = egret_perm::DEFAULT_ENGINE_IMAGES;
    let spec = if ty == "trino" {
        json!({ "imageVersion": d.trino_image_version })
    } else {
        json!({ "starrocksConfig": { "feImage": d.starrocks_fe_image } })
    };
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, $2, 'egret', $4, $3) RETURNING id",
    )
    .bind(unique("egx-eng-"))
    .bind(ty)
    .bind(profile)
    .bind(spec)
    .fetch_one(&e.db.pool)
    .await
    .expect("engine");
    (profile, engine, tag)
}

async fn call(
    e: &Env,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(v.to_string())
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

async fn grant(e: &Env, engine: Uuid, body: Value) -> String {
    let (status, resp) = call(
        e,
        "POST",
        &format!("/perm/engines/{engine}/grants"),
        &e.admin,
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{resp}");
    resp["id"].as_str().unwrap().to_string()
}

async fn explain(
    e: &Env,
    user: &str,
    engine: Uuid,
    capability: &str,
    values: Value,
) -> (StatusCode, Value) {
    call(
        e,
        "POST",
        "/perm/explain/capability",
        &e.admin,
        Some(json!({"subject": {"user": user}, "engineId": engine,
                    "capability": capability, "values": values})),
    )
    .await
}

async fn person(pool: &PgPool) -> String {
    let name = unique("egx-u-");
    insert_user(pool, Some(&name)).await;
    name
}

#[tokio::test]
async fn a_granted_session_property_is_allowed_for_the_grantee_and_denied_for_another() {
    let Some(e) = env().await else { return };
    let (_, engine, _) = bound(&e, "trino").await;
    let ua = person(&e.db.pool).await;
    let uc = person(&e.db.pool).await;
    let id = grant(
        &e,
        engine,
        json!({"capability": "trino.set_system_session_property", "values": ["query_max_memory"],
               "principals": [{"kind": "user", "ref": ua}]}),
    )
    .await;

    let (status, body) = explain(
        &e,
        &ua,
        engine,
        "trino.set_system_session_property",
        json!(["query_max_memory"]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "per_profile");
    assert_eq!(body["decision"]["kind"], "allowed", "{body}");
    assert_eq!(
        body["attribution"],
        json!({"kind": "engine_grant", "engine_grant_id": id})
    );
    assert_eq!(body["policyName"], json!(format!("perm-engine-{id}")));
    assert_eq!(body["tagResolution"], "not_applicable");

    let (_, body) = explain(
        &e,
        &uc,
        engine,
        "trino.set_system_session_property",
        json!(["query_max_memory"]),
    )
    .await;
    assert_eq!(body["decision"]["kind"], "denied", "{body}");
}

/// Spec C9: on a delegating profile the self-impersonation baseline puts
/// `{USER}` on `trinouser`, which the evaluator refuses to expand, so every
/// Trino impersonation question is Undecidable — even with a grant allowing it.
#[tokio::test]
async fn trino_impersonation_is_undecidable_on_a_delegating_profile() {
    let Some(e) = env().await else { return };
    let (profile, engine, _) = bound(&e, "trino").await;
    let ua = person(&e.db.pool).await;
    let ub = person(&e.db.pool).await;
    e.policy
        .upsert_infra_policy(
            profile,
            "egret-user-impersonate-self",
            &json!({"name": "egret-user-impersonate-self", "policyType": 0, "isEnabled": true,
                    "resources": {"trinouser": {"values": ["{USER}"]}},
                    "policyItems": [{"users": ["{USER}"], "groups": [], "roles": [],
                                     "accesses": [{"type": "impersonate", "isAllowed": true}]}]}),
        )
        .await
        .expect("baseline");
    grant(
        &e,
        engine,
        json!({"capability": "trino.impersonate_user", "values": [ub],
               "principals": [{"kind": "user", "ref": ua}]}),
    )
    .await;
    let (status, body) = explain(&e, &ua, engine, "trino.impersonate_user", json!([ub])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["decision"]["kind"], "undecidable", "{body}");
    let why = body["decision"]["why"].to_string();
    assert!(
        why.contains("resource_value_expression_marker") && why.contains("trinouser"),
        "{why}"
    );
}

#[tokio::test]
async fn a_starrocks_global_function_grant_is_explained() {
    let Some(e) = env().await else { return };
    let (_, engine, _) = bound(&e, "starrocks").await;
    let ua = person(&e.db.pool).await;
    let uc = person(&e.db.pool).await;
    grant(
        &e,
        engine,
        json!({"capability": "starrocks.use_global_function", "values": ["my_udf(INT)"],
               "principals": [{"kind": "user", "ref": ua}]}),
    )
    .await;
    let (_, body) = explain(
        &e,
        &ua,
        engine,
        "starrocks.use_global_function",
        json!(["my_udf(INT)"]),
    )
    .await;
    assert_eq!(body["decision"]["kind"], "allowed", "{body}");
    let (_, body) = explain(
        &e,
        &uc,
        engine,
        "starrocks.use_global_function",
        json!(["my_udf(INT)"]),
    )
    .await;
    assert_eq!(body["decision"]["kind"], "denied", "{body}");
}

/// A capability with a `catalog` level is asked with unresolved tags when the
/// download's tag block holds a policy: Undecidable, `declined`. With an empty
/// tag block it is decided.
#[tokio::test]
async fn a_catalog_capability_is_declined_when_the_tag_block_holds_a_policy() {
    let Some(e) = env().await else { return };
    let (_, engine, tag) = bound(&e, "trino").await;
    let ua = person(&e.db.pool).await;
    let values = json!(["ice", "system", "rollback_to_snapshot"]);

    let (status, body) = explain(&e, &ua, engine, "trino.execute_procedure", values.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["decision"]["kind"], "denied",
        "empty tag block: decided {body}"
    );
    assert_eq!(body["tagResolution"], "not_applicable");

    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) VALUES ($1, $2, $3)",
    )
    .bind(tag.expect("tag service"))
    .bind("egx tag policy")
    .bind(
        json!({"name": "egx tag policy", "policyType": 0, "isEnabled": true,
                 "resources": {"tag": {"values": ["pii"]}},
                 "policyItems": [{"users": ["someone"], "groups": [], "roles": [],
                                  "accesses": [{"type": "trino:select", "isAllowed": true}]}]}),
    )
    .execute(&e.db.pool)
    .await
    .expect("tag policy");
    let (_, body) = explain(&e, &ua, engine, "trino.execute_procedure", values).await;
    assert_eq!(body["decision"]["kind"], "undecidable", "{body}");
    assert_eq!(body["tagResolution"], "declined");
}

#[tokio::test]
async fn no_profile_bad_input_and_permission() {
    let Some(e) = env().await else { return };
    let ua = person(&e.db.pool).await;
    let np: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec) \
         VALUES ($1, 'trino', 'egret', '{}'::jsonb) RETURNING id",
    )
    .bind(unique("egx-np-"))
    .fetch_one(&e.db.pool)
    .await
    .expect("engine");
    let (status, body) = explain(
        &e,
        &ua,
        np,
        "trino.set_system_session_property",
        json!(["q"]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "no_profile");
    assert_eq!(body["decision"], Value::Null);

    let (status, _) = explain(&e, &ua, np, "trino.kill_any", json!([])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = explain(
        &e,
        &ua,
        np,
        "trino.set_system_session_property",
        json!(["*"]),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = explain(
        &e,
        &ua,
        Uuid::new_v4(),
        "trino.set_system_session_property",
        json!(["q"]),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = call(
        &e,
        "POST",
        "/perm/explain/capability",
        &e.nobody,
        Some(json!({"subject": {"user": ua}, "engineId": np,
                    "capability": "trino.set_system_session_property", "values": ["q"]})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
