//! SP4a Task 8 (D4): a profile the permission-model compiler owns
//! (`policy.authz_services.origin = 'generated'`, migration 0155) is
//! read-only through the policy editor. Before this guard, a human edit into
//! such a profile would be silently overwritten (or its policy silently
//! deleted) by the next recompile — see `egret-perm-compile::trigger`'s
//! `apply_diff_in_tx`, which reaps any `policy.authz_policies` row under a
//! service that this compiler no longer produces.
//!
//! This file proves the REFUSAL side: given a profile already marked
//! `generated` (seeded directly via SQL here — the fixture is simulating "a
//! profile the compiler has claimed", not exercising the compiler itself),
//! every hand-authored write path refuses it with 409, naming the compiler in
//! the body. The companion proof that a REAL compile is what sets
//! `origin = 'generated'` in the first place lives in
//! `crates/egret-perm-compile/tests/recompile.rs` (this crate has no
//! dependency on the compiler crate, so it cannot exercise that side).
//!
//! DB-gated; self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-policy --test generated_profile_guard

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::groups::GroupRepo;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_policy::{
    policy_admin_router, EngineBinding, EngineBindingReader, EngineReadError, PolicyAdminState,
    PolicyStore,
};
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

/// No engine ever binds to anything in these tests — every profile here is
/// created and torn down within one test, so `delete_profile`'s "still
/// enforced by an engine" check is always a pass-through, and the origin
/// guard is the only thing under test.
struct NoEngines;
#[async_trait::async_trait]
impl EngineBindingReader for NoEngines {
    async fn list_engine_bindings(&self) -> Result<Vec<EngineBinding>, EngineReadError> {
        Ok(Vec::new())
    }
    async fn engine_catalog_names(&self, _engine_id: Uuid) -> Result<Vec<String>, EngineReadError> {
        Ok(Vec::new())
    }
    async fn warehouse_schemas(&self, _wh: Uuid) -> Result<Vec<String>, EngineReadError> {
        Ok(Vec::new())
    }
    async fn warehouse_tables(
        &self,
        _wh: Uuid,
        _schema: &str,
    ) -> Result<Vec<String>, EngineReadError> {
        Ok(Vec::new())
    }
    async fn warehouse_columns(
        &self,
        _wh: Uuid,
        _schema: &str,
        _table: &str,
    ) -> Result<Vec<String>, EngineReadError> {
        Ok(Vec::new())
    }
}

struct AllowAllPdp;
#[async_trait::async_trait]
impl egret_core::authz::Pdp for AllowAllPdp {
    async fn authorize(
        &self,
        _s: &egret_core::authz::Subject,
        _a: &str,
        _r: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: true,
            reasons: vec![],
        })
    }
}

struct Fixture {
    db: Db,
    users: UserRepo,
    groups: GroupRepo,
    sessions: SessionService,
    store: PolicyStore,
    pdp: Arc<dyn egret_core::authz::Pdp>,
}

impl Fixture {
    fn app(&self) -> axum::Router {
        policy_admin_router().with_state(PolicyAdminState {
            store: self.store.clone(),
            engines: Arc::new(NoEngines),
            users: self.users.clone(),
            groups: self.groups.clone(),
            sessions: self.sessions.clone(),
            engine_egret_url: "http://egret:8080".into(),
            pdp: self.pdp.clone(),
        })
    }
}

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    Some(Fixture {
        users: UserRepo::new(db.clone()),
        groups: GroupRepo::new(db.clone()),
        sessions,
        store: PolicyStore::new(db.clone()),
        pdp: Arc::new(AllowAllPdp),
        db,
    })
}

fn uniq(p: &str) -> String {
    format!("{p}_{}", Uuid::new_v4().simple())
}

