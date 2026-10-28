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
    // A whitespace-only name is no name: the label falls back to the email.
    // No username yet.
    let bob = insert_user(&e.db.pool, &format!("bob_{t}@egret.local"), " \t ", None).await;
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
    // A service account (migration 0119) is left out of browsing by default;
    // `a_service_account_is_found_by_username_listed_on_request_and_marked`
    // covers the other half.
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
            "withoutUsername": null,
            "serviceAccount": false,
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
                "withoutUsername": 1,
                "serviceAccount": false,
            },
            {
                "kind": "group",
                "key": format!("g_oidc_{t}"),
                "label": format!("g_oidc_{t}"),
                "detail": "oidc",
                "username": null,
                "memberCount": 0,
                "withoutUsername": 0,
                "serviceAccount": false,
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
    // A holder with no username still holds the role; the compiler names
    // them on engines only once a compile mints one (members.rs), but they
    // are a holder either way.
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
                "withoutUsername": 1,
                "serviceAccount": false,
            },
            {
                "kind": "role",
                "key": format!("r_zz_empty_{t}"),
                "label": format!("r_zz_empty_{t}"),
                "detail": "workspace",
                "username": null,
                "memberCount": 0,
                "withoutUsername": 0,
                "serviceAccount": false,
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
        "kind=group&username=x",
        "kind=role&username=x",
        "kind=group&includeServiceAccounts=true",
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

#[tokio::test]
async fn a_service_account_is_found_by_username_listed_on_request_and_marked() {
    let Some(e) = env().await else { return };
    let t = tag();
    let svc: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('service', $1, $2, $3) RETURNING id",
    )
    .bind(format!("svc_{t}@service.local"))
    .bind(format!("Invoker {t}"))
    .bind(format!("svc_{t}"))
    .fetch_one(&e.db.pool)
    .await
    .expect("service account");
    let human = insert_user(&e.db.pool, &format!("hum_{t}@egret.local"), "Human", None).await;

    let marked = json!({
        "kind": "user",
        "key": svc.to_string(),
        "label": format!("Invoker {t}"),
        "detail": "service account",
        "username": format!("svc_{t}"),
        "memberCount": null,
        "withoutUsername": null,
        "serviceAccount": true,
    });

    // Browsing leaves it out by default, and `total` agrees with the items.
    let browse = page(&e, &format!("kind=user&q={t}")).await;
    assert_eq!(keys(&browse), vec![human.to_string()], "{browse}");
    assert_eq!(browse["total"], json!(1));

    // On request it is listed, marked.
    let all = page(&e, &format!("kind=user&q={t}&includeServiceAccounts=true")).await;
    assert_eq!(all["total"], json!(2), "{all}");
    let row = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["key"] == json!(svc.to_string()))
        .cloned()
        .expect("listed with the flag");
    assert_eq!(row, marked);

    // A username lookup always finds it, without the flag and with it false.
    for query in [
        format!("kind=user&username=SVC_{}", t.to_uppercase()),
        format!("kind=user&username=svc_{t}&includeServiceAccounts=false"),
    ] {
        let found = page(&e, &query).await;
        assert_eq!(found["total"], json!(1), "{query}: {found}");
        assert_eq!(found["items"], json!([marked.clone()]), "{query}");
    }

    // A role it holds counts it, and the Users tab can now show every holder
    // that count stands for.
    let role: Uuid = insert_role(&e.db.pool, "platform", &format!("svcrole_{t}")).await;
    insert_grant(
        &e.db.pool, "user", svc, role, "platform", None, "allow", false,
    )
    .await;
    let roles = page(&e, &format!("kind=role&q=svcrole_{t}")).await;
    assert_eq!(roles["items"][0]["memberCount"], json!(1), "{roles}");
    assert_eq!(roles["items"][0]["withoutUsername"], json!(0), "{roles}");
}

// ---------------------------------------------------------------------------
// Task 3: one principal's memberships, `standsFor` and clearances
// ---------------------------------------------------------------------------

