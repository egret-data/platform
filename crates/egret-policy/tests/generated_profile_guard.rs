//! SP4a Task 8 (D4), fix round 2 (I-1): a profile the permission-model
//! compiler owns (`policy.authz_services.origin = 'generated'`, migration
//! 0155) is read-only through the policy editor ONLY for the specific
//! policies the compiler produced. Before Task 8, any edit into such a
//! profile could be silently overwritten (or its policy silently deleted) by
//! the next recompile — see `egret-perm-compile::trigger`'s
//! `apply_diff_in_tx`, which reaps any `policy.authz_policies` row under a
//! service that this compiler no longer produces. Round 1 refused EVERY
//! write into a `generated` profile, which protected policies the compiler
//! never looks at too (anything not named with its `perm-grant-`/
//! `perm-rule-` prefix — see `egret_policy::is_compiler_owned_policy_name`)
//! and so built a one-way door: once a profile compiled anything, an
//! operator could never touch ANY policy in it again through the editor.
//! Round 2 (this file, per the user's I-1 ruling) moves the EDIT/DELETE
//! guard from profile scope to policy scope: `create_policy` and
//! `seed_default_trino_policies` still refuse outright on a `generated`
//! profile (adding brand-new content to a compiled profile is still the
//! confusion Task 8 exists to prevent), but `update_policy`/`delete_policy`/
//! `set_policy_enabled` now refuse only the SPECIFIC policies the compiler
//! owns, and `delete_service` refuses only while a compiler-owned policy
//! still exists under it.
//!
//! This file proves BOTH directions: the refusal side (given a profile
//! already marked `generated`, seeded directly via SQL — the fixture is
//! simulating "a profile the compiler has claimed", not exercising the
//! compiler itself — a compiler-owned policy's edit/delete/enable-toggle,
//! `create_policy`, and `delete_service` while an owned policy remains all
//! refuse with 409, naming the compiler AND (where the refusal is
//! policy-scoped) the specific policy in the body) and the escape-hatch side
//! (a NON-owned policy in that same generated profile can still be updated,
//! toggled, and deleted, and the profile itself can be deleted once no
//! owned policy remains — proving I-1 is actually closed, not merely that
//! the guard still refuses something). The companion proof that a REAL
//! compile is what sets `origin = 'generated'` in the first place lives in
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
            recompiler: std::sync::Arc::new(NoRecompile),
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

