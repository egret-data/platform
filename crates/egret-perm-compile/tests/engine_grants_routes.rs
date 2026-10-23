//! F11 Task 6: the engine-grant routes (spec §5) with a real `CedarPdp`:
//! permissions, the catalogue, the per-engine listing with reach and managed
//! baselines, every 400/404/409 of §5.5, patch and delete, and the two locks
//! a write must hold (the target's user lock, the engine row `FOR SHARE`).
//!
//! Every assertion is scoped to this fixture's own rows: tests in one binary
//! share a database.
//!
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grants_routes

use std::sync::Arc;
use std::time::Duration;

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
    reader: String,
    writer: String,
    nobody: String,
    policy: PolicyStore,
}

async fn insert_user(pool: &PgPool, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('local', $1, 'Engine route test', $2) RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("egrt")))
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("user")
}

async fn session(db: &Db, sessions: &SessionService, permissions: &[&str]) -> String {
    let uid = insert_user(&db.pool, None).await;
    if !permissions.is_empty() {
        let role_id: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id",
        )
        .bind(unique("egrt-role-"))
        .fetch_one(&db.pool)
        .await
        .expect("role");
        for perm in permissions {
            sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
                .bind(role_id)
                .bind(perm)
                .execute(&db.pool)
                .await
                .expect("permission");
        }
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
             VALUES ('user', $1, $2, 'platform')",
        )
        .bind(uid)
        .bind(role_id)
        .execute(&db.pool)
        .await
        .expect("access grant");
    }
    sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 20).await.expect("connect");
    db.migrate().await.expect("migrate");
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let reader = session(&db, &sessions, &["policy:read"]).await;
    let writer = session(&db, &sessions, &["policy:read", "policy:update"]).await;
    let nobody = session(&db, &sessions, &[]).await;
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
    // Built AFTER every permission above: the PDP loads them once.
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
        reader,
        writer,
        nobody,
        policy,
    })
}

/// A profile of `ty` and an engine bound to it; with `catalog`, one attached
/// registry catalog of that name.
struct Bound {
    profile: i64,
    profile_name: String,
    engine: Uuid,
}

async fn profile(e: &Env, ty: &str) -> (i64, String) {
    let name = unique(&format!("egrt-{ty}-"));
    let id = e
        .policy
        .create_service(&name, ty, &json!({}))
        .await
        .expect("profile")
        .id;
    (id, name)
}

async fn engine(pool: &PgPool, ty: &str, name: &str, profile: Option<i64>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, $2, 'egret', '{}'::jsonb, $3) RETURNING id",
    )
    .bind(name)
    .bind(ty)
    .bind(profile)
    .fetch_one(pool)
    .await
    .expect("engine")
}

async fn bound(e: &Env, ty: &str) -> Bound {
    let (profile, profile_name) = profile(e, ty).await;
    let engine = engine(&e.db.pool, ty, &unique("egrt-b-"), Some(profile)).await;
    Bound {
        profile,
        profile_name,
        engine,
    }
}

async fn attach_catalog(pool: &PgPool, engine: Uuid) -> String {
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,'W') RETURNING id")
            .bind(unique("egrt-ws-"))
            .fetch_one(pool)
            .await
            .expect("ws");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egrt','AKIATEST') RETURNING id",
    )
    .bind(unique("egrt-sc-"))
    .fetch_one(pool)
    .await
    .expect("sc");
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("egrt_wh_"))
    .bind(ws)
    .bind(sc)
    .fetch_one(pool)
    .await
    .expect("wh");
    let name = unique("egrt_cat_");
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&name)
    .bind(wh)
    .fetch_one(pool)
    .await
    .expect("catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine)
    .bind(cat)
    .execute(pool)
    .await
    .expect("attach");
    name
}

async fn call(
    e: &Env,
    method: &'static str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call_app(
        e.app.clone(),
        method,
        uri.to_string(),
        token.to_string(),
        body,
    )
    .await
}

async fn call_app(
    app: axum::Router,
    method: &'static str,
    uri: String,
    token: String,
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
    let resp = app.oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

fn sys_grant(user: &str) -> Value {
    json!({
        "capability": "trino.set_system_session_property",
        "values": ["query_max_memory"],
        "principals": [{"kind": "user", "ref": user}],
    })
}

async fn compiled(pool: &PgPool, service: i64, id: &Value) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service)
    .bind(format!("perm-engine-{}", id.as_str().unwrap()))
    .fetch_optional(pool)
    .await
    .expect("read")
}

async fn person(pool: &PgPool) -> (Uuid, String) {
    let name = unique("egrt-u-");
    (insert_user(pool, Some(&name)).await, name)
}