async fn get_path(e: &Env, token: &str, path: &str) -> (StatusCode, Value) {
    let resp = e
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri(path)
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

async fn principal(e: &Env, path: &str) -> Value {
    let (status, body) = get_path(e, &e.admin, path).await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}

async fn insert_group(pool: &sqlx::PgPool, name: &str, members: &[Uuid]) -> Uuid {
    let id: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("group");
    for m in members {
        sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
            .bind(m)
            .bind(id)
            .execute(pool)
            .await
            .expect("member");
    }
    id
}

/// The fixture of the agreement test and the per-`Via` assertions:
///
/// - `ua` (username) in `g1`; holds `r1` live; `rw` in `W` EXPIRED.
/// - `ub` (NO username) in `g1`; holds `r1` live.
/// - `uc` (username) in `g2`; `g2` holds `r1` (egret only).
/// - `ud` (username) holds `rw` in `W` live.
/// - `ue` (username) holds `r1` EXPIRED only.
/// - `g1` holds `rw` in `W` (egret only).
struct Org {
    t: String,
    ws: Uuid,
    ua: Uuid,
    ub: Uuid,
    uc: Uuid,
    ud: Uuid,
    ue: Uuid,
    g1: Uuid,
    g2: Uuid,
}

impl Org {
    fn n(&self, s: &str) -> String {
        format!("{s}_{}", self.t)
    }
}

async fn org(e: &Env) -> Org {
    let t = tag();
    let p = &e.db.pool;
    let email = |n: &str| format!("{n}_{t}@egret.local");
    let uname = |n: &str| format!("{n}_{t}");
    let ua = insert_user(p, &email("ua"), "User A", Some(&uname("ua"))).await;
    let ub = insert_user(p, &email("ub"), "User B", None).await;
    let uc = insert_user(p, &email("uc"), "User C", Some(&uname("uc"))).await;
    let ud = insert_user(p, &email("ud"), "User D", Some(&uname("ud"))).await;
    let ue = insert_user(p, &email("ue"), "User E", Some(&uname("ue"))).await;
    let g1 = insert_group(p, &uname("g1"), &[ua, ub]).await;
    let g2 = insert_group(p, &uname("g2"), &[uc]).await;
    let r1 = insert_role(p, "platform", &uname("r1")).await;
    let rw = insert_role(p, "workspace", &uname("rw")).await;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("ppl-w-{t}"))
            .bind(format!("W {t}"))
            .fetch_one(p)
            .await
            .expect("workspace");
    insert_grant(p, "user", ua, r1, "platform", None, "allow", false).await;
    insert_grant(p, "user", ub, r1, "platform", None, "allow", false).await;
    insert_grant(p, "user", ue, r1, "platform", None, "allow", true).await;
    insert_grant(p, "group", g2, r1, "platform", None, "allow", false).await;
    insert_grant(p, "user", ud, rw, "workspace", Some(ws), "allow", false).await;
    insert_grant(p, "user", ua, rw, "workspace", Some(ws), "allow", true).await;
    insert_grant(p, "group", g1, rw, "workspace", Some(ws), "allow", false).await;
    // A deny-effect grant is no membership, on engines or in egret.
    insert_grant(p, "user", ud, r1, "platform", None, "deny", false).await;
    Org {
        t,
        ws,
        ua,
        ub,
        uc,
        ud,
        ue,
        g1,
        g2,
    }
}

fn stands_for(body: &Value) -> Vec<Value> {
    body["standsFor"].as_array().expect("standsFor").clone()
}

/// The `standsFor` entry for `(kind, ref)` reached `via`.
fn entry(body: &Value, kind: &str, r: &str, via: &Value) -> Value {
    stands_for(body)
        .into_iter()
        .find(|s| s["kind"] == json!(kind) && s["ref"] == json!(r) && &s["via"] == via)
        .unwrap_or_else(|| panic!("no {kind}:{r} via {via} in {body}"))
}

#[tokio::test]
async fn a_users_stands_for_marks_each_via_and_why_engines_do_not_apply_it() {
    let Some(e) = env().await else { return };
    let o = org(&e).await;

    let ua = principal(&e, &format!("/perm/people/users/{}", o.ua)).await;
    let on = |s: Value| (s["onEngines"].clone(), s["notOnEngines"].clone());
    assert_eq!(
        on(entry(&ua, "user", &o.n("ua"), &json!({"kind": "itself"}))),
        (json!(true), Value::Null)
    );
    assert_eq!(
        on(entry(
            &ua,
            "group",
            &o.n("g1"),
            &json!({"kind": "group", "name": o.n("g1")})
        )),
        (json!(true), Value::Null)
    );
    assert_eq!(
        on(entry(
            &ua,
            "role",
            &o.n("r1"),
            &json!({"kind": "role", "name": o.n("r1")})
        )),
        (json!(true), Value::Null)
    );
    assert_eq!(
        on(entry(&ua, "group", "public", &json!({"kind": "public"}))),
        (json!(true), Value::Null)
    );
    let w = json!({"kind": "workspace", "id": o.ws, "name": format!("W {}", o.t)});
    assert_eq!(
        on(entry(&ua, "workspace_members", &o.ws.to_string(), &w)),
        (json!(false), json!("expired")),
        "an expired direct row is listed, marked"
    );
    let gw = json!({"kind": "group_workspace", "group": o.n("g1"), "id": o.ws, "name": format!("W {}", o.t)});
    assert_eq!(
        on(entry(&ua, "workspace_members", &o.ws.to_string(), &gw)),
        (json!(false), json!("held_through_group"))
    );

    let uc = principal(&e, &format!("/perm/people/users/{}", o.uc)).await;
    let gr = json!({"kind": "group_role", "group": o.n("g2"), "role": o.n("r1")});
    // A role held through a group reaches engines by the members' usernames.
    assert_eq!(
        on(entry(&uc, "role", &o.n("r1"), &gr)),
        (json!(true), Value::Null)
    );

    // No username: no `user` entry, and every entry is `no_username`.
    let ub = principal(&e, &format!("/perm/people/users/{}", o.ub)).await;
    assert_eq!(ub["username"], Value::Null);
    let entries = stands_for(&ub);
    assert!(!entries.iter().any(|s| s["kind"] == json!("user")), "{ub}");
    assert!(
        entries.len() >= 4,
        "group, role, public, group workspace: {ub}"
    );
    for s in &entries {
        assert_eq!(s["onEngines"], json!(false), "{s}");
        assert_eq!(s["notOnEngines"], json!("no_username"), "{s}");
    }

    // The deny-effect grant makes `ud` no holder of r1.
    let ud = principal(&e, &format!("/perm/people/users/{}", o.ud)).await;
    assert!(
        !stands_for(&ud).iter().any(|s| s["ref"] == json!(o.n("r1"))),
        "{ud}"
    );
    assert_eq!(
        on(entry(&ud, "workspace_members", &o.ws.to_string(), &w)),
        (json!(true), Value::Null)
    );
}