/// Like [`sample_policy`], with a caller-chosen name — used by the fix
/// round 3 reserved-namespace tests to attempt a `perm-grant-`/`perm-rule-`
/// named create/rename.
fn sample_policy_named(svc_name: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name, "service": svc_name, "isEnabled": true,
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
/// system claimed this profile/policy and (b) what to do about it instead of
/// retrying. Check for all three, independently, against what every shipped
/// message in `store.rs` actually carries: it names the compiler by name
/// ("SP4a permission model"), says the target IS compiled (so a retry looks
/// pointless, not transient), and tells the operator the alternative (a
/// grant or rule to edit/delete instead). Singular "grant"/"rule" substrings
/// so this also matches a message that happens to pluralize them ("grants
/// and rules") — the fact under test is that the concept is named, not its
/// exact grammatical number.
fn assert_names_the_compiler(body: &serde_json::Value) {
    let text = body.to_string();
    assert!(
        text.contains("SP4a permission model"),
        "the refusal must name WHICH system owns this profile, not just say something is \
         \"compiled\" in the abstract — body={body}"
    );
    assert!(
        text.contains("compiled"),
        "the refusal must say the target IS compiled — a 409 that does not say why reads as \
         a bug, and the operator's next move is to retry; body={body}"
    );
    assert!(
        text.contains("grant") && text.contains("rule"),
        "the refusal must tell the operator what to do INSTEAD (edit/delete the grant or rule \
         that produces this), not merely that the write failed; body={body}"
    );
}

/// Fix round 2 (I-1, review requirement 2): the PER-POLICY refusal
/// (`update_policy`/`delete_policy`/`set_policy_enabled` on a
/// compiler-owned policy) must additionally name WHICH policy is
/// compiler-owned — an operator with several policies in a generated
/// profile needs to know which one to leave alone, not just that some
/// unspecified thing in this profile is off-limits (that genericity is
/// exactly what made the round-1 profile-level refusal a one-way door: it
/// gave no way to tell "this policy is protected" from "this profile is
/// protected"). Builds on [`assert_names_the_compiler`] with the added,
/// independent fact.
fn assert_names_the_compiler_and_the_policy(body: &serde_json::Value, policy_name: &str) {
    assert_names_the_compiler(body);
    let text = body.to_string();
    assert!(
        text.contains(policy_name),
        "the refusal must name WHICH policy is compiler-owned, not just that some policy in \
         this profile is — body={body}, policy_name={policy_name:?}"
    );
}

/// Fix round 3: the RESERVED-NAMESPACE refusal (`refuse_reserved_name`) is a
/// distinct guard from `refuse_if_generated`/`refuse_if_compiler_owned` — it
/// fires regardless of the profile's `origin` and says something different
/// ("this NAME is off-limits, rename it") from either of those ("this
/// PROFILE/POLICY is compiled, edit the grant/rule instead"). Assert on that
/// distinct vocabulary ("reserved") rather than reusing
/// `assert_names_the_compiler`, so a test asserting THIS guard fired can't
/// be satisfied by one of the other two firing instead.
fn assert_names_reserved_prefix(body: &serde_json::Value, attempted_name: &str) {
    let text = body.to_string();
    assert!(
        text.contains("SP4a permission model"),
        "the refusal must name WHICH system reserves this prefix — body={body}"
    );
    assert!(
        text.contains("reserved"),
        "the refusal must say the NAME is reserved (not that a profile/policy is \"compiled\") \
         — a human hitting this needs to know the fix is renaming, not waiting for a recompile \
         or editing a grant; body={body}"
    );
    assert!(
        text.contains(attempted_name),
        "the refusal must name WHICH name was rejected; body={body}, attempted_name={attempted_name:?}"
    );
    assert!(
        text.contains("rename"),
        "the refusal must tell the operator the fix is to RENAME, so their next move is that, \
         not a retry; body={body}"
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
    assert_names_the_compiler_and_the_policy(&body, "perm-grant-existing");

    // The body must be byte-identical to what was seeded — not partially applied.
    let row = f.store.get_policy(policy_id).await.expect("get policy");
    assert_eq!(row.policy_jsonb, sample_policy_body("perm-grant-existing"));

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

/// I-1 closed (review requirement 3): the escape hatch. A policy in the SAME
/// generated profile whose name the compiler does not recognize (no
/// `perm-grant-`/`perm-rule-` prefix) can still be updated, toggled, and
/// deleted — proving the guard moved from profile scope to policy scope, not
/// merely that it still refuses SOMETHING. Without this test the round-2 fix
/// is unproven: every "refuses" test above would pass identically whether
/// the guard checks the profile or the policy.
#[tokio::test]
async fn a_non_owned_policy_can_still_be_edited_and_deleted_in_a_generated_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_escape_hatch").await;
    let (svc_name, svc_id) = seed_generated_profile(&f).await;
    let policy_id = insert_policy_directly(&f, svc_id, "hand-authored-policy").await;

    // Update succeeds.
    let update = serde_json::json!({
        "name": "hand-authored-policy", "service": svc_name, "isEnabled": true,
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
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a non-owned policy must still be updatable in a generated profile"
    );
    let row = f.store.get_policy(policy_id).await.expect("get policy");
    assert!(
        row.policy_jsonb["policyItems"][0]["users"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "mallory"),
        "the update must actually have landed, not just returned 200; body={:?}",
        row.policy_jsonb
    );

    // Enabled-toggle succeeds.
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
        "a non-owned policy must still be toggleable in a generated profile"
    );
    let row = f.store.get_policy(policy_id).await.expect("get policy");
    assert!(!row.enabled, "the toggle must actually have landed");

    // Delete succeeds.
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
        "a non-owned policy must still be deletable in a generated profile"
    );
    assert!(
        f.store.get_policy(policy_id).await.is_err(),
        "the delete must actually have landed"
    );

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
    assert_names_the_compiler_and_the_policy(&body, "perm-grant-existing");

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
    assert_names_the_compiler_and_the_policy(&body, "perm-grant-existing");

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

/// I-1 closed, `delete_service` side: a generated profile holding ONLY a
/// foreign (non-compiler-owned) policy is deletable — the profile's
/// `origin` alone no longer gates the delete, only whether a compiler-owned
/// policy is still present under it.
#[tokio::test]
async fn delete_profile_succeeds_on_a_generated_profile_holding_only_foreign_policies() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_delete_profile_foreign").await;
    let (svc_name, svc_id) = seed_generated_profile(&f).await;
    insert_policy_directly(&f, svc_id, "hand-authored-policy").await;

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
        "a generated profile with no compiler-owned policy must be deletable"
    );
    assert!(
        f.store.get_service(&svc_name).await.is_err(),
        "the delete must actually have landed"
    );
    let _ = svc_id; // deleted via the HTTP route; nothing left to clean up
}

