//! People & groups Task 4: `POST /perm/explain/principal` answers for a group
//! or role as its members' answers, each exactly `POST /perm/explain`'s for
//! that username (spec §4.3, P5, P6).
//!
//! Its own binary: `egret-test-db` gives it a private database. Each test has
//! its own profile, catalog, group and role, and they run one at a time
//! (`SERIAL`).
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test explain_principal

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
use tower::ServiceExt; // oneshot
use uuid::Uuid;

fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// Held for a whole test, so the tests in this binary run one at a time.
/// Every compile expands — and since the 2026-10-18 design R5 rebase pre-mints
/// for — the WHOLE model, so one test's `grant_to` mints the username of a
/// member another test deliberately left without one, and its "not
/// evaluated: no username" assertion then fails for a reason that is not its
/// own.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Env {
    _serial: tokio::sync::MutexGuard<'static, ()>,
    db: Db,
    app: axum::Router,
    admin: String,
    reader: String,
    nobody: String,
    catalog_id: Uuid,
}

async fn insert_user(pool: &sqlx::PgPool, email: &str, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local', $1, $1, $2) \
         RETURNING id",
    )
    .bind(email)
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("user")
}

async fn session(db: &Db, sessions: &SessionService, role: Option<(&str, &str)>) -> String {
    let t = tag();
    let uid = insert_user(&db.pool, &format!("xp_gate_{t}@egret.local"), None).await;
    match role {
        Some(("platform_admin", _)) => {
            sqlx::query(
                "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
                 SELECT 'user', $1, id, 'platform' FROM roles \
                  WHERE scope = 'platform' AND name = 'platform_admin'",
            )
            .bind(uid)
            .execute(&db.pool)
            .await
            .expect("platform_admin");
        }
        Some((_, permission)) => {
            let role_id: Uuid = sqlx::query_scalar(
                "INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id",
            )
            .bind(format!("xp_gate_role_{t}"))
            .fetch_one(&db.pool)
            .await
            .expect("role");
            sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
                .bind(role_id)
                .bind(permission)
                .execute(&db.pool)
                .await
                .expect("permission");
            sqlx::query(
                "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
                 VALUES ('user', $1, $2, 'platform')",
            )
            .bind(uid)
            .bind(role_id)
            .execute(&db.pool)
            .await
            .expect("grant");
        }
        None => {}
    }
    sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

/// A StarRocks profile with one bound engine and one attached catalog, and
/// three sessions.
async fn env() -> Option<Env> {
    let serial = SERIAL.lock().await;
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let t = tag();
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks");
    let service = policy
        .create_service(&format!("sr_xp_{t}"), "starrocks", &json!({}))
        .await
        .expect("service");
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $1) RETURNING id")
            .bind(format!("xp-ws-{t}"))
            .fetch_one(&pool)
            .await
            .expect("workspace");
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, 'us-east-1', 'egret-xp-test', 'AKIATEST') RETURNING id",
    )
    .bind(format!("xp-sc-{t}"))
    .fetch_one(&pool)
    .await
    .expect("storage connection");
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("xp_wh_{t}"))
    .bind(ws)
    .bind(sc)
    .fetch_one(&pool)
    .await
    .expect("warehouse");
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, 'starrocks', 'egret', \
                 '{\"starrocksConfig\":{\"feImage\":\"starrocks/fe-ubuntu:4.1.4\"}}'::jsonb, $2) \
         RETURNING id",
    )
    .bind(format!("xp-eng-{t}"))
    .bind(service.id)
    .fetch_one(&pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(format!("xp_cat_{t}"))
    .bind(wh)
    .fetch_one(&pool)
    .await
    .expect("catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1, $2)",
    )
    .bind(engine)
    .bind(catalog_id)
    .execute(&pool)
    .await
    .expect("attach");

    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let admin = session(&db, &sessions, Some(("platform_admin", ""))).await;
    let reader = session(&db, &sessions, Some(("", "policy:read"))).await;
    let nobody = session(&db, &sessions, None).await;
    // Built AFTER every permission above is written: the PDP loads them once.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("pdp"),
    );
    let app = perm_router().with_state(PermState {
        perm: PermStore::new(pool),
        policy,
        sessions,
        pdp,
    });
    Some(Env {
        _serial: serial,
        db,
        app,
        admin,
        reader,
        nobody,
        catalog_id,
    })
}