#[tokio::test]
async fn every_route_is_gated_by_its_permission() {
    let Some(e) = env().await else { return };
    let b = bound(&e, "trino").await;
    let (_, ua) = person(&e.db.pool).await;
    let grants_uri = format!("/perm/engines/{}/grants", b.engine);

    for uri in [
        "/perm/engine-capabilities",
        "/perm/engine-grants",
        &grants_uri,
    ] {
        assert_eq!(
            call(&e, "GET", uri, &e.nobody, None).await.0,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
        assert_eq!(
            call(&e, "GET", uri, &e.reader, None).await.0,
            StatusCode::OK,
            "{uri}"
        );
        assert_eq!(
            call(&e, "GET", uri, &e.admin, None).await.0,
            StatusCode::OK,
            "{uri}"
        );
    }
    assert_eq!(
        call(&e, "POST", &grants_uri, &e.nobody, Some(sys_grant(&ua)))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&e, "POST", &grants_uri, &e.reader, Some(sys_grant(&ua)))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, created) = call(&e, "POST", &grants_uri, &e.writer, Some(sys_grant(&ua))).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let one = format!("/perm/engine-grants/{id}");
    for token in [&e.nobody, &e.reader] {
        assert_eq!(
            call(&e, "PATCH", &one, token, Some(sys_grant(&ua))).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(&e, "DELETE", &one, token, None).await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(&e, "PATCH", &one, &e.writer, Some(sys_grant(&ua)))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&e, "DELETE", &one, &e.writer, None).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_catalogue_lists_both_engines_in_order_with_what_is_not_grantable() {
    let Some(e) = env().await else { return };
    let (status, body) = call(&e, "GET", "/perm/engine-capabilities", &e.reader, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let engines = body["engines"].as_array().unwrap();
    assert_eq!(engines.len(), 2);
    assert_eq!(engines[0]["engineType"], "trino");
    assert_eq!(engines[1]["engineType"], "starrocks");
    let ids = |i: usize| -> Vec<String> {
        engines[i]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        ids(0),
        [
            "trino.impersonate_user",
            "trino.set_system_session_property",
            "trino.set_catalog_session_property",
            "trino.execute_procedure",
            "trino.execute_function"
        ]
    );
    assert_eq!(
        ids(1),
        [
            "starrocks.impersonate_user",
            "starrocks.use_global_function",
            "starrocks.manage_blacklist"
        ]
    );
    for eng in engines {
        assert!(!eng["notGrantable"].as_array().unwrap().is_empty());
    }
    assert!(engines[1]["notGrantable"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["what"] == "system operate"));
    assert_eq!(
        engines[1]["capabilities"][2]["resources"],
        json!([{"key": "system", "valueKind": "fixed"}])
    );
}

#[tokio::test]
async fn a_create_compiles_into_the_profile_and_reports_its_reach() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let b = bound(&e, "trino").await;
    let (_, ua) = person(pool).await;
    let (status, body) = call(
        &e,
        "POST",
        &format!("/perm/engines/{}/grants", b.engine),
        &e.writer,
        Some(sys_grant(&ua)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["compile"]["services"], json!([b.profile]));
    assert!(compiled(pool, b.profile, &body["id"]).await.is_some());
    assert_eq!(body["reach"]["sharedProfile"], false);
    assert_eq!(body["reach"]["engines"].as_array().unwrap().len(), 1);
    assert_eq!(body["reach"]["engines"][0]["id"], json!(b.engine));

    // A legacy shared profile: a second engine bound by SQL. Both listings
    // show the grant reaching both, sorted by name.
    let second = engine(pool, "trino", "zzz-egrt-second", Some(b.profile)).await;
    for eng in [b.engine, second] {
        let (status, listing) = call(
            &e,
            "GET",
            &format!("/perm/engines/{eng}/grants"),
            &e.reader,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{listing}");
        assert_eq!(listing["reach"]["sharedProfile"], true);
        assert_eq!(listing["profile"]["serviceId"], json!(b.profile));
        assert_eq!(listing["profile"]["name"], json!(b.profile_name));
        // Security review M3: the listing of EITHER engine holds the grant,
        // marked with the engine it was written on.
        let g = &listing["grants"][0];
        assert_eq!(listing["grants"].as_array().unwrap().len(), 1, "{listing}");
        assert_eq!(g["id"], body["id"], "{listing}");
        assert_eq!(g["engineId"], json!(b.engine));
        let first_name: String =
            sqlx::query_scalar("SELECT name FROM engine_mgmt.engines WHERE id = $1")
                .bind(b.engine)
                .fetch_one(pool)
                .await
                .expect("name");
        assert_eq!(g["engineName"], json!(first_name));
        if eng == b.engine {
            assert_eq!(g["id"], body["id"]);
            assert_eq!(g["values"], json!(["query_max_memory"]));
            assert_eq!(
                g["policyName"],
                json!(format!("perm-engine-{}", body["id"].as_str().unwrap()))
            );
            let reach: Vec<Value> = g["reach"]["engines"].as_array().unwrap().clone();
            assert_eq!(reach.len(), 2);
            assert_eq!(reach[1]["id"], json!(second), "sorted by name");
        }
    }

    // `GET /perm/engine-grants` lists grants of two engines.
    let other = bound(&e, "starrocks").await;
    let (status, sr) = call(
        &e,
        "POST",
        &format!("/perm/engines/{}/grants", other.engine),
        &e.writer,
        Some(json!({"capability": "starrocks.manage_blacklist", "principals": [{"kind": "user", "ref": ua}]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sr}");
    let (_, all) = call(&e, "GET", "/perm/engine-grants", &e.reader, None).await;
    let ids: Vec<&Value> = all.as_array().unwrap().iter().map(|g| &g["id"]).collect();
    assert!(
        ids.contains(&&body["id"]) && ids.contains(&&sr["id"]),
        "{all}"
    );
}

#[tokio::test]
async fn every_refusal_answers_its_status() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let trino = bound(&e, "trino").await;
    let sr = bound(&e, "starrocks").await;
    let catalog = attach_catalog(pool, trino.engine).await;
    let (_, ua) = person(pool).await;
    let (pa_id, platform_admin_target) = person(pool).await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(pa_id)
    .execute(pool)
    .await
    .expect("platform_admin");
    let group = unique("egrt-g-");
    sqlx::query("INSERT INTO groups (name, source) VALUES ($1, 'local')")
        .bind(&group)
        .execute(pool)
        .await
        .expect("group");
    let post = |eng: Uuid, body: Value| {
        let app = e.app.clone();
        let token = e.writer.clone();
        async move {
            call_app(
                app,
                "POST",
                format!("/perm/engines/{eng}/grants"),
                token,
                Some(body),
            )
            .await
        }
    };
    let user = json!([{"kind": "user", "ref": ua}]);

    let bad = [
        (
            "unknown capability",
            trino.engine,
            json!({"capability": "trino.kill_any", "values": [], "principals": user}),
        ),
        ("other engine", sr.engine, sys_grant(&ua)),
        (
            "wildcard",
            trino.engine,
            json!({"capability": "trino.set_system_session_property", "values": ["*"], "principals": user}),
        ),
        (
            "no principals",
            trino.engine,
            json!({"capability": "trino.set_system_session_property", "values": ["q"], "principals": []}),
        ),
        (
            "workspace_members",
            trino.engine,
            json!({"capability": "trino.set_system_session_property", "values": ["q"], "principals": [{"kind": "workspace_members", "ref": Uuid::new_v4()}]}),
        ),
        (
            "public",
            trino.engine,
            json!({"capability": "trino.set_system_session_property", "values": ["q"], "principals": [{"kind": "group", "ref": "public"}]}),
        ),
        (
            "unreached catalog",
            trino.engine,
            json!({"capability": "trino.set_catalog_session_property", "values": ["nope", "max_splits"], "principals": user}),
        ),
        (
            "unknown user ref",
            trino.engine,
            json!({"capability": "trino.set_system_session_property", "values": ["q"], "principals": [{"kind": "user", "ref": unique("ghost-")}]}),
        ),
        (
            "unknown target",
            trino.engine,
            json!({"capability": "trino.impersonate_user", "values": [unique("ghost-")], "principals": user}),
        ),
    ];
    for (why, eng, body) in bad {
        let (status, resp) = post(eng, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {resp}");
    }
    // A reached catalog passes.
    let (status, resp) = post(
        trino.engine,
        json!({"capability": "trino.set_catalog_session_property", "values": [catalog, "max_splits"], "principals": user}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{resp}");

    let no_profile = engine(pool, "trino", &unique("egrt-np-"), None).await;
    let deleting = bound(&e, "trino").await;
    sqlx::query("UPDATE engine_mgmt.engines SET status = 'deleting' WHERE id = $1")
        .bind(deleting.engine)
        .execute(pool)
        .await
        .expect("deleting");
    let (rw_profile, _) = profile(&e, "trino").await;
    let rw = engine(pool, "risingwave", &unique("egrt-rw-"), Some(rw_profile)).await;
    let imp = |target: &str| json!({"capability": "trino.impersonate_user", "values": [target], "principals": user});
    let conflicts = [
        ("no profile", no_profile, sys_grant(&ua)),
        ("deleting", deleting.engine, sys_grant(&ua)),
        ("risingwave", rw, sys_grant(&ua)),
        ("delegator target", trino.engine, imp("egret_delegator")),
        ("lineage target", trino.engine, imp("egret-lineage")),
        ("root target (Trino)", trino.engine, imp("root")),
        (
            "Root target (StarRocks)",
            sr.engine,
            json!({"capability": "starrocks.impersonate_user", "values": ["Root"], "principals": user}),
        ),
        (
            "platform admin target",
            trino.engine,
            imp(&platform_admin_target),
        ),
    ];
    for (why, eng, body) in conflicts {
        let (status, resp) = post(eng, body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{why}: {resp}");
    }
    let (status, _) = post(trino.engine, sys_grant(&ua)).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, resp) = post(trino.engine, sys_grant(&ua)).await;
    assert_eq!(status, StatusCode::CONFLICT, "duplicate: {resp}");

    let (status, _) = post(Uuid::new_v4(), sys_grant(&ua)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let missing = format!("/perm/engine-grants/{}", Uuid::new_v4());
    assert_eq!(
        call(&e, "PATCH", &missing, &e.writer, Some(sys_grant(&ua)))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&e, "DELETE", &missing, &e.writer, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, _) = call(
        &e,
        "GET",
        &format!("/perm/engines/{}/grants", Uuid::new_v4()),
        &e.reader,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn patch_replaces_and_recompiles_and_delete_reaps() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let b = bound(&e, "trino").await;
    let (_, ua) = person(pool).await;
    let (_, ub) = person(pool).await;
    let (uc_id, uc) = person(pool).await;
    let (_, created) = call(
        &e,
        "POST",
        &format!("/perm/engines/{}/grants", b.engine),
        &e.writer,
        Some(sys_grant(&ua)),
    )
    .await;
    let id = created["id"].clone();
    let one = format!("/perm/engine-grants/{}", id.as_str().unwrap());

    let (status, resp) = call(
        &e,
        "PATCH",
        &one,
        &e.writer,
        Some(json!({"capability": "trino.impersonate_user", "values": [uc.to_uppercase()], "principals": [{"kind": "user", "ref": ub}]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let body = compiled(pool, b.profile, &id).await.expect("compiled");
    assert_eq!(body["policyItems"][0]["users"], json!([ub]));
    assert_eq!(
        body["resources"],
        json!({"trinouser": {"values": [uc], "isExcludes": false, "isRecursive": false}})
    );
    let stored: Option<Uuid> =
        sqlx::query_scalar("SELECT target_user_id FROM perm.engine_grants WHERE id = $1::uuid")
            .bind(id.as_str().unwrap())
            .fetch_one(pool)
            .await
            .expect("row");
    assert_eq!(stored, Some(uc_id), "the target is stored by id");
    let (_, listing) = call(
        &e,
        "GET",
        &format!("/perm/engines/{}/grants", b.engine),
        &e.reader,
        None,
    )
    .await;
    assert_eq!(listing["grants"][0]["values"], json!([uc]));
    assert_eq!(listing["grants"][0]["targetUserId"], json!(uc_id));

    let (status, resp) = call(&e, "DELETE", &one, &e.writer, None).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["compile"]["deleted"], 1);
    assert!(compiled(pool, b.profile, &id).await.is_none());
}

async fn seed_infra(
    e: &Env,
    profile: i64,
    name: &str,
    resources: Value,
    users: Value,
    access: &str,
) {
    e.policy
        .upsert_infra_policy(
            profile,
            name,
            &json!({
                "name": name, "policyType": 0, "isEnabled": true, "resources": resources,
                "policyItems": [{"users": users, "groups": [], "roles": [],
                                 "accesses": [{"type": access, "isAllowed": true}]}],
            }),
        )
        .await
        .expect("infra policy");
}

#[tokio::test]
async fn the_listing_maps_seeded_baselines_to_managed_entries() {
    let Some(e) = env().await else { return };
    let b = bound(&e, "trino").await;
    let star = |k: &str| json!({k: {"values": ["*"]}});
    seed_infra(
        &e,
        b.profile,
        "egret-user-impersonate-self",
        json!({"trinouser": {"values": ["{USER}"]}}),
        json!(["{USER}"]),
        "impersonate",
    )
    .await;
    seed_infra(
        &e,
        b.profile,
        "egret-user-execute-query",
        star("queryid"),
        json!(["{USER}"]),
        "execute",
    )
    .await;
    seed_infra(
        &e,
        b.profile,
        "egret-query-timeout",
        json!({"systemproperty": {"values": ["query_max_run_time"]}}),
        json!(["{USER}"]),
        "alter",
    )
    .await;
    seed_infra(
        &e,
        b.profile,
        "egret-delegator-impersonate",
        star("trinouser"),
        json!(["egret_delegator"]),
        "impersonate",
    )
    .await;
    e.policy
        .seed_admin_all_access(
            b.profile,
            &b.profile_name,
            "trino",
            &["root-admin".to_string()],
        )
        .await
        .expect("admin");

    let (status, body) = call(
        &e,
        "GET",
        &format!("/perm/engines/{}/grants", b.engine),
        &e.reader,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let managed = body["managed"].as_array().unwrap();
    let by_name = |n: &str| {
        managed
            .iter()
            .find(|m| m["policyName"] == n)
            .unwrap_or_else(|| panic!("{n} in {body}"))
    };
    let selfimp = by_name("egret-user-impersonate-self");
    assert_eq!(selfimp["kind"], "user_baseline");
    assert_eq!(selfimp["capability"], "trino.impersonate_user");
    assert_eq!(selfimp["resources"], json!({"trinouser": ["{USER}"]}));
    assert_eq!(
        by_name("egret-user-execute-query")["capability"],
        Value::Null
    );
    assert_eq!(
        by_name("egret-query-timeout")["capability"],
        "trino.set_system_session_property"
    );
    assert_eq!(by_name("egret-delegator-impersonate")["kind"], "delegator");
    let admin_rows: Vec<&Value> = managed
        .iter()
        .filter(|m| {
            m["policyName"]
                .as_str()
                .unwrap()
                .starts_with("egret-admin - ")
        })
        .collect();
    assert!(!admin_rows.is_empty());
    for row in &admin_rows {
        assert_eq!(row["kind"], "admin_all_access");
    }
    assert_eq!(
        by_name("egret-admin - trinouser")["capability"],
        "trino.impersonate_user"
    );
    assert_eq!(by_name("egret-admin - queryid")["capability"], Value::Null);

    // No profile: 200, `profile: null`, empty lists.
    let np = engine(&e.db.pool, "trino", &unique("egrt-np-"), None).await;
    let (status, body) = call(
        &e,
        "GET",
        &format!("/perm/engines/{np}/grants"),
        &e.reader,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["profile"], Value::Null);
    assert_eq!(body["grants"], json!([]));
    assert_eq!(body["managed"], json!([]));
}

/// Whether `fut` is still pending after `wait`; the future is driven in a task.
async fn blocks<T: Send + 'static>(
    fut: impl std::future::Future<Output = T> + Send + 'static,
    wait: Duration,
) -> (bool, tokio::task::JoinHandle<T>) {
    let h = tokio::spawn(fut);
    tokio::time::sleep(wait).await;
    (!h.is_finished(), h)
}

/// Step 5: the write holds the engine row `FOR SHARE`, which conflicts with
/// the UPDATE a rebind makes (and `FOR KEY SHARE` would not). Pinned from the
/// other side: an uncommitted UPDATE of the engine row's profile column makes
/// the POST wait until it commits.
#[tokio::test]
async fn a_write_waits_on_an_uncommitted_rebind_of_its_engine() {
    let Some(e) = env().await else { return };
    let b = bound(&e, "trino").await;
    let (_, ua) = person(&e.db.pool).await;
    let mut rebind = e.db.pool.begin().await.expect("begin");
    sqlx::query("UPDATE engine_mgmt.engines SET authz_service_id = authz_service_id WHERE id = $1")
        .bind(b.engine)
        .execute(&mut *rebind)
        .await
        .expect("rebind");
    let (pending, h) = blocks(
        call_app(
            e.app.clone(),
            "POST",
            format!("/perm/engines/{}/grants", b.engine),
            e.writer.clone(),
            Some(sys_grant(&ua)),
        ),
        Duration::from_millis(800),
    )
    .await;
    assert!(pending, "the write must wait on the engine row");
    rebind.commit().await.expect("commit");
    let (status, body) = h.await.expect("join");
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// The Task 6 amendment: an impersonation write takes the target's per-user
/// lock (the one `UserRepo::hard_delete` takes first), and takes it BEFORE the
/// group locks. Held by another transaction, the user lock makes the write
/// wait; and with a group lock held elsewhere, the write already holds the
/// user lock while it waits, so a third transaction cannot take it.
#[tokio::test]
async fn an_impersonation_write_takes_the_targets_user_lock_before_the_group_locks() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let b = bound(&e, "trino").await;
    let (ua, _) = person(pool).await;
    let (uc, c) = person(pool).await;
    let group = unique("egrt-g-");
    let gid: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(&group)
            .fetch_one(pool)
            .await
            .expect("group");
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(ua)
        .bind(gid)
        .execute(pool)
        .await
        .expect("member");
    let body = json!({"capability": "trino.impersonate_user", "values": [c],
                      "principals": [{"kind": "group", "ref": group}]});
    let uri = format!("/perm/engines/{}/grants", b.engine);

    // 1. The user lock held elsewhere: the write waits.
    let mut holder = pool.begin().await.expect("begin");
    egret_auth::groups::lock_member_users_via(&mut holder, &[uc])
        .await
        .expect("user lock");
    let (pending, h) = blocks(
        call_app(
            e.app.clone(),
            "POST",
            uri.clone(),
            e.writer.clone(),
            Some(body.clone()),
        ),
        Duration::from_millis(800),
    )
    .await;
    assert!(pending, "the write must wait on the target's user lock");
    holder.rollback().await.expect("release");
    let (status, resp) = h.await.expect("join");
    assert_eq!(status, StatusCode::CREATED, "{resp}");
    let id = resp["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &e,
        "DELETE",
        &format!("/perm/engine-grants/{id}"),
        &e.writer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 2. The group lock held elsewhere: the write is parked on it, already
    //    holding the user lock.
    let mut group_holder = pool.begin().await.expect("begin");
    egret_auth::groups::lock_group_names_via(&mut group_holder, std::slice::from_ref(&group))
        .await
        .expect("group lock");
    let (pending, h) = blocks(
        call_app(
            e.app.clone(),
            "POST",
            uri.clone(),
            e.writer.clone(),
            Some(body),
        ),
        Duration::from_millis(800),
    )
    .await;
    assert!(pending, "the write must wait on the group lock");
    let mut probe = pool.begin().await.expect("begin");
    sqlx::query("SET LOCAL lock_timeout = '300ms'")
        .execute(&mut *probe)
        .await
        .expect("timeout");
    let took = egret_auth::groups::lock_member_users_via(&mut probe, &[uc]).await;
    assert!(
        took.is_err(),
        "the parked write must already hold the user lock (user lock before group locks)"
    );
    drop(probe);
    group_holder.rollback().await.expect("release");
    let (status, resp) = h.await.expect("join");
    assert_eq!(status, StatusCode::CREATED, "{resp}");
}

/// A hand-written policy, as an operator writes it in the editor.
async fn hand_written(
    pool: &PgPool,
    profile: i64,
    name: &str,
    resources: Value,
    users: Value,
    groups: Value,
    access: &str,
) {
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) VALUES ($1, $2, $3)",
    )
    .bind(profile)
    .bind(name)
    .bind(json!({
        "name": name, "policyType": 0, "isEnabled": true, "resources": resources,
        "policyItems": [{"users": users, "groups": groups, "roles": [],
                         "accesses": [{"type": access, "isAllowed": true}]}],
    }))
    .execute(pool)
    .await
    .expect("hand-written policy");
}

/// Security fix I1: a target who holds a not-grantable (near-root) permission
/// on the profile through ANY policy that is not an engine grant's — an
/// operator's hand-written one included, and through a group — cannot be
/// impersonated: impersonating them would hand that permission out.
#[tokio::test]
async fn a_target_holding_a_near_root_permission_by_any_policy_is_refused() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let sr = bound(&e, "starrocks").await;
    let trino = bound(&e, "trino").await;
    let (_, grantee) = person(pool).await;
    let (_, ops) = person(pool).await;
    let (ops2_id, ops2) = person(pool).await;
    let (_, plain) = person(pool).await;
    let group = unique("egrt-ops-");
    let gid: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(&group)
            .fetch_one(pool)
            .await
            .expect("group");
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(ops2_id)
        .bind(gid)
        .execute(pool)
        .await
        .expect("member");
    hand_written(
        pool,
        sr.profile,
        "ops can operate",
        json!({"system": {"values": ["*"]}}),
        json!([ops]),
        json!([]),
        "operate",
    )
    .await;
    hand_written(
        pool,
        trino.profile,
        "ops group becomes anyone",
        json!({"trinouser": {"values": ["*"]}}),
        json!([]),
        json!([group]),
        "impersonate",
    )
    .await;
    let user = json!([{"kind": "user", "ref": grantee}]);
    let post = |eng: Uuid, cap: &str, target: &str| {
        let app = e.app.clone();
        let token = e.writer.clone();
        let body = json!({"capability": cap, "values": [target], "principals": user});
        async move {
            call_app(
                app,
                "POST",
                format!("/perm/engines/{eng}/grants"),
                token,
                Some(body),
            )
            .await
        }
    };

    let (status, body) = post(sr.engine, "starrocks.impersonate_user", &ops).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "system operate holder: {body}"
    );
    assert!(
        body.to_string().contains("ops can operate"),
        "names the policy: {body}"
    );
    let (status, body) = post(trino.engine, "trino.impersonate_user", &ops2).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "trinouser * through a group: {body}"
    );
    let (status, body) = post(sr.engine, "starrocks.impersonate_user", &plain).await;
    assert_eq!(status, StatusCode::CREATED, "a plain user: {body}");
    let (status, body) = post(trino.engine, "trino.impersonate_user", &plain).await;
    assert_eq!(status, StatusCode::CREATED, "a plain user: {body}");
}

/// Security re-review I-a: impersonation chains. A target who can impersonate
/// (through any policy of the profile: hand-written, infra or a compiled engine
/// grant) someone near-root or reserved is as dangerous as that someone.
#[tokio::test]
async fn an_impersonation_chain_to_a_near_root_user_is_refused() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let sr = bound(&e, "starrocks").await;
    let trino = bound(&e, "trino").await;
    let (_, alice) = person(pool).await;
    let (_, bob) = person(pool).await;
    let (_, carol) = person(pool).await;
    let (_, dave) = person(pool).await;
    let (_, bob2) = person(pool).await;
    let (_, bob3) = person(pool).await;
    let (_, erin) = person(pool).await;
    let (_, bob4) = person(pool).await;
    let none = json!([]);
    let post = |eng: Uuid, cap: &str, target: &str, grantee: &str| {
        let app = e.app.clone();
        let token = e.writer.clone();
        let body = json!({"capability": cap, "values": [target],
                          "principals": [{"kind": "user", "ref": grantee}]});
        async move {
            call_app(
                app,
                "POST",
                format!("/perm/engines/{eng}/grants"),
                token,
                Some(body),
            )
            .await
        }
    };

    // StarRocks: bob may become carol (hand-written), carol may operate.
    hand_written(
        pool,
        sr.profile,
        &unique("bob-to-carol-"),
        json!({"user": {"values": [carol]}}),
        json!([bob]),
        none.clone(),
        "impersonate",
    )
    .await;
    hand_written(
        pool,
        sr.profile,
        &unique("carol-operates-"),
        json!({"system": {"values": ["*"]}}),
        json!([carol]),
        none.clone(),
        "operate",
    )
    .await;
    let (status, body) = post(sr.engine, "starrocks.impersonate_user", &bob, &alice).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "bob -> carol -> operate: {body}"
    );

    // Trino: bob2 may become carol (hand-written), carol may become anyone.
    hand_written(
        pool,
        trino.profile,
        &unique("bob2-to-carol-"),
        json!({"trinouser": {"values": [carol]}}),
        json!([bob2]),
        none.clone(),
        "impersonate",
    )
    .await;
    hand_written(
        pool,
        trino.profile,
        &unique("carol-anyone-"),
        json!({"trinouser": {"values": ["*"]}}),
        json!([carol]),
        none.clone(),
        "impersonate",
    )
    .await;
    let (status, body) = post(trino.engine, "trino.impersonate_user", &bob2, &alice).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "bob2 -> carol -> anyone: {body}"
    );

    // Two hops through a compiled engine grant: bob3 -(engine grant)-> dave
    // -(hand-written)-> carol, who operates.
    let (status, body) = post(sr.engine, "starrocks.impersonate_user", &dave, &bob3).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "dave is harmless so far: {body}"
    );
    // The hop must come from the MODEL, not only from what has been compiled:
    // drop the compiled policy, as if the grant were not compiled yet.
    sqlx::query("DELETE FROM policy.authz_policies WHERE name = $1")
        .bind(format!("perm-engine-{}", body["id"].as_str().unwrap()))
        .execute(pool)
        .await
        .expect("drop the compiled hop");
    hand_written(
        pool,
        sr.profile,
        &unique("dave-to-carol-"),
        json!({"user": {"values": [carol]}}),
        json!([dave]),
        none.clone(),
        "impersonate",
    )
    .await;
    let (status, body) = post(sr.engine, "starrocks.impersonate_user", &bob3, &alice).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "bob3 -> dave -> carol -> operate: {body}"
    );

    // A chain to `root` is a chain to the engine superuser.
    hand_written(
        pool,
        sr.profile,
        &unique("bob4-to-root-"),
        json!({"user": {"values": ["root"]}}),
        json!([bob4]),
        none.clone(),
        "impersonate",
    )
    .await;
    let (status, body) = post(sr.engine, "starrocks.impersonate_user", &bob4, &alice).await;
    assert_eq!(status, StatusCode::CONFLICT, "bob4 -> root: {body}");

    // A harmless chain: erin -> dave2 who holds nothing, and a {USER} self hop.
    let (_, dave2) = person(pool).await;
    hand_written(
        pool,
        sr.profile,
        &unique("erin-to-dave2-"),
        json!({"user": {"values": [dave2]}}),
        json!([erin]),
        none.clone(),
        "impersonate",
    )
    .await;
    hand_written(
        pool,
        sr.profile,
        &unique("self-"),
        json!({"user": {"values": ["{USER}"]}}),
        json!(["{USER}"]),
        none.clone(),
        "impersonate",
    )
    .await;
    let (status, body) = post(sr.engine, "starrocks.impersonate_user", &erin, &alice).await;
    assert_eq!(status, StatusCode::CREATED, "a harmless chain: {body}");
}

/// Security re-review M-c: Ranger matches group names ignoring case.
#[tokio::test]
async fn the_near_root_scan_matches_groups_ignoring_case() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let sr = bound(&e, "starrocks").await;
    let (_, alice) = person(pool).await;
    let (ops_id, ops) = person(pool).await;
    let group = format!("OpsTeam{}", Uuid::new_v4().simple());
    let gid: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(&group)
            .fetch_one(pool)
            .await
            .expect("group");
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(ops_id)
        .bind(gid)
        .execute(pool)
        .await
        .expect("member");
    hand_written(
        pool,
        sr.profile,
        "ops team operates",
        json!({"system": {"values": ["*"]}}),
        json!([]),
        json!([group.to_uppercase()]),
        "operate",
    )
    .await;
    let (status, body) = call(
        &e,
        "POST",
        &format!("/perm/engines/{}/grants", sr.engine),
        &e.writer,
        Some(
            json!({"capability": "starrocks.impersonate_user", "values": [ops],
                    "principals": [{"kind": "user", "ref": alice}]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

/// DG #3 × F11: every engine-grant write records one governance event in its
/// own transaction (`engine_grant:create|update|delete`, family Policy); a
/// refused write records none.
#[tokio::test]
async fn engine_grant_writes_record_governance_events() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let b = bound(&e, "trino").await;
    let (_, ua) = person(pool).await;
    let (_, ub) = person(pool).await;
    let events = |id: String| async move {
        sqlx::query_as::<_, (String, String, Value)>(
            "SELECT action, resource_type, detail FROM audit.audit_log \
              WHERE kind = 'mutation' AND resource_id = $1 ORDER BY id",
        )
        .bind(id)
        .fetch_all(pool)
        .await
        .expect("events")
    };

    let (status, body) = call(
        &e,
        "POST",
        &format!("/perm/engines/{}/grants", b.engine),
        &e.writer,
        Some(sys_grant(&ua)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let created = json!({
        "engineId": b.engine, "capability": "trino.set_system_session_property",
        "values": ["query_max_memory"], "principals": [{"kind": "user", "ref": ua}],
        "services": [b.profile]
    });
    let evs = events(id.clone()).await;
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert_eq!(
        (evs[0].0.as_str(), evs[0].1.as_str()),
        ("engine_grant:create", "engine_grant")
    );
    assert_eq!(evs[0].2, json!({"before": null, "after": created}));

    let (status, body) = call(
        &e,
        "PATCH",
        &format!("/perm/engine-grants/{id}"),
        &e.writer,
        Some(sys_grant(&ub)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut patched = created.clone();
    patched["principals"] = json!([{"kind": "user", "ref": ub}]);
    let evs = events(id.clone()).await;
    assert_eq!(evs.len(), 2, "{evs:?}");
    assert_eq!(evs[1].0, "engine_grant:update");
    assert_eq!(evs[1].2, json!({"before": created, "after": patched}));

    // A refused write (unknown capability) records nothing.
    let (status, _) = call(
        &e,
        "PATCH",
        &format!("/perm/engine-grants/{id}"),
        &e.writer,
        Some(json!({"capability": "trino.nope", "principals": [{"kind": "user", "ref": ub}]})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(events(id.clone()).await.len(), 2);

    let (status, body) = call(
        &e,
        "DELETE",
        &format!("/perm/engine-grants/{id}"),
        &e.writer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let evs = events(id.clone()).await;
    assert_eq!(evs.len(), 3, "{evs:?}");
    assert_eq!(evs[2].0, "engine_grant:delete");
    assert_eq!(evs[2].2, json!({"before": patched, "after": null}));
}
