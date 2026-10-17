//! People & groups Task 2: `GET /perm/people` lists users, groups and roles for
//! Governance, gated `policy:read` (spec §4.2, Q2). Group member counts and
//! role holder counts come from the compiler's own membership reads
//! (`members::group_members_via`, `members::role_holders_via`, spec §4.1).
//!
//! Its own test binary, so `egret-test-db` gives it a private database; tests
//! in it run concurrently, so each one filters by a unique `q` and never
//! assumes another test's rows absent.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test people

use std::collections::BTreeSet;
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

/// A short unique token, lowercase alphanumerics only, so it is a valid
/// email local part, username and group or role name, and a `q` matching
/// only the rows of the test that made it.
fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

struct Env {
    db: Db,
    app: axum::Router,
    /// `platform_admin`.
    admin: String,
    /// A session holding exactly `policy:read`.
    reader: String,
    /// A session holding no permission at all.
    nobody: String,
}

async fn insert_user(pool: &sqlx::PgPool, email: &str, name: &str, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local', $1, $2, $3) \
         RETURNING id",
    )
    .bind(email)
    .bind(name)
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("insert user")
}

async fn insert_role(pool: &sqlx::PgPool, scope: &str, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ($1, $2) RETURNING id")
        .bind(scope)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("insert role")
}

/// One `access_grants` row; `scope_id` is `None` for platform scope.
#[allow(clippy::too_many_arguments)]
async fn insert_grant(
    pool: &sqlx::PgPool,
    principal_type: &str,
    principal_id: Uuid,
    role_id: Uuid,
    scope_type: &str,
    scope_id: Option<Uuid>,
    effect: &str,
    expired: bool,
) {
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, scope_id, effect, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, \
                 CASE WHEN $7 THEN now() - interval '1 hour' ELSE NULL END)",
    )
    .bind(principal_type)
    .bind(principal_id)
    .bind(role_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(effect)
    .bind(expired)
    .execute(pool)
    .await
    .expect("insert access grant");
}

async fn session(db: &Db, sessions: &SessionService, permission: Option<&str>) -> String {
    let t = tag();
    let uid = insert_user(&db.pool, &format!("ppl_gate_{t}@egret.local"), "Gate", None).await;
    if let Some(permission) = permission {
        let role_id = insert_role(&db.pool, "platform", &format!("ppl_gate_role_{t}")).await;
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role_id)
            .bind(permission)
            .execute(&db.pool)
            .await
            .expect("grant permission");
        insert_grant(
            &db.pool, "user", uid, role_id, "platform", None, "allow", false,
        )
        .await;
    }
    sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let reader = session(&db, &sessions, Some("policy:read")).await;
    let nobody = session(&db, &sessions, None).await;
    let admin_id = insert_user(
        &db.pool,
        &format!("ppl_admin_{}@egret.local", tag()),
        "Admin",
        None,
    )
    .await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(admin_id)
    .execute(&db.pool)
    .await
    .expect("grant platform_admin");
    let admin = sessions
        .create(egret_core::UserId(admin_id), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    // Built AFTER every permission above is written: the PDP loads them once.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let app = perm_router().with_state(PermState {
        perm: PermStore::new(db.pool.clone()),
        policy: PolicyStore::new(db.clone()),
        sessions,
        pdp,
    });
    Some(Env {
        db,
        app,
        admin,
        reader,
        nobody,
    })
}

async fn get(e: &Env, token: &str, query: &str) -> (StatusCode, Value) {
    let resp = e
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri(format!("/perm/people?{query}"))
                .header("Cookie", format!("egret_session={token}"))
                .body(Body::empty())
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

async fn page(e: &Env, query: &str) -> Value {
    let (status, body) = get(e, &e.admin, query).await;
    assert_eq!(status, StatusCode::OK, "{query}: {body}");
    body
}

fn keys(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["key"].as_str().expect("key").to_string())
        .collect()
}