fn req(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> HttpRequest<Body> {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    b.body(body).unwrap()
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

/// Platform-admin session token — every route here gates on a `policy:*`
/// permission the PDP is stubbed to always grant; the interesting variable
/// under test is the profile's `origin`, not authz.
async fn admin_session(f: &Fixture, tag: &str) -> String {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// A trino profile whose `origin` is `'generated'` — simulating a profile the
/// SP4a compiler has claimed. Set directly via SQL: going through
/// `PolicyStore::create_service` only ever produces `'manual'` (migration
/// 0155's default), and the ONLY code path that ever writes `'generated'` is
/// `egret-perm-compile::trigger::apply_diff_in_tx`'s real compile — proven
/// separately in `egret-perm-compile/tests/recompile.rs`. This fixture is
/// entitled to bypass that and set the column directly because what it's
/// testing is "given a generated profile, is writing into it refused", not
/// "does a compile mark a profile generated".
async fn seed_generated_profile(f: &Fixture) -> (String, i64) {
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino def");
    let name = uniq("trino-generated");
    let svc = f
        .store
        .create_service(&name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");
    sqlx::query("UPDATE policy.authz_services SET origin = 'generated' WHERE id = $1")
        .bind(svc.id)
        .execute(&f.db.pool)
        .await
        .expect("mark generated");
    (name, svc.id)
}

async fn seed_manual_profile(f: &Fixture) -> (String, i64) {
    egret_policy::seed_trino(&f.store)
        .await
        .expect("seed trino def");
    let name = uniq("trino-manual");
    let svc = f
        .store
        .create_service(&name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");
    (name, svc.id)
}

/// Insert a policy row directly (bypassing the now-guarded
/// `PolicyStore::create_policy`) so update/enable/delete tests have
/// something to act on inside an already-generated profile.
async fn insert_policy_directly(f: &Fixture, service_id: i64, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(service_id)
    .bind(name)
    .bind(sample_policy_body(name))
    .fetch_one(&f.db.pool)
    .await
    .expect("insert policy directly")
}

fn sample_policy_body(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name, "policyType": 0,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "policyItems": [{"users": ["alice"], "accesses": [{"type": "select"}]}]
    })
}

fn sample_policy(svc_name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": "p-into-generated", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
    })
}

async fn post_policy(
    f: &Fixture,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    into_json(
        f.app()
            .oneshot(req("POST", "/policies", token, Some(body)))
            .await
            .unwrap(),
    )
    .await
}

/// Review fix (M-3): a single `contains("compiled")` would also pass for a
/// reworded-but-opaque message ("compiled: no" or "recompiled recently"),
/// which doesn't actually satisfy the brief's requirement that the refusal
/// NAME the compiler — an operator reading it must be able to tell (a) WHICH
/// system claimed this profile and (b) what to do about it instead of
/// retrying. Check for all three, independently, against what the shipped
/// message (`store.rs::refuse_if_generated`) actually carries: it names the
/// compiler by name ("SP4a permission model"), says the profile IS compiled
/// (so a retry looks pointless, not transient), and tells the operator the
/// alternative ("grants and rules").
fn assert_names_the_compiler(body: &serde_json::Value) {
    let text = body.to_string();
    assert!(
        text.contains("SP4a permission model"),
        "the refusal must name WHICH system owns this profile, not just say something is \
         \"compiled\" in the abstract — body={body}"
    );
    assert!(
        text.contains("compiled"),
        "the refusal must say the profile IS compiled — a 409 that does not say why reads as \
         a bug, and the operator's next move is to retry; body={body}"
    );
    assert!(
        text.contains("grants") && text.contains("rules"),
        "the refusal must tell the operator what to do INSTEAD (edit the grants/rules that \
         compile into this profile), not merely that the write failed; body={body}"
    );
}

#[tokio::test]
async fn the_editor_refuses_to_write_a_policy_into_a_generated_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_create").await;
    let (svc_name, svc_id) = seed_generated_profile(&f).await;

    let (status, body) = post_policy(&f, &token, sample_policy(&svc_name)).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={body}");
    assert_names_the_compiler(&body);

    // And no policy was actually written.
    let rows = f.store.list_policies(svc_id).await.expect("list");
    assert!(rows.is_empty(), "the refused write must not land");

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn the_editor_refuses_to_update_a_policy_in_a_generated_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_update").await;
    let (svc_name, svc_id) = seed_generated_profile(&f).await;
    let policy_id = insert_policy_directly(&f, svc_id, "perm-grant-existing").await;

    let update = serde_json::json!({
        "name": "perm-grant-existing", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "items": [{"users": ["mallory"], "accesses": ["select"], "effect": "allow"}]
    });
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PUT",
                &format!("/policies/{policy_id}"),
                &token,
                Some(update),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body={body}");
    assert_names_the_compiler(&body);

    // The body must be byte-identical to what was seeded — not partially applied.
    let row = f.store.get_policy(policy_id).await.expect("get policy");
    assert_eq!(row.policy_jsonb, sample_policy_body("perm-grant-existing"));

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn the_editor_refuses_to_toggle_enabled_on_a_policy_in_a_generated_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_enable").await;
    let (_svc_name, svc_id) = seed_generated_profile(&f).await;
    let policy_id = insert_policy_directly(&f, svc_id, "perm-grant-existing").await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PUT",
                &format!("/policies/{policy_id}/enabled"),
                &token,
                Some(serde_json::json!({ "enabled": false })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body={body}");
    assert_names_the_compiler(&body);

    let row = f.store.get_policy(policy_id).await.expect("get policy");
    assert!(row.enabled, "the refused toggle must not land");

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn the_editor_refuses_to_delete_a_policy_in_a_generated_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_delete").await;
    let (_svc_name, svc_id) = seed_generated_profile(&f).await;
    let policy_id = insert_policy_directly(&f, svc_id, "perm-grant-existing").await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "DELETE",
                &format!("/policies/{policy_id}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body={body}");
    assert_names_the_compiler(&body);

    // Still there.
    f.store
        .get_policy(policy_id)
        .await
        .expect("must survive the refused delete");

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

/// Review fix (I-2): the fixture MUST already have a policy before the
/// seed-defaults call. `seed_default_trino_policies` has two guards in
/// series — Task 8's origin check, THEN (only if that passes) the pre-existing
/// "already has policies → no-op" check reached via the loop of
/// `create_policy` calls, which carries its OWN Task 8 guard. On an EMPTY
/// generated profile, deleting `default_policies.rs`'s own guard still
/// produces a 409 — just from the first `create_policy` call inside the seed
/// loop, not from seed-defaults' own check — so a test with an empty fixture
/// cannot tell the two guards apart and passes even when the one it claims to
/// cover is gone. With an EXISTING policy already in place, the no-op branch
/// (`list_policies` non-empty → `Ok(0)`) is what the seed path would hit
/// BEFORE ever reaching a `create_policy` call, and only `seed_default_trino_policies`'s
/// own guard — checked before that no-op — can catch it: exactly the "silent
/// `Ok(0)`/200 instead of a 409" bug the guard's comment claims to prevent.
#[tokio::test]
async fn seed_defaults_refuses_a_generated_profile_that_already_has_policies() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_seed").await;
    let (svc_name, svc_id) = seed_generated_profile(&f).await;
    let policy_id = insert_policy_directly(&f, svc_id, "perm-grant-existing").await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                &format!("/policies/services/{svc_name}/seed-defaults"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body={body}");
    assert_names_the_compiler(&body);

    // Exactly the pre-existing policy, untouched — not the no-op `Ok(0)`
    // (which would ALSO leave exactly one row, so this alone wouldn't
    // distinguish the bug; the 409 assertion above is what does that) and not
    // 13 freshly-seeded defaults on top of it either.
    let rows = f.store.list_policies(svc_id).await.expect("list");
    assert_eq!(
        rows.len(),
        1,
        "seed-defaults must not have created anything on a generated profile; rows={rows:?}"
    );
    assert_eq!(rows[0].id, policy_id);

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn delete_profile_refuses_a_generated_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_delete_profile").await;
    let (svc_name, svc_id) = seed_generated_profile(&f).await;

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "DELETE",
                &format!("/policies/services/{svc_name}"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body={body}");
    assert_names_the_compiler(&body);

    // Still resolvable.
    f.store
        .get_service(&svc_name)
        .await
        .expect("the profile must survive the refused delete");

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

/// Regression: the guard must not overreach onto an ordinary `manual`
/// profile — every write path this file exercises against a generated
/// profile must still succeed against a ordinary one.
#[tokio::test]
async fn a_manual_profile_is_still_fully_editable() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_manual").await;
    let (svc_name, svc_id) = seed_manual_profile(&f).await;

    let (status, body) = post_policy(&f, &token, sample_policy(&svc_name)).await;
    assert_eq!(status, StatusCode::OK, "create; body={body}");
    let policy_id = body["id"].as_i64().expect("id");

    let update = serde_json::json!({
        "name": "p-into-generated", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "items": [{"users": ["mallory"], "accesses": ["select"], "effect": "allow"}]
    });
    let resp = f
        .app()
        .oneshot(req(
            "PUT",
            &format!("/policies/{policy_id}"),
            &token,
            Some(update),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "update must still work");

    let resp = f
        .app()
        .oneshot(req(
            "PUT",
            &format!("/policies/{policy_id}/enabled"),
            &token,
            Some(serde_json::json!({ "enabled": false })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "toggle must still work"
    );

    let resp = f
        .app()
        .oneshot(req(
            "DELETE",
            &format!("/policies/{policy_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "delete must still work"
    );

    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "POST",
                &format!("/policies/services/{svc_name}/seed-defaults"),
                &token,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "seed-defaults must still work; body={body}"
    );

    let resp = f
        .app()
        .oneshot(req(
            "DELETE",
            &format!("/policies/services/{svc_name}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "delete-profile must still work"
    );
    let _ = svc_id; // service already deleted above via the HTTP route
}