/// I-1 closed, `delete_service` side (the refusal half): the SAME generated
/// profile is refused with 409 while a compiler-owned policy still exists
/// under it — this is the case round 1's profile-level `origin` check would
/// have also caught, so pairing it with the success case above is what
/// proves the new rule is "owned policies remain", not "origin says
/// generated".
#[tokio::test]
async fn delete_profile_refuses_while_a_compiler_owned_policy_remains() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_delete_profile_owned").await;
    let (svc_name, svc_id) = seed_generated_profile(&f).await;
    insert_policy_directly(&f, svc_id, "perm-grant-existing").await;

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

    // Still resolvable, and the owned policy is untouched.
    f.store
        .get_service(&svc_name)
        .await
        .expect("the profile must survive the refused delete");
    let rows = f.store.list_policies(svc_id).await.expect("list");
    assert_eq!(rows.len(), 1, "the owned policy must survive too");

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

// ── Fix round 3: the compiler's prefix namespace is reserved everywhere ────
//
// The round-2 report found that a human could hand-name a policy
// `perm-grant-foo` while its profile was still `manual` (allowed then,
// since `create_policy`'s origin check only fires once the profile is
// ALREADY `generated`), and the FIRST real recompile that ever reached that
// service would silently delete it in the SAME transaction that first
// stamped the profile `generated` — no observable window for an operator to
// notice and intervene. The fix: make the collision unrepresentable at the
// only human entry point (`create_policy`/`update_policy`), rather than
// teaching the sweep to defend against it (Task 7 fix round 1's own
// Critical was precisely that a name-matching row is reaped with or without
// `perm.compiled` provenance — re-litigating that here would undo it).