#[tokio::test]
async fn users_are_listed_with_and_without_a_username_and_q_matches_name_email_and_username() {
    let Some(e) = env().await else { return };
    let t = tag();
    let alice = insert_user(
        &e.db.pool,
        &format!("alice_{t}@egret.local"),
        "Alice Example",
        Some(&format!("alice_{t}")),
    )
    .await;
    // No name: the label falls back to the email. No username yet.
    let bob = insert_user(&e.db.pool, &format!("bob_{t}@egret.local"), "", None).await;
    // Matched by name only: the email and username do not contain the tag.
    let carol_name = format!("Carol {t}");
    let carol = insert_user(
        &e.db.pool,
        &format!("carol_{}@egret.local", tag()),
        &carol_name,
        None,
    )
    .await;
    // Matched by username only.
    let dave_username = format!("dave_{t}");
    let dave = insert_user(
        &e.db.pool,
        &format!("dave_{}@egret.local", tag()),
        "Dave",
        Some(&dave_username),
    )
    .await;
    // A service account is a `users` row but never a person (migration 0119):
    // a human-facing list excludes it.
    sqlx::query(
        "INSERT INTO users (provider, email, name, username) VALUES ('service', $1, $1, $1)",
    )
    .bind(format!("svc_{t}"))
    .execute(&e.db.pool)
    .await
    .expect("service account");

    let body = page(&e, &format!("kind=user&q={t}")).await;
    assert_eq!(body["total"], json!(4), "{body}");
    let got: BTreeSet<String> = keys(&body).into_iter().collect();
    let want: BTreeSet<String> = [alice, bob, carol, dave]
        .iter()
        .map(|u| u.to_string())
        .collect();
    assert_eq!(got, want, "{body}");

    let item = |id: Uuid| {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["key"] == json!(id.to_string()))
            .cloned()
            .expect("listed")
    };
    assert_eq!(
        item(alice),
        json!({
            "kind": "user",
            "key": alice.to_string(),
            "label": "Alice Example",
            "detail": format!("alice_{t}@egret.local"),
            "username": format!("alice_{t}"),
            "memberCount": null,
        }),
        "exactly these fields — no password, session, permission or login data"
    );
    let b = item(bob);
    assert_eq!(b["label"], json!(format!("bob_{t}@egret.local")));
    assert_eq!(b["username"], Value::Null);
    assert_eq!(item(carol)["label"], json!(carol_name));
    assert_eq!(item(dave)["username"], json!(dave_username));

    // `q` is a substring match, case-insensitive, and a LIKE wildcard in it is
    // a literal character, not a pattern.
    let upper = page(&e, &format!("kind=user&q={}", t.to_uppercase())).await;
    assert_eq!(upper["total"], json!(4), "{upper}");
    let wildcard = page(&e, "kind=user&q=%25_%25").await;
    assert_eq!(wildcard["total"], json!(0), "{wildcard}");
}

#[tokio::test]
async fn username_is_an_exact_case_insensitive_lookup() {
    let Some(e) = env().await else { return };
    let t = tag();
    let erin = insert_user(
        &e.db.pool,
        &format!("erin_{t}@egret.local"),
        "Erin",
        Some(&format!("erin_{t}")),
    )
    .await;
    // A username that merely CONTAINS the other is not an exact match.
    insert_user(
        &e.db.pool,
        &format!("erin2_{t}@egret.local"),
        "Erin Two",
        Some(&format!("erin_{t}-2")),
    )
    .await;

    let body = page(&e, &format!("kind=user&username=ERIN_{}", t.to_uppercase())).await;
    assert_eq!(body["total"], json!(1), "{body}");
    assert_eq!(keys(&body), vec![erin.to_string()]);

    let none = page(&e, &format!("kind=user&username=erin_{}", &t[..6])).await;
    assert_eq!(none["total"], json!(0), "{none}");
    assert_eq!(none["items"], json!([]));
}

#[tokio::test]
async fn groups_are_listed_with_their_source_and_direct_member_count() {
    let Some(e) = env().await else { return };
    let t = tag();
    let local: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(format!("g_local_{t}"))
            .fetch_one(&e.db.pool)
            .await
            .expect("group");
    sqlx::query("INSERT INTO groups (name, source) VALUES ($1, 'oidc')")
        .bind(format!("g_oidc_{t}"))
        .execute(&e.db.pool)
        .await
        .expect("group");
    // Two members, one with no username: a member is a member either way
    // (the compiler mints the username, it does not skip the member).
    for (i, username) in [Some(format!("gm1_{t}")), None].into_iter().enumerate() {
        let uid = insert_user(
            &e.db.pool,
            &format!("gm{i}_{t}@egret.local"),
            "Member",
            username.as_deref(),
        )
        .await;
        sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
            .bind(uid)
            .bind(local)
            .execute(&e.db.pool)
            .await
            .expect("member");
    }

    let body = page(&e, &format!("kind=group&q={t}")).await;
    assert_eq!(body["total"], json!(2), "{body}");
    assert_eq!(
        body["items"],
        json!([
            {
                "kind": "group",
                "key": format!("g_local_{t}"),
                "label": format!("g_local_{t}"),
                "detail": "local",
                "username": null,
                "memberCount": 2,
            },
            {
                "kind": "group",
                "key": format!("g_oidc_{t}"),
                "label": format!("g_oidc_{t}"),
                "detail": "oidc",
                "username": null,
                "memberCount": 0,
            },
        ]),
        "sorted by name"
    );
}