async fn post(e: &Env, token: &str, uri: &str, body: &Value) -> (StatusCode, Value) {
    let resp = e
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(uri)
                .header("Cookie", format!("egret_session={token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

fn target(e: &Env) -> Value {
    json!({ "catalog_id": e.catalog_id, "schema": "sales", "table": "orders", "column": null })
}

async fn explain_principal(e: &Env, token: &str, kind: &str, r: &str) -> (StatusCode, Value) {
    post(
        e,
        token,
        "/perm/explain/principal",
        &json!({ "principal": { "kind": kind, "ref": r }, "action": "read", "target": target(e) }),
    )
    .await
}

async fn grant_to(e: &Env, effect: &str, kind: &str, r: &str) {
    let (status, body) = post(
        e,
        &e.admin,
        "/perm/grants",
        &json!({
            "effect": effect,
            "actions": ["read"],
            "scope": target(e),
            "principals": { "include": [{ "kind": kind, "ref": r }], "exclude": [] }
        }),
    )
    .await;
    assert!(status.is_success(), "grant to {kind}:{r}: {status} {body}");
}

async fn group_with(pool: &sqlx::PgPool, name: &str, members: &[Uuid]) {
    let id: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("group");
    for m in members {
        add_to_group(pool, id, *m).await;
    }
}

async fn add_to_group(pool: &sqlx::PgPool, group: Uuid, user: Uuid) {
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user)
        .bind(group)
        .execute(pool)
        .await
        .expect("member");
}

/// Each evaluated member's `result` is `/perm/explain`'s answer for them,
/// compared as JSON values (never as strings: key order is not part of it).
async fn assert_each_equals_explain(e: &Env, body: &Value) {
    for m in body["evaluated"].as_array().expect("evaluated") {
        let username = m["username"].as_str().unwrap();
        let (status, single) = post(
            e,
            &e.admin,
            "/perm/explain",
            &json!({ "subject": { "user": username }, "action": "read", "target": target(e) }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{single}");
        assert_eq!(m["result"], single, "{username}");
    }
}

fn select_decision(result: &Value) -> Value {
    result["profiles"][0]["accesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["access_type"] == "select")
        .map(|a| a["decision"].clone())
        .expect("select")
}

#[tokio::test]
async fn a_groups_members_are_each_answered_as_explain_answers_them() {
    let Some(e) = env().await else { return };
    let t = tag();
    let p = &e.db.pool;
    let a = insert_user(p, &format!("a_{t}@x.local"), Some(&format!("a_{t}"))).await;
    let b = insert_user(p, &format!("b_{t}@x.local"), Some(&format!("b_{t}"))).await;
    let c = insert_user(p, &format!("c_{t}@x.local"), Some(&format!("c_{t}"))).await;
    let g = format!("xg_{t}");
    group_with(p, &g, &[a, b, c]).await;
    // A grant to the group, and a deny to one member only, so the members'
    // answers differ: evaluating the wrong username shows.
    grant_to(&e, "allow", "group", &g).await;
    grant_to(&e, "deny", "user", &format!("c_{t}")).await;
    // Added after the compile, so nothing has minted a username for them.
    let unnamed = insert_user(p, &format!("z_{t}@x.local"), None).await;
    let gid: Uuid = sqlx::query_scalar("SELECT id FROM groups WHERE name = $1")
        .bind(&g)
        .fetch_one(p)
        .await
        .unwrap();
    add_to_group(p, gid, unnamed).await;

    let (status, body) = explain_principal(&e, &e.admin, "group", &g).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["membersTotal"], json!(4));
    assert_eq!(body["limit"], json!(200));
    assert_eq!(body["truncated"], json!(false));
    let names: Vec<&str> = body["evaluated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["username"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            format!("a_{t}").as_str(),
            &format!("b_{t}"),
            &format!("c_{t}")
        ],
        "sorted by username"
    );
    assert_eq!(
        body["notEvaluated"],
        json!([{ "userId": unnamed, "label": format!("z_{t}@x.local"), "reason": "no_username" }])
    );
    let kinds: Vec<Value> = body["evaluated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| select_decision(&m["result"])["kind"].clone())
        .collect();
    assert_eq!(
        kinds,
        vec![json!("allowed"), json!("allowed"), json!("denied")]
    );
    assert_each_equals_explain(&e, &body).await;
    assert!(body.get("verdict").is_none(), "no overall verdict");
}

#[tokio::test]
async fn a_roles_holders_are_each_answered_as_explain_answers_them() {
    let Some(e) = env().await else { return };
    let t = tag();
    let p = &e.db.pool;
    let role = format!("xr_{t}");
    let rid: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role)
            .fetch_one(p)
            .await
            .unwrap();
    let hold = |u: Uuid| {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
             VALUES ('user', $1, $2, 'platform')",
        )
        .bind(u)
        .bind(rid)
        .execute(p)
    };
    let mut holders = Vec::new();
    for n in ["ra", "rb", "rc"] {
        let u = insert_user(p, &format!("{n}_{t}@x.local"), Some(&format!("{n}_{t}"))).await;
        hold(u).await.unwrap();
        holders.push(u);
    }
    grant_to(&e, "allow", "role", &role).await;
    // Added after the compile: a compile mints a role holder's missing
    // username (2026-10-18 design R5), so only a holder it never saw has none.
    let unnamed = insert_user(p, &format!("rz_{t}@x.local"), None).await;
    hold(unnamed).await.unwrap();
    holders.push(unnamed);

    let (status, body) = explain_principal(&e, &e.reader, "role", &role).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["membersTotal"], json!(4));
    assert_eq!(body["evaluated"].as_array().unwrap().len(), 3);
    assert_eq!(body["notEvaluated"][0]["userId"], json!(holders[3]));
    assert_each_equals_explain(&e, &body).await;
}

#[tokio::test]
async fn an_empty_group_has_no_members_and_201_members_are_truncated_at_200() {
    let Some(e) = env().await else { return };
    let t = tag();
    let p = &e.db.pool;
    let empty = format!("xe_{t}");
    group_with(p, &empty, &[]).await;
    let (status, body) = explain_principal(&e, &e.admin, "group", &empty).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "membersTotal": 0, "limit": 200, "truncated": false,
                "evaluated": [], "notEvaluated": [] })
    );

    let big = format!("xb_{t}");
    group_with(p, &big, &[]).await;
    sqlx::query(
        "WITH u AS ( \
           INSERT INTO users (provider, email, name, username) \
           SELECT 'local', format('m%s_%s@x.local', lpad(i::text, 3, '0'), $1::text), 'M', \
                  format('m%s_%s', lpad(i::text, 3, '0'), $1::text) \
             FROM generate_series(1, 201) AS i RETURNING id) \
         INSERT INTO user_groups (user_id, group_id) \
         SELECT u.id, g.id FROM u, groups g WHERE g.name = $2",
    )
    .bind(&t)
    .bind(&big)
    .execute(p)
    .await
    .expect("201 members");
    let (status, body) = explain_principal(&e, &e.admin, "group", &big).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["membersTotal"], json!(201));
    assert_eq!(body["truncated"], json!(true));
    let evaluated = body["evaluated"].as_array().unwrap();
    assert_eq!(evaluated.len(), 200);
    assert_eq!(evaluated[0]["username"], json!(format!("m001_{t}")));
    assert_eq!(evaluated[199]["username"], json!(format!("m200_{t}")));
    assert_eq!(body["notEvaluated"], json!([]));
}