#[tokio::test]
async fn a_users_memberships_carry_scope_expiry_and_engine_marks() {
    let Some(e) = env().await else { return };
    let o = org(&e).await;
    let ua = principal(&e, &format!("/perm/people/users/{}", o.ua)).await;

    assert_eq!(ua["id"], json!(o.ua.to_string()));
    assert_eq!(ua["name"], json!("User A"));
    assert_eq!(ua["username"], json!(o.n("ua")));
    assert_eq!(ua["status"], json!("active"));
    assert_eq!(
        ua["groups"],
        json!([{
            "via": {"kind": "group", "name": o.n("g1")},
            "role": null, "scope": null, "expiresAt": null, "expired": false,
            "onEngines": true, "notOnEngines": null,
        }])
    );
    let roles = ua["roles"].as_array().unwrap();
    let r1 = roles
        .iter()
        .find(|m| m["via"] == json!({"kind": "role", "name": o.n("r1")}))
        .expect("r1");
    assert_eq!(r1["scope"], json!({"type": "platform"}));
    assert_eq!(r1["onEngines"], json!(true));
    let rw = roles
        .iter()
        .find(|m| m["via"] == json!({"kind": "role", "name": o.n("rw")}))
        .expect("rw, expired, still listed");
    assert_eq!(rw["expired"], json!(true));
    assert!(rw["expiresAt"].is_string());
    assert_eq!(rw["notOnEngines"], json!("expired"));
    assert_eq!(
        rw["scope"],
        json!({"type": "workspace", "id": o.ws, "name": format!("W {}", o.t)})
    );
    // g1's workspace membership, egret only.
    let gws = ua["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["via"]["kind"] == json!("group_workspace"))
        .cloned()
        .expect("group workspace");
    assert_eq!(gws["role"], json!(o.n("rw")));
    assert_eq!(gws["notOnEngines"], json!("held_through_group"));
}

#[tokio::test]
async fn a_group_lists_members_with_and_without_a_username_and_what_it_holds() {
    let Some(e) = env().await else { return };
    let o = org(&e).await;
    let svc: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('service', $1, $1, $1) RETURNING id",
    )
    .bind(o.n("svc"))
    .fetch_one(&e.db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(svc)
        .bind(o.g1)
        .execute(&e.db.pool)
        .await
        .unwrap();

    let g1 = principal(&e, &format!("/perm/people/groups/{}", o.n("g1"))).await;
    assert_eq!(g1["name"], json!(o.n("g1")));
    assert_eq!(g1["source"], json!("local"));
    assert_eq!(g1["isPublic"], json!(false));
    assert_eq!(g1["serviceAccounts"], json!(1));
    let members = g1["members"].as_array().unwrap();
    assert_eq!(members.len(), 3, "{g1}");
    let m = |id: Uuid| {
        members
            .iter()
            .find(|m| m["userId"] == json!(id.to_string()))
            .cloned()
            .unwrap()
    };
    assert_eq!(m(o.ua)["username"], json!(o.n("ua")));
    assert_eq!(m(o.ua)["onEngines"], json!(true));
    assert_eq!(m(o.ub)["username"], Value::Null);
    assert_eq!(m(o.ub)["onEngines"], json!(false));
    assert_eq!(m(o.ub)["notOnEngines"], json!("no_username"));
    assert_eq!(
        m(svc)["serviceAccount"],
        json!(true),
        "service accounts are not filtered out"
    );

    let w = json!({"kind": "workspace", "id": o.ws, "name": format!("W {}", o.t)});
    assert_eq!(g1["workspaces"][0]["via"], w);
    assert_eq!(
        g1["workspaces"][0]["notOnEngines"],
        json!("held_through_group")
    );
    let sf = |b: &Value| -> BTreeSet<(String, String, String)> {
        stands_for(b)
            .iter()
            .map(|s| {
                (
                    s["kind"].as_str().unwrap().to_string(),
                    s["ref"].as_str().unwrap().to_string(),
                    s["notOnEngines"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect()
    };
    assert_eq!(
        sf(&g1),
        [
            ("group".to_string(), o.n("g1"), String::new()),
            ("group".to_string(), "public".to_string(), String::new()),
            ("role".to_string(), o.n("rw"), String::new()),
            (
                "workspace_members".to_string(),
                o.ws.to_string(),
                "held_through_group".to_string()
            ),
        ]
        .into_iter()
        .collect()
    );
    let g2 = principal(&e, &format!("/perm/people/groups/{}", o.n("g2"))).await;
    assert!(sf(&g2).contains(&("role".to_string(), o.n("r1"), String::new())));
    let _ = o.g2;
}

#[tokio::test]
async fn a_role_lists_holders_with_and_without_a_username_and_its_holder_groups() {
    let Some(e) = env().await else { return };
    let o = org(&e).await;
    let r1 = principal(&e, &format!("/perm/people/roles/{}", o.n("r1"))).await;
    assert_eq!(r1["name"], json!(o.n("r1")));
    assert_eq!(r1["scopes"], json!(["platform"]));
    let holders: BTreeSet<(String, bool)> = r1["holders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["userId"].as_str().unwrap().to_string(),
                h["onEngines"] == json!(true),
            )
        })
        .collect();
    // ue's is expired, ud's is a deny, uc holds it through g2: not holders.
    assert_eq!(
        holders,
        [(o.ua.to_string(), true), (o.ub.to_string(), false)]
            .into_iter()
            .collect()
    );
    assert_eq!(r1["serviceAccounts"], json!(0));
    assert_eq!(
        r1["holderGroups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["name"].clone())
            .collect::<Vec<_>>(),
        vec![json!(o.n("g2"))]
    );
    assert_eq!(
        r1["standsFor"],
        json!([{ "kind": "role", "ref": o.n("r1"), "via": {"kind": "itself"},
                 "onEngines": true, "notOnEngines": null }])
    );
    let _ = (o.uc, o.ud, o.ue);
}

async fn gate(pool: &sqlx::PgPool, key: &str, roles: &[String], groups: &[Uuid]) -> Uuid {
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, $3)",
    )
    .bind(id)
    .bind(roles)
    .bind(groups)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn clearance(body: &Value, tag_id: Uuid) -> Value {
    body["clearances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["tagId"] == json!(tag_id))
        .cloned()
        .unwrap_or_else(|| panic!("no clearance for {tag_id}: {body}"))
}

#[tokio::test]
async fn clearances_answer_for_engines_and_for_egret_per_gate() {
    let Some(e) = env().await else { return };
    let o = org(&e).await;
    let p = &e.db.pool;
    // roles-only on r1; groups-only on g1; mixed: a role nobody holds, or g2.
    let a = gate(p, &o.n("cla"), &[o.n("r1")], &[]).await;
    let b = gate(p, &o.n("clb"), &[], &[o.g1]).await;
    let c = gate(p, &o.n("clc"), &[o.n("rx")], &[o.g2]).await;

    let user = |u: Uuid| format!("/perm/people/users/{u}");
    let both = |body: &Value, tag: Uuid| {
        let c = clearance(body, tag);
        (c["engines"].clone(), c["egret"].clone())
    };
    let not_cleared = json!({"kind": "not_cleared"});
    let engines_no = |because: Value| json!({"kind": "not_cleared", "because": because});
    let groups_only = json!({"kind": "not_enforced_groups_only"});

    // ua: cleared directly.
    let ua = principal(&e, &user(o.ua)).await;
    let ca = clearance(&ua, a);
    assert_eq!(ca["tagKey"], json!(o.n("cla")));
    assert_eq!(ca["requiredRoles"], json!([o.n("r1")]));
    assert_eq!(ca["requiredGroups"], json!([]));
    assert_eq!(
        both(&ua, a),
        (
            json!({"kind": "cleared", "role": o.n("r1")}),
            json!({"kind": "cleared", "via": {"kind": "role", "name": o.n("r1")}, "scope": {"type": "platform"}})
        )
    );
    assert_eq!(
        clearance(&ua, b)["requiredGroups"],
        json!([o.n("g1")]),
        "group NAMES"
    );
    assert_eq!(
        both(&ua, b),
        (
            groups_only.clone(),
            json!({"kind": "cleared", "via": {"kind": "group", "name": o.n("g1")}, "scope": null})
        )
    );
    assert_eq!(both(&ua, c), (engines_no(Value::Null), not_cleared.clone()));

    // uc: cleared in egret through its group, and on engines the same way.
    let uc = principal(&e, &user(o.uc)).await;
    assert_eq!(
        both(&uc, a),
        (
            json!({"kind": "cleared", "role": o.n("r1")}),
            json!({"kind": "cleared", "via": {"kind": "group_role", "group": o.n("g2"), "role": o.n("r1")}, "scope": {"type": "platform"}})
        )
    );
    assert_eq!(both(&uc, b), (groups_only.clone(), not_cleared.clone()));
    // Cleared in egret through a required group; engines enforce only the
    // gate's role half, and say so.
    assert_eq!(
        both(&uc, c),
        (
            engines_no(json!("groups_not_enforced")),
            json!({"kind": "cleared", "via": {"kind": "group", "name": o.n("g2")}, "scope": null})
        )
    );

    // ud: cleared nowhere (its r1 grant is a deny).
    let ud = principal(&e, &user(o.ud)).await;
    assert_eq!(both(&ud, a), (engines_no(Value::Null), not_cleared.clone()));
    assert_eq!(both(&ud, b), (groups_only.clone(), not_cleared.clone()));
    assert_eq!(both(&ud, c), (engines_no(Value::Null), not_cleared.clone()));

    // ub holds r1 but has no username; ue's r1 is expired.
    let ub = principal(&e, &user(o.ub)).await;
    assert_eq!(both(&ub, a).0, engines_no(json!("no_username")));
    let ue = principal(&e, &user(o.ue)).await;
    assert_eq!(
        both(&ue, a),
        (engines_no(json!("expired")), not_cleared.clone())
    );

    // A group and a role answer for themselves.
    let g2 = principal(&e, &format!("/perm/people/groups/{}", o.n("g2"))).await;
    assert_eq!(
        both(&g2, a),
        (
            json!({"kind": "cleared", "role": o.n("r1")}),
            json!({"kind": "cleared", "via": {"kind": "role", "name": o.n("r1")}, "scope": {"type": "platform"}})
        )
    );
    assert_eq!(
        both(&g2, c),
        (
            engines_no(json!("groups_not_enforced")),
            json!({"kind": "cleared", "via": {"kind": "itself"}, "scope": null})
        )
    );
    let r1 = principal(&e, &format!("/perm/people/roles/{}", o.n("r1"))).await;
    assert_eq!(
        both(&r1, a),
        (
            json!({"kind": "cleared", "role": o.n("r1")}),
            json!({"kind": "cleared", "via": {"kind": "itself"}, "scope": null})
        )
    );
    assert_eq!(both(&r1, b), (groups_only, not_cleared));
}

#[tokio::test]
async fn public_unknown_names_and_ids() {
    let Some(e) = env().await else { return };
    let public = principal(&e, "/perm/people/groups/public").await;
    assert_eq!(public["isPublic"], json!(true));
    assert_eq!(public["members"], json!([]));
    assert_eq!(
        public["standsFor"],
        json!([{ "kind": "group", "ref": "public", "via": {"kind": "public"},
                 "onEngines": true, "notOnEngines": null }])
    );

    let t = tag();
    for path in [
        format!("/perm/people/groups/nogroup_{t}"),
        format!("/perm/people/roles/norole_{t}"),
        format!("/perm/people/users/{}", Uuid::new_v4()),
    ] {
        let (status, body) = get_path(&e, &e.admin, &path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
        let msg = body["error"].as_str().expect("a sentence");
        assert!(
            msg.len() > "not found".len() && msg.ends_with('.'),
            "{path}: {msg}"
        );
    }
}

#[tokio::test]
async fn principal_reads_are_gated_on_policy_read_and_reveal_only_identity_and_membership() {
    let Some(e) = env().await else { return };
    let o = org(&e).await;
    let paths = [
        format!("/perm/people/users/{}", o.ua),
        format!("/perm/people/groups/{}", o.n("g1")),
        format!("/perm/people/roles/{}", o.n("r1")),
    ];
    for path in &paths {
        let (status, body) = get_path(&e, &e.nobody, path).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
        let (status, body) = get_path(&e, &e.reader, path).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{path}: policy:read suffices: {body}"
        );
    }
    let keys = |v: &Value| -> BTreeSet<String> { v.as_object().unwrap().keys().cloned().collect() };
    let set = |ks: &[&str]| -> BTreeSet<String> { ks.iter().map(|s| s.to_string()).collect() };
    let (_, user) = get_path(&e, &e.reader, &paths[0]).await;
    assert_eq!(
        keys(&user),
        set(&[
            "id",
            "email",
            "name",
            "username",
            "status",
            "serviceAccount",
            "groups",
            "roles",
            "workspaces",
            "standsFor",
            "clearances"
        ])
    );
    let (_, group) = get_path(&e, &e.reader, &paths[1]).await;
    assert_eq!(
        keys(&group),
        set(&[
            "name",
            "source",
            "isPublic",
            "members",
            "serviceAccounts",
            "roles",
            "workspaces",
            "standsFor",
            "clearances"
        ])
    );
    for m in group["members"].as_array().unwrap() {
        assert_eq!(
            keys(m),
            set(&[
                "userId",
                "label",
                "username",
                "serviceAccount",
                "onEngines",
                "notOnEngines"
            ])
        );
    }
    let (_, role) = get_path(&e, &e.reader, &paths[2]).await;
    assert_eq!(
        keys(&role),
        set(&[
            "name",
            "scopes",
            "holders",
            "serviceAccounts",
            "holderGroups",
            "standsFor",
            "clearances"
        ])
    );
    let text = format!("{user}{group}{role}");
    for leak in [
        "password",
        "session",
        "permission",
        "token",
        "lastLogin",
        "last_login",
    ] {
        assert!(!text.contains(leak), "{leak} in a response");
    }
}

#[tokio::test]
async fn platform_admin_clears_in_egret_only_when_held_at_platform_scope() {
    let Some(e) = env().await else { return };
    let o = org(&e).await;
    let p = &e.db.pool;
    let a = gate(p, &o.n("cladm"), &[o.n("r1")], &[]).await;
    let admin_role: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .fetch_one(p)
    .await
    .expect("platform_admin");
    // A workspace-scope role that merely has the name, held in W: no bypass
    // (`egret_auth::pdp`: `is_platform_admin` needs a platform-scoped grant).
    let ws_admin: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name) VALUES ('workspace', 'platform_admin') \
         ON CONFLICT (scope, name) DO UPDATE SET name = EXCLUDED.name RETURNING id",
    )
    .fetch_one(p)
    .await
    .expect("workspace platform_admin");

    let boss = insert_user(
        p,
        &format!("boss_{}@egret.local", o.t),
        "Boss",
        Some(&o.n("boss")),
    )
    .await;
    insert_grant(
        p, "user", boss, admin_role, "platform", None, "allow", false,
    )
    .await;
    let pretender = insert_user(
        p,
        &format!("pre_{}@egret.local", o.t),
        "Pre",
        Some(&o.n("pre")),
    )
    .await;
    insert_grant(
        p,
        "user",
        pretender,
        ws_admin,
        "workspace",
        Some(o.ws),
        "allow",
        false,
    )
    .await;

    let body = principal(&e, &format!("/perm/people/users/{boss}")).await;
    let c = clearance(&body, a);
    assert_eq!(
        c["egret"],
        json!({"kind": "cleared", "via": {"kind": "role", "name": "platform_admin"}, "scope": {"type": "platform"}})
    );
    assert_eq!(
        c["engines"],
        json!({"kind": "not_cleared", "because": null}),
        "engines have no bypass"
    );

    let body = principal(&e, &format!("/perm/people/users/{pretender}")).await;
    assert_eq!(clearance(&body, a)["egret"], json!({"kind": "not_cleared"}));
}

// ---------------------------------------------------------------------------
// Classification UX Task 4: who a clearance admits
// ---------------------------------------------------------------------------

async fn insert_tag_with_clearance(pool: &sqlx::PgPool, roles: &[String], groups: &[Uuid]) -> Uuid {
    let t = tag();
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(format!("clr-{t}"))
        .bind(format!("Clr {t}"))
        .fetch_one(pool)
        .await
        .expect("tag");
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, $3)",
    )
    .bind(id)
    .bind(roles)
    .bind(groups)
    .execute(pool)
    .await
    .expect("clearance");
    id
}