#[tokio::test]
async fn a_role_is_listed_once_per_name_with_every_scope_and_its_live_allow_holders() {
    let Some(e) = env().await else { return };
    let t = tag();
    let name = format!("r_{t}");
    let platform = insert_role(&e.db.pool, "platform", &name).await;
    let workspace = insert_role(&e.db.pool, "workspace", &name).await;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $1) RETURNING id")
            .bind(format!("ppl-ws-{t}"))
            .fetch_one(&e.db.pool)
            .await
            .expect("workspace");
    let user = |n: &str| format!("{n}_{t}@egret.local");
    let u1 = insert_user(&e.db.pool, &user("h1"), "H1", Some(&format!("h1_{t}"))).await;
    // A holder with no username still holds the role; the compiler does not
    // NAME them on engines (members.rs), but they are a holder.
    let u2 = insert_user(&e.db.pool, &user("h2"), "H2", None).await;
    let denied = insert_user(&e.db.pool, &user("h3"), "H3", None).await;
    let expired = insert_user(&e.db.pool, &user("h4"), "H4", None).await;
    let group: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(format!("rg_{t}"))
        .fetch_one(&e.db.pool)
        .await
        .expect("group");
    let p = &e.db.pool;
    insert_grant(p, "user", u1, platform, "platform", None, "allow", false).await;
    // u1 holds it a second time, in the workspace scope: one holder, not two.
    insert_grant(
        p,
        "user",
        u1,
        workspace,
        "workspace",
        Some(ws),
        "allow",
        false,
    )
    .await;
    insert_grant(
        p,
        "user",
        u2,
        workspace,
        "workspace",
        Some(ws),
        "allow",
        false,
    )
    .await;
    // Not holders: a deny, an expired allow, and a group (egret only).
    insert_grant(p, "user", denied, platform, "platform", None, "deny", false).await;
    insert_grant(
        p, "user", expired, platform, "platform", None, "allow", true,
    )
    .await;
    insert_grant(
        p, "group", group, platform, "platform", None, "allow", false,
    )
    .await;
    // Another role, no holders.
    insert_role(p, "workspace", &format!("r_zz_empty_{t}")).await;

    let body = page(&e, &format!("kind=role&q={t}")).await;
    assert_eq!(body["total"], json!(2), "{body}");
    assert_eq!(
        body["items"],
        json!([
            {
                "kind": "role",
                "key": name,
                "label": name,
                "detail": "platform, workspace",
                "username": null,
                "memberCount": 2,
            },
            {
                "kind": "role",
                "key": format!("r_zz_empty_{t}"),
                "label": format!("r_zz_empty_{t}"),
                "detail": "workspace",
                "username": null,
                "memberCount": 0,
            },
        ])
    );
}

#[tokio::test]
async fn limit_and_offset_page_the_list_and_total_counts_every_match() {
    let Some(e) = env().await else { return };
    let t = tag();
    // 205 users: more than the default (50) and the maximum (200).
    sqlx::query(
        "INSERT INTO users (provider, email, name) \
         SELECT 'local', format('pg%s_%s@egret.local', lpad(i::text, 3, '0'), $1::text), \
                format('Page %s %s', lpad(i::text, 3, '0'), $1::text) \
           FROM generate_series(1, 205) AS i",
    )
    .bind(&t)
    .execute(&e.db.pool)
    .await
    .expect("users");

    let default = page(&e, &format!("kind=user&q={t}")).await;
    assert_eq!(default["total"], json!(205));
    assert_eq!(
        default["items"].as_array().unwrap().len(),
        50,
        "default limit"
    );

    let capped = page(&e, &format!("kind=user&q={t}&limit=1000")).await;
    assert_eq!(capped["total"], json!(205));
    assert_eq!(capped["items"].as_array().unwrap().len(), 200, "max limit");

    let first = page(&e, &format!("kind=user&q={t}&limit=3&offset=0")).await;
    let second = page(&e, &format!("kind=user&q={t}&limit=3&offset=3")).await;
    let labels = |b: &Value| -> Vec<String> {
        b["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["label"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        labels(&first),
        (1..=3)
            .map(|i| format!("Page {i:03} {t}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        labels(&second),
        (4..=6)
            .map(|i| format!("Page {i:03} {t}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(second["total"], json!(205));

    let past = page(&e, &format!("kind=user&q={t}&offset=300")).await;
    assert_eq!(past["items"], json!([]));
    assert_eq!(past["total"], json!(205));
}

#[tokio::test]
async fn a_kind_outside_user_group_role_is_a_400() {
    let Some(e) = env().await else { return };
    for query in [
        "kind=workspace_members",
        "kind=public",
        "kind=",
        "",
        "kind=user&limit=-1",
        "kind=user&offset=-1",
    ] {
        let (status, body) = get(&e, &e.admin, query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query:?}: {body}");
        assert!(body["error"].is_string(), "{query:?}: a sentence: {body}");
    }
}

#[tokio::test]
async fn the_list_is_gated_on_policy_read() {
    let Some(e) = env().await else { return };
    for kind in ["user", "group", "role"] {
        let (status, body) = get(&e, &e.nobody, &format!("kind={kind}")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{kind}: {body}");
        let (status, body) = get(&e, &e.reader, &format!("kind={kind}")).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{kind}: policy:read alone suffices: {body}"
        );
    }
    let (status, _) = get(&e, "not-a-session", "kind=user").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