#[tokio::test]
async fn public_workspaces_users_unknown_names_and_bad_targets_are_400s() {
    let Some(e) = env().await else { return };
    let t = tag();
    let g = format!("xv_{t}");
    group_with(&e.db.pool, &g, &[]).await;
    for (kind, r) in [
        ("group", "public".to_string()),
        ("workspace_members", Uuid::new_v4().to_string()),
        ("user", "someone".to_string()),
        ("group", format!("nogroup_{t}")),
        ("role", format!("norole_{t}")),
    ] {
        let (status, body) = explain_principal(&e, &e.admin, kind, &r).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{kind}:{r}: {body}");
        assert!(
            body["error"].as_str().is_some_and(|s| s.len() > 10),
            "{body}"
        );
    }
    // Every `validate_explain_target` case, and a catalog nothing has.
    for target in [
        json!({ "catalog_id": e.catalog_id, "schema": null, "table": "orders", "column": null }),
        json!({ "catalog_id": e.catalog_id, "schema": "s", "table": null, "column": "c" }),
        json!({ "catalog_id": e.catalog_id, "schema": "", "table": null, "column": null }),
        json!({ "catalog_id": e.catalog_id, "schema": "*", "table": null, "column": null }),
        json!({ "catalog_id": e.catalog_id, "schema": "s", "table": "*", "column": null }),
        json!({ "catalog_id": e.catalog_id, "schema": "s", "table": "t", "column": "" }),
        json!({ "catalog_id": Uuid::new_v4(), "schema": "s", "table": null, "column": null }),
    ] {
        let (status, body) = post(
            &e,
            &e.admin,
            "/perm/explain/principal",
            &json!({ "principal": { "kind": "group", "ref": g }, "action": "read", "target": target }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{target}: {body}");
        assert!(body["error"].is_string(), "{body}");
    }
}

#[tokio::test]
async fn the_route_is_gated_on_policy_read() {
    let Some(e) = env().await else { return };
    let g = format!("xq_{}", tag());
    group_with(&e.db.pool, &g, &[]).await;
    let (status, body) = explain_principal(&e, &e.nobody, "group", &g).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = explain_principal(&e, &e.reader, "group", &g).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_group_of_people_with_no_username_is_answered_by_not_evaluated_alone() {
    let Some(e) = env().await else { return };
    let t = tag();
    let p = &e.db.pool;
    let u1 = insert_user(p, &format!("n1_{t}@x.local"), None).await;
    let u2 = insert_user(p, &format!("n2_{t}@x.local"), None).await;
    let g = format!("xn_{t}");
    group_with(p, &g, &[u1, u2]).await;
    let (status, body) = explain_principal(&e, &e.admin, "group", &g).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["membersTotal"], json!(2));
    assert_eq!(body["truncated"], json!(false));
    assert_eq!(body["evaluated"], json!([]));
    let ids: Vec<Value> = body["notEvaluated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["userId"].clone())
        .collect();
    assert_eq!(ids, vec![json!(u1), json!(u2)], "sorted by label");
}

/// `POST /perm/explain/principal` for a COLUMN read of `sales.orders.email`.
async fn explain_principal_column(e: &Env, kind: &str, r: &str) -> Value {
    let mut target = target(e);
    target["column"] = json!("email");
    let (status, body) = post(
        e,
        &e.admin,
        "/perm/explain/principal",
        &json!({ "principal": { "kind": kind, "ref": r }, "action": "read", "target": target }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// One member's single profile, by username.
fn member_profile<'a>(body: &'a Value, username: &str) -> &'a Value {
    let m = body["evaluated"]
        .as_array()
        .expect("evaluated")
        .iter()
        .find(|m| m["username"] == username)
        .unwrap_or_else(|| panic!("{username} not evaluated: {body}"));
    &m["result"]["profiles"][0]
}

fn via(kind: &str, r: &str) -> Value {
    json!({ "principal_kind": kind, "ref": r })
}

/// Spec §5.4 (D1): an exempt verdict says WHO exempts the person. The compiled
/// body names usernames only, so the server rebuilds the reason from the rule's
/// exempt principals and the person's memberships: directly, then groups by
/// name; three kept and the rest counted. (A rule exempts only users and groups:
/// a role or a workspace is refused at write, `validate_rule_shape`.) When the compiled body
/// is older than the model (a membership removed without a recompile) the
/// reason is empty rather than invented.
#[tokio::test]
async fn an_exempt_member_is_told_who_exempts_them() {
    let Some(e) = env().await else { return };
    let t = tag();
    let p = &e.db.pool;
    let user = |n: &str| (format!("{n}_{t}@x.local"), format!("{n}_{t}"));
    let mut ids = std::collections::HashMap::new();
    for n in ["bob", "alice", "carol", "dave", "erin"] {
        let (email, name) = user(n);
        ids.insert(n, insert_user(p, &email, Some(&name)).await);
    }
    let team = format!("data-team-{t}");
    group_with(p, &team, &[ids["bob"], ids["carol"]]).await;
    let five: Vec<String> = (1..=5).map(|i| format!("g{i}-{t}")).collect();
    for g in &five {
        group_with(p, g, &[ids["erin"]]).await;
    }
    let everyone = format!("all-{t}");
    group_with(
        p,
        &everyone,
        &["bob", "alice", "carol", "dave", "erin"].map(|n| ids[n]),
    )
    .await;

    let mut exempt = vec![
        json!({"kind": "group", "ref": team}),
        json!({"kind": "user", "ref": format!("alice_{t}")}),
        json!({"kind": "user", "ref": format!("carol_{t}")}),
    ];
    // Stored in reverse, so the answer's order is the server's, not the rule's.
    exempt.extend(
        five.iter()
            .rev()
            .map(|g| json!({"kind": "group", "ref": g})),
    );
    for (kind, name) in [
        (
            json!({ "type": "mask", "mask_type": "MASK_HASH", "value_expr": null }),
            format!("hash-{t}"),
        ),
        (
            json!({ "type": "row_filter", "filter_expr": "region = 'us'" }),
            format!("us-{t}"),
        ),
    ] {
        let column = if kind["type"] == "mask" {
            json!("email")
        } else {
            Value::Null
        };
        let (status, out) = post(
            &e,
            &e.admin,
            "/perm/rules",
            &json!({
                "name": name, "kind": kind,
                "selector": { "type": "path", "catalog_id": e.catalog_id,
                              "schema": "sales", "table": "orders", "column": column },
                "exempt": { "include": exempt, "exclude": [] }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{out}");
    }

    let body = explain_principal_column(&e, "group", &everyone).await;
    let reason = |n: &str, field: &str| member_profile(&body, &format!("{n}_{t}"))[field].clone();
    let bob = reason("bob", "mask");
    assert_eq!(
        bob["decision"]["result"]["mask_type"], "MASK_NONE",
        "{body}"
    );
    assert_eq!(bob["attribution"]["kind"], "rule", "{body}");
    assert_eq!(
        bob["exemption"],
        json!({"via": [via("group", &team)], "more": 0}),
        "{body}"
    );
    assert_eq!(
        reason("alice", "mask")["exemption"],
        json!({"via": [via("user", &format!("alice_{t}"))], "more": 0}),
        "{body}"
    );
    assert_eq!(
        reason("carol", "mask")["exemption"],
        json!({"via": [via("user", &format!("carol_{t}")), via("group", &team)], "more": 0}),
        "directly before groups; {body}"
    );
    let dave = reason("dave", "mask");
    assert_eq!(
        dave["decision"]["result"]["mask_type"], "MASK_HASH",
        "{body}"
    );
    assert!(dave.get("exemption").is_none(), "{body}");
    assert_eq!(
        reason("erin", "mask")["exemption"],
        json!({"via": five[..3].iter().map(|g| via("group", g)).collect::<Vec<_>>(), "more": 2}),
        "{body}"
    );
    let bob_rows = reason("bob", "row_filter");
    assert_eq!(bob_rows["decision"]["result"]["filter_expr"], "", "{body}");
    assert_eq!(
        bob_rows["exemption"],
        json!({"via": [via("group", &team)], "more": 0}),
        "{body}"
    );
    assert!(
        reason("dave", "row_filter").get("exemption").is_none(),
        "{body}"
    );

    // The model moves without a recompile: bob leaves the group by raw SQL.
    // The served policy still exempts him, and nothing in the model says why.
    sqlx::query(
        "DELETE FROM user_groups WHERE user_id = $1 \
           AND group_id = (SELECT id FROM groups WHERE name = $2)",
    )
    .bind(ids["bob"])
    .bind(&team)
    .execute(p)
    .await
    .unwrap();
    let body = explain_principal_column(&e, "group", &everyone).await;
    let bob = &member_profile(&body, &format!("bob_{t}"))["mask"];
    assert_eq!(
        bob["decision"]["result"]["mask_type"], "MASK_NONE",
        "{body}"
    );
    assert_eq!(bob["exemption"], json!({"via": [], "more": 0}), "{body}");
}