#[tokio::test]
async fn a_manual_profile_refuses_creating_a_reserved_name_policy() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_reserved_create_manual").await;
    let (svc_name, svc_id) = seed_manual_profile(&f).await;

    let (status, body) = post_policy(
        &f,
        &token,
        sample_policy_named(&svc_name, "perm-grant-imposter"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a MANUAL profile must still refuse a reserved-prefix name; body={body}"
    );
    assert_names_reserved_prefix(&body, "perm-grant-imposter");

    let rows = f.store.list_policies(svc_id).await.expect("list");
    assert!(rows.is_empty(), "the refused create must not land");

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

#[tokio::test]
async fn a_manual_profile_refuses_renaming_a_policy_to_a_reserved_name() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_reserved_rename_manual").await;
    let (svc_name, svc_id) = seed_manual_profile(&f).await;

    // An ordinary, non-reserved policy — creating it must still work.
    let (status, body) = post_policy(&f, &token, sample_policy(&svc_name)).await;
    assert_eq!(status, StatusCode::OK, "create; body={body}");
    let policy_id = body["id"].as_i64().expect("id");

    // Renaming it INTO the reserved namespace must be refused.
    let rename = sample_policy_named(&svc_name, "perm-rule-imposter");
    let (status, body) = into_json(
        f.app()
            .oneshot(req(
                "PUT",
                &format!("/policies/{policy_id}"),
                &token,
                Some(rename),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body={body}");
    assert_names_reserved_prefix(&body, "perm-rule-imposter");

    // The policy must still have its ORIGINAL name — the refused rename
    // must not have landed even partially.
    let row = f.store.get_policy(policy_id).await.expect("get policy");
    assert_eq!(row.name, "p-into-generated");

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

/// The generated-profile case, asserting the RESERVED-NAME reason is the
/// operative one (not merely "this profile is compiled" — `refuse_if_generated`
/// would ALSO refuse this exact call, but with different wording; this test
/// pins down that the reserved-namespace guard is what actually fires,
/// which matters because it is the guard that also protects `manual`
/// profiles, where `refuse_if_generated` does not apply at all).
#[tokio::test]
async fn a_generated_profile_refuses_a_reserved_name_for_the_reserved_reason() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_reserved_create_generated").await;
    let (svc_name, svc_id) = seed_generated_profile(&f).await;

    let (status, body) = post_policy(
        &f,
        &token,
        sample_policy_named(&svc_name, "perm-rule-imposter"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body={body}");
    assert_names_reserved_prefix(&body, "perm-rule-imposter");

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

/// **SP4d debt Task 16, closing Task 6's concern 5.** When a create would be
/// refused by BOTH the profile's `origin` and one of the editor's own authoring
/// guards, the operator must read the OPERATIVE refusal — "this profile is
/// compiled, edit its grants and rules" (409) — and not the editor guard's 400.
///
/// Task 6 shipped its ambiguous-arbitration guard in the route handler, which
/// runs before `store.create_policy` and therefore before
/// `refuse_if_generated`, so a steward who hit both was told their policy was
/// ambiguous when the real answer was that nothing they wrote could be kept: the
/// next recompile would reap it. Task 6 deliberately did NOT add its own
/// `origin` read to fix that (a second spelling of "is this profile
/// compiler-owned"), and was right not to; the fix is a pre-flight that runs the
/// store's OWN refusals, in the store's own order.
///
/// The premise is a generated profile that already serves a row filter, plus a
/// POST of a DISAGREEING second row filter on the same table — Task 6's refused
/// shape. The two accept/behaviour controls are elsewhere and both matter:
/// `a_generated_profile_refuses_a_reserved_name_for_the_reserved_reason` pins
/// that the pre-flight did not invert the reserved-name precedence, and
/// `a_second_row_filter_on_one_table_is_refused_only_when_it_disagrees`
/// (admin_routes) pins that the 400 still fires in a MANUAL profile — without
/// that one, "always 409" would pass here.
#[tokio::test]
async fn a_generated_profile_answers_409_before_the_editors_own_400() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = admin_session(&f, "guard_409_before_400").await;
    let (svc_name, svc_id) = seed_generated_profile(&f).await;

    // A row filter already served by this profile, inserted directly because
    // `create_policy` is (correctly) refused for this profile.
    let existing = serde_json::json!({
        "name": "rf-existing", "service": svc_name, "policyType": 2,
        "serviceType": "trino",
        "resources": {
            "catalog": {"values": ["iceberg"], "isExcludes": false, "isRecursive": false},
            "schema": {"values": ["sales"], "isExcludes": false, "isRecursive": false},
            "table": {"values": ["orders"], "isExcludes": false, "isRecursive": false}
        },
        "rowFilterPolicyItems": [
            {"groups": ["analysts"], "accesses": [{"type": "select", "isAllowed": true}],
             "rowFilterInfo": {"filterExpr": "region = 'eu'"}}
        ]
    });
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) \
         VALUES ($1, $2, $3)",
    )
    .bind(svc_id)
    .bind("rf-existing")
    .bind(&existing)
    .execute(&f.db.pool)
    .await
    .expect("seed existing row filter");

    // A second row filter on the same table with a DIFFERENT expression: Task
    // 6's refused pair, which the editor guard would answer 400 for.
    let (status, body) = post_policy(
        &f,
        &token,
        serde_json::json!({
            "kind": "rowfilter", "name": "rf-second", "service": svc_name,
            "isEnabled": true,
            "resources": {
                "catalog": {"values": ["iceberg"]},
                "schema": {"values": ["sales"]},
                "table": {"values": ["orders"]}
            },
            "items": [{"groups": ["analysts"], "filterExpr": "region = 'us'"}]
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a generated profile must answer the compiler-owned 409, not the editor's \
         ambiguous-arbitration 400 — the operative fact is that nothing written here \
         survives the next recompile; body={body}"
    );
    assert_names_the_compiler(&body);

    sqlx::query("DELETE FROM policy.authz_services WHERE id = $1")
        .bind(svc_id)
        .execute(&f.db.pool)
        .await
        .expect("cleanup");
}

/// The editor's recompile seam is `egret_perm_compile::PermRecompiler` in
/// production, which this crate cannot depend on; these tests are about the
/// editor's own guards, and the refold it triggers is tested in
/// `egret-perm-compile` (`tests/clearance_starrocks.rs`).
struct NoRecompile;

#[async_trait::async_trait]
impl egret_policy::ProfileRecompiler for NoRecompile {
    async fn recompile_in_tx(
        &self,
        _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
        _service_ids: &[i64],
    ) -> Result<(), egret_policy::PolicyHttpError> {
        Ok(())
    }
}