/// A `via` entry for a role grant.
fn role_via(name: &str, scope: &str, scope_id: Option<Uuid>, through: Option<&str>) -> Value {
    json!({"kind": "role", "name": name, "scope": scope, "scopeId": scope_id,
           "throughGroup": through})
}

/// A `via` entry for a group membership.
fn group_via(name: &str) -> Value {
    json!({"kind": "group", "name": name, "scope": null, "scopeId": null, "throughGroup": null})
}

/// Spec C9, a MIXED gate (a role and a group), every population at once: a
/// direct role holder (engines and egret), a holder through a group (egret
/// only), a group member (engines refuse: the gate names a role), each person
/// once; an expired grant and a deny grant not counted.
#[tokio::test]
async fn cleared_people_lists_each_population_of_a_mixed_gate_once() {
    let Some(e) = env().await else { return };
    let p = &e.db.pool;
    let t = tag();
    let both = insert_user(
        p,
        &format!("clr_both_{t}@egret.local"),
        "Both",
        Some(&format!("clr_both_{t}")),
    )
    .await;
    let role_only = insert_user(
        p,
        &format!("clr_role_{t}@egret.local"),
        "Role only",
        Some(&format!("clr_role_{t}")),
    )
    .await;
    let group_only = insert_user(p, &format!("clr_grp_{t}@egret.local"), "Group only", None).await;
    let expired = insert_user(
        p,
        &format!("clr_exp_{t}@egret.local"),
        "Expired",
        Some(&format!("clr_exp_{t}")),
    )
    .await;
    let denied = insert_user(
        p,
        &format!("clr_deny_{t}@egret.local"),
        "Denied",
        Some(&format!("clr_deny_{t}")),
    )
    .await;
    let through_group = insert_user(
        p,
        &format!("clr_thru_{t}@egret.local"),
        "Through group",
        Some(&format!("clr_thru_{t}")),
    )
    .await;
    let lapsed_through = insert_user(
        p,
        &format!("clr_lthru_{t}@egret.local"),
        "Lapsed through group",
        Some(&format!("clr_lthru_{t}")),
    )
    .await;
    let role_name = format!("clr_role_{t}");
    let role = insert_role(p, "platform", &role_name).await;
    insert_grant(p, "user", both, role, "platform", None, "allow", false).await;
    insert_grant(p, "user", role_only, role, "platform", None, "allow", false).await;
    insert_grant(p, "user", expired, role, "platform", None, "allow", true).await;
    insert_grant(p, "user", denied, role, "platform", None, "deny", false).await;
    let holder_name = format!("clr_holder_{t}");
    let holder_group = insert_group(p, &holder_name, &[through_group]).await;
    insert_grant(
        p,
        "group",
        holder_group,
        role,
        "platform",
        None,
        "allow",
        false,
    )
    .await;
    // A group whose grant of the role has expired, and one with a deny: no
    // egret clearance through either.
    let lapsed_group = insert_group(p, &format!("clr_lapsed_{t}"), &[lapsed_through]).await;
    insert_grant(
        p,
        "group",
        lapsed_group,
        role,
        "platform",
        None,
        "allow",
        true,
    )
    .await;
    let deny_group = insert_group(p, &format!("clr_denyg_{t}"), &[lapsed_through]).await;
    insert_grant(
        p, "group", deny_group, role, "platform", None, "deny", false,
    )
    .await;
    let group_name = format!("clr_group_{t}");
    let group = insert_group(p, &group_name, &[both, group_only]).await;
    let tag_id = insert_tag_with_clearance(p, std::slice::from_ref(&role_name), &[group]).await;

    let body = principal(&e, &format!("/perm/clearances/{tag_id}/people")).await;
    assert_eq!(body["count"], 4, "{body}");
    assert_eq!(body["truncated"], false);
    assert_eq!(body["enginesEnforce"], true);
    assert_eq!(body["onEnginesCount"], 2, "Both and Role only: {body}");
    let people = body["people"].as_array().unwrap();
    let labels: Vec<&str> = people
        .iter()
        .map(|p| p["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        vec!["Both", "Group only", "Role only", "Through group"],
        "sorted by label: {body}"
    );
    let both_row = &people[0];
    assert_eq!(both_row["userId"], json!(both));
    assert_eq!(
        both_row["via"],
        json!([
            role_via(&role_name, "platform", None, None),
            group_via(&group_name)
        ]),
        "{both_row}"
    );
    assert_eq!(both_row["onEngines"], true);
    assert_eq!(both_row["username"], json!(format!("clr_both_{t}")));
    assert_eq!(
        people[1]["onEngines"], false,
        "a group member is refused on engines when the gate names a role"
    );
    assert!(people[1]["username"].is_null());
    assert_eq!(people[1]["via"], json!([group_via(&group_name)]));
    assert_eq!(people[2]["onEngines"], true);
    assert_eq!(
        people[2]["via"],
        json!([role_via(&role_name, "platform", None, None)])
    );
    let thru = &people[3];
    assert_eq!(thru["userId"], json!(through_group));
    assert_eq!(
        thru["via"],
        json!([role_via(&role_name, "platform", None, Some(&holder_name))]),
        "{thru}"
    );
    assert_eq!(
        thru["onEngines"], false,
        "cleared in egret, refused on engines"
    );
}

/// A gate naming only groups compiles to nothing on engines: nobody is
/// withheld there, so `enginesEnforce` is false and no member is `onEngines`.
#[tokio::test]
async fn cleared_people_group_only_gate_is_not_enforced_on_engines() {
    let Some(e) = env().await else { return };
    let p = &e.db.pool;
    let t = tag();
    let member = insert_user(
        p,
        &format!("clr_go_{t}@egret.local"),
        "Member",
        Some(&format!("clr_go_{t}")),
    )
    .await;
    let group_name = format!("clr_go_group_{t}");
    let group = insert_group(p, &group_name, &[member]).await;
    let tag_id = insert_tag_with_clearance(p, &[], &[group]).await;
    let body = principal(&e, &format!("/perm/clearances/{tag_id}/people")).await;
    assert_eq!(body["enginesEnforce"], false, "{body}");
    assert_eq!(body["count"], 1);
    assert_eq!(body["onEnginesCount"], 0);
    assert_eq!(body["people"][0]["onEngines"], false);
    assert_eq!(body["people"][0]["via"], json!([group_via(&group_name)]));
}

#[tokio::test]
async fn cleared_people_is_gated_and_answers_404_and_empty() {
    let Some(e) = env().await else { return };
    let tag_id = insert_tag_with_clearance(&e.db.pool, &[], &[]).await;
    let path = format!("/perm/clearances/{tag_id}/people");
    let (status, _) = get_path(&e, &e.nobody, &path).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = get_path(&e, &e.reader, &path).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let empty = json!({"count": 0, "people": [], "truncated": false,
                       "enginesEnforce": false, "onEnginesCount": 0});
    assert_eq!(body, empty);
    // A tag with no clearance row at all answers the same.
    let t = tag();
    let bare: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
            .bind(format!("clr-bare-{t}"))
            .fetch_one(&e.db.pool)
            .await
            .expect("tag");
    let (status, body) = get_path(&e, &e.reader, &format!("/perm/clearances/{bare}/people")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, empty);
    let (status, _) = get_path(
        &e,
        &e.reader,
        &format!("/perm/clearances/{}/people", Uuid::new_v4()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A scoped role grant carries its scope (egret clears within it only;
/// engines everywhere). A role holder with no username yet is listed but not
/// named on engines until a compile mints one, and a required group id no
/// group has any more admits nobody.
#[tokio::test]
async fn cleared_people_reports_scope_and_an_unnamed_role_holder() {
    let Some(e) = env().await else { return };
    let p = &e.db.pool;
    let t = tag();
    let unnamed = insert_user(p, &format!("clr_unn_{t}@egret.local"), "", None).await;
    let role_name = format!("clr_unn_role_{t}");
    let role = insert_role(p, "workspace", &role_name).await;
    let ws = Uuid::new_v4();
    insert_grant(
        p,
        "user",
        unnamed,
        role,
        "workspace",
        Some(ws),
        "allow",
        false,
    )
    .await;
    let tag_id =
        insert_tag_with_clearance(p, std::slice::from_ref(&role_name), &[Uuid::new_v4()]).await;
    let body = principal(&e, &format!("/perm/clearances/{tag_id}/people")).await;
    assert_eq!(body["count"], 1, "{body}");
    assert_eq!(body["onEnginesCount"], 0);
    let row = &body["people"][0];
    assert_eq!(
        row["label"],
        json!(format!("clr_unn_{t}@egret.local")),
        "no name: the label is the email"
    );
    assert!(row["username"].is_null());
    assert_eq!(row["onEngines"], false, "{row}");
    assert_eq!(
        row["via"],
        json!([role_via(&role_name, "workspace", Some(ws), None)])
    );
}

/// A disabled account and a service account are listed, and marked.
#[tokio::test]
async fn cleared_people_marks_disabled_and_service_accounts() {
    let Some(e) = env().await else { return };
    let p = &e.db.pool;
    let t = tag();
    let disabled = insert_user(
        p,
        &format!("clr_dis_{t}@egret.local"),
        "A disabled",
        Some(&format!("clr_dis_{t}")),
    )
    .await;
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(disabled)
        .execute(p)
        .await
        .expect("disable");
    let svc: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('service', $1, 'B service', $1) RETURNING id",
    )
    .bind(format!("clr_svc_{t}"))
    .fetch_one(p)
    .await
    .expect("service account");
    let role_name = format!("clr_mark_{t}");
    let role = insert_role(p, "platform", &role_name).await;
    insert_grant(p, "user", disabled, role, "platform", None, "allow", false).await;
    insert_grant(p, "user", svc, role, "platform", None, "allow", false).await;
    let tag_id = insert_tag_with_clearance(p, std::slice::from_ref(&role_name), &[]).await;
    let body = principal(&e, &format!("/perm/clearances/{tag_id}/people")).await;
    let people = body["people"].as_array().unwrap();
    assert_eq!(people.len(), 2, "{body}");
    assert_eq!(people[0]["userId"], json!(disabled));
    assert_eq!(people[0]["status"], "disabled");
    assert_eq!(people[0]["serviceAccount"], false);
    assert_eq!(people[1]["userId"], json!(svc));
    assert_eq!(people[1]["status"], "active");
    assert_eq!(people[1]["serviceAccount"], true);
}

/// `n` role holders with usernames, labelled `Cap 000`, `Cap 001`, …
async fn insert_role_holders(p: &sqlx::PgPool, t: &str, n: i32) -> String {
    let role_name = format!("clr_cap_{t}");
    let role = insert_role(p, "platform", &role_name).await;
    sqlx::query(
        "WITH u AS ( \
            INSERT INTO users (provider, email, name, username) \
            SELECT 'local', format('clr_cap_%s_%s@egret.local', $1::text, i), \
                   format('Cap %s', lpad(i::text, 3, '0')), \
                   format('clr_cap_%s_%s', $1::text, i) \
              FROM generate_series(0, $3 - 1) AS i \
            RETURNING id) \
         INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, effect) \
         SELECT 'user', id, $2, 'platform', 'allow' FROM u",
    )
    .bind(t)
    .bind(role)
    .bind(n)
    .execute(p)
    .await
    .expect("role holders");
    role_name
}

/// The list is capped at 200 people; `count` and `onEnginesCount` still
/// count everyone.
#[tokio::test]
async fn cleared_people_is_capped_at_200_and_says_so() {
    let Some(e) = env().await else { return };
    let p = &e.db.pool;
    let role_name = insert_role_holders(p, &tag(), 201).await;
    let tag_id = insert_tag_with_clearance(p, &[role_name], &[]).await;
    let body = principal(&e, &format!("/perm/clearances/{tag_id}/people")).await;
    assert_eq!(body["count"], 201, "{}", body["count"]);
    assert_eq!(body["onEnginesCount"], 201, "independent of the cap");
    assert_eq!(body["truncated"], true);
    let people = body["people"].as_array().unwrap();
    assert_eq!(people.len(), 200);
    assert_eq!(people[0]["label"], "Cap 000");
    assert_eq!(people[199]["label"], "Cap 199");
}

/// Exactly 200 people: all listed, not truncated.
#[tokio::test]
async fn cleared_people_lists_exactly_200_untruncated() {
    let Some(e) = env().await else { return };
    let p = &e.db.pool;
    let role_name = insert_role_holders(p, &tag(), 200).await;
    let tag_id = insert_tag_with_clearance(p, &[role_name], &[]).await;
    let body = principal(&e, &format!("/perm/clearances/{tag_id}/people")).await;
    assert_eq!(body["count"], 200);
    assert_eq!(body["truncated"], false);
    assert_eq!(body["people"].as_array().unwrap().len(), 200);
}
