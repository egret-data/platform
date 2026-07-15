# Delegated Trino Queries — Impersonate Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make per-user delegated queries run securely on Ranger-governed Trino engines by having egret authenticate as a trusted `egret_delegator` service user (basic-auth, secret-only) and impersonate the calling end-user, so the query runs as that user under their own Ranger policies.

**Architecture:** Add an engine `delegationMode` (default `impersonate`). At provision time an impersonate-mode engine gets a generated `egret_delegator` password (stored in an egret secret, referenced by a new `delegator_secret_id` column), the user is injected into `password.db`, and two Ranger policies are seeded (impersonate + query-timeout). The engine→workspace assignment materializes the delegated `query.databases` row with that secret + `{user, impersonate:true}` options. The query route, seeing `options.impersonate`, runs as the caller without minting a JWT. The `TrinoConnector` basic-auth+impersonate path is used unchanged.

**Tech Stack:** Rust workspace (axum, sqlx, utoipa), Postgres, Trino + apache-ranger plugin, `bcrypt`, `egret-secrets`, `egret-policy` store.

**Spec:** `docs/en/specs/2026-07-15-delegated-trino-impersonate-mode-design.md`

---

## File Structure

- `crates/egret-metastore/migrations/0085_engine_delegator_secret.sql` (create) — `delegator_secret_id` column on `engine_mgmt.engines`.
- `crates/egret-engine-mgmt/src/store.rs` (modify) — `Engine.delegator_secret_id`; `delegation_mode()`, `supports_delegation()`, `is_impersonate_delegation()`; `set_delegator_secret()`.
- `crates/egret-engine-mgmt/src/engines.rs` (modify) — `DELEGATOR_USERNAME` const, `ensure_delegator(...)`, `inject_delegator_user(...)`; seed policies; wire into `create_engine`/`update_engine`.
- `crates/egret-engine-mgmt/src/delegator_policy.rs` (create) — build the two seed policy JSON bodies (pure fn + unit test).
- `crates/egret-query/src/registry/repo.rs` (modify) — `create_provisioned` gains `secret_id: Option<Uuid>`.
- `crates/egret-engine-mgmt/src/assignment.rs` (modify) — gate on `supports_delegation()`; materialize impersonate options + delegator secret.
- `crates/egret-query/src/execution/routes.rs` (modify) — delegated branch: `options.impersonate` ⇒ run as caller, no mint.

**Test/verify commands** (run from repo root `.../egret`):
- Rust unit: `cargo test -p <crate> --lib <filter>`
- Format+lint before each commit: `cargo fmt -p <crate>` and `cargo clippy -p <crate> --lib --tests`

---

## Task 1: Migration — `delegator_secret_id` column

**Files:**
- Create: `crates/egret-metastore/migrations/0085_engine_delegator_secret.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (touch to force the embedded migrator to re-embed — see [[egret-sqlx-migration-staleness]])

- [ ] **Step 1: Write the migration**

```sql
-- 0085_engine_delegator_secret.sql
-- Per-engine secret holding the generated `egret_delegator` service-user password
-- (impersonate-mode delegation). NULL until the engine is (re)provisioned in
-- impersonate mode. The password is stored in `secrets`; this column references it.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN delegator_secret_id uuid;
```

- [ ] **Step 2: Force the embedded migrator to pick it up**

Append a trailing newline / touch `crates/egret-metastore/src/lib.rs` so `sqlx::migrate!` re-embeds (the `serve` binary embeds migrations at compile time; a new `.sql` alone does not recompile the metastore).

```bash
printf '\n' >> crates/egret-metastore/src/lib.rs
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p egret-metastore`
Expected: builds; migration file is embedded.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-metastore/migrations/0085_engine_delegator_secret.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(engine): add delegator_secret_id column (impersonate delegation)"
```

---

## Task 2: Engine model — delegation mode + delegator secret accessors

**Files:**
- Modify: `crates/egret-engine-mgmt/src/store.rs` (the `Engine` struct + its impl; the `SELECT` in `get_engine`/`list_engines`)

Context: `Engine` is `#[derive(sqlx::FromRow)]` with `spec: serde_json::Value`, `authz_service_id: Option<i64>`, etc. The delegation mode lives in `spec["trinoConfig"]["delegationMode"]`.

- [ ] **Step 1: Write failing tests** (add to `store.rs` `#[cfg(test)] mod tests`, or create one)

```rust
#[test]
fn delegation_mode_defaults_to_impersonate() {
    let e = Engine {
        id: Uuid::nil(), name: "e".into(), r#type: "trino".into(),
        namespace: "ns".into(), warehouse_id: Uuid::nil(),
        spec: serde_json::json!({ "trinoConfig": {} }),
        status: "provisioning".into(), authz_service_id: None,
        delegator_secret_id: None, created_at: chrono::Utc::now(),
    };
    assert_eq!(e.delegation_mode(), "impersonate");
    assert!(e.is_impersonate_delegation());
    assert!(e.supports_delegation());
}

#[test]
fn delegation_mode_jwt_is_not_impersonate() {
    let mut e = /* same as above */;
    e.spec = serde_json::json!({ "trinoConfig": { "delegationMode": "jwt" } });
    assert_eq!(e.delegation_mode(), "jwt");
    assert!(!e.is_impersonate_delegation());
    assert!(e.supports_delegation());
}
```

(Repeat the full struct literal in the second test — do not abbreviate.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p egret-engine-mgmt --lib delegation_mode`
Expected: FAIL — `delegator_secret_id` field and methods don't exist.

- [ ] **Step 3: Add the field + accessors**

Add `pub delegator_secret_id: Option<Uuid>` to the `Engine` struct (after `authz_service_id`). Add `delegator_secret_id` to the `SELECT` column list in every query that maps to `Engine` (`get_engine`, `list_engines`, and any assignment lookups — grep `authz_service_id` in `store.rs` to find them all and add the column alongside).

Add to `impl Engine`:

```rust
/// Delegation mode from the stored spec (`trinoConfig.delegationMode`).
/// Defaults to `impersonate` when absent.
pub fn delegation_mode(&self) -> &str {
    self.spec
        .get("trinoConfig")
        .and_then(|t| t.get("delegationMode"))
        .and_then(|v| v.as_str())
        .unwrap_or("impersonate")
}

/// True when delegated queries run via basic-auth service-user impersonation.
pub fn is_impersonate_delegation(&self) -> bool {
    self.delegation_mode() == "impersonate"
}

/// True when the engine can back a delegated (per-user) connection at all.
/// Impersonate mode always can; jwt mode requires JWT auth enabled.
pub fn supports_delegation(&self) -> bool {
    self.is_impersonate_delegation() || self.supports_jwt_delegation()
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p egret-engine-mgmt --lib delegation_mode`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt -p egret-engine-mgmt && cargo clippy -p egret-engine-mgmt --lib --tests
git add crates/egret-engine-mgmt/src/store.rs
git commit -m "feat(engine): delegation_mode + delegator_secret_id on Engine"
```

---

## Task 3: Seed-policy bodies (pure) — `delegator_policy.rs`

**Files:**
- Create: `crates/egret-engine-mgmt/src/delegator_policy.rs`
- Modify: `crates/egret-engine-mgmt/src/lib.rs` (add `mod delegator_policy;`)

These are the Ranger policyType-0 JSON bodies passed to `PolicyStore::create_policy(service_id, name, &body)`. `create_policy` injects `id`/`version`/`isEnabled` on download, so the body omits `id`.

- [ ] **Step 1: Write failing tests**

```rust
// crates/egret-engine-mgmt/src/delegator_policy.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impersonate_policy_grants_delegator_impersonate_any_user() {
        let p = delegator_impersonate_policy("rgr2");
        assert_eq!(p["service"], "rgr2");
        assert_eq!(p["policyType"], 0);
        assert_eq!(p["resources"]["trinouser"]["values"][0], "*");
        let item = &p["policyItems"][0];
        assert_eq!(item["users"][0], "egret_delegator");
        assert_eq!(item["accesses"][0]["type"], "impersonate");
        assert_eq!(item["accesses"][0]["isAllowed"], true);
    }

    #[test]
    fn query_timeout_policy_lets_each_user_set_the_property() {
        let p = query_timeout_policy("rgr2");
        assert_eq!(p["resources"]["systemproperty"]["values"][0], "query_max_run_time");
        let item = &p["policyItems"][0];
        assert_eq!(item["users"][0], "{USER}");
        assert_eq!(item["accesses"][0]["type"], "alter");
    }

    #[test]
    fn seed_names_are_stable() {
        assert_eq!(IMPERSONATE_POLICY_NAME, "egret-delegator-impersonate");
        assert_eq!(QUERY_TIMEOUT_POLICY_NAME, "egret-query-timeout");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p egret-engine-mgmt --lib delegator_policy`
Expected: FAIL — module doesn't exist.

- [ ] **Step 3: Implement the module**

```rust
//! Ranger seed policies for impersonate-mode delegation. `egret_delegator` may
//! impersonate any user (the trusted service user); every user may set the
//! `query_max_run_time` system session property on their own session (the
//! connector sets it on each statement). Both are stored as real, auditable
//! policies via `PolicyStore::create_policy` at provision time.

use serde_json::{json, Value};

/// The Trino service user egret authenticates as for impersonate-mode delegation.
pub const DELEGATOR_USERNAME: &str = "egret_delegator";
pub const IMPERSONATE_POLICY_NAME: &str = "egret-delegator-impersonate";
pub const QUERY_TIMEOUT_POLICY_NAME: &str = "egret-query-timeout";
const QUERY_TIMEOUT_PROPERTY: &str = "query_max_run_time";

fn policy(service: &str, name: &str, resources: Value, users: Value, access: &str) -> Value {
    json!({
        "name": name,
        "service": service,
        "serviceType": "trino",
        "policyType": 0,
        "isEnabled": true,
        "resources": resources,
        "policyItems": [{
            "users": users,
            "groups": [],
            "roles": [],
            "accesses": [{ "type": access, "isAllowed": true }],
            "delegateAdmin": false
        }],
        "denyPolicyItems": [],
        "allowExceptions": [],
        "denyExceptions": [],
        "isDenyAllElse": false,
        "policyPriority": 0
    })
}

/// `egret_delegator` may impersonate any user. Only the delegator holds this
/// grant; its password lives only in an egret secret, so no end-user can use it.
pub fn delegator_impersonate_policy(service: &str) -> Value {
    policy(
        service,
        IMPERSONATE_POLICY_NAME,
        json!({ "trinouser": { "values": ["*"], "isExcludes": false, "isRecursive": false } }),
        json!([DELEGATOR_USERNAME]),
        "impersonate",
    )
}

/// Every user may `alter` the `query_max_run_time` system property on their own
/// session (the effective query user is the impersonated end-user). Not an
/// impersonation grant — no cross-user identity or data access.
pub fn query_timeout_policy(service: &str) -> Value {
    policy(
        service,
        QUERY_TIMEOUT_POLICY_NAME,
        json!({ "systemproperty": { "values": [QUERY_TIMEOUT_PROPERTY], "isExcludes": false, "isRecursive": false } }),
        json!(["{USER}"]),
        "alter",
    )
}
```

Add `mod delegator_policy;` to `crates/egret-engine-mgmt/src/lib.rs`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p egret-engine-mgmt --lib delegator_policy`
Expected: PASS (3 tests).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt -p egret-engine-mgmt && cargo clippy -p egret-engine-mgmt --lib --tests
git add crates/egret-engine-mgmt/src/delegator_policy.rs crates/egret-engine-mgmt/src/lib.rs
git commit -m "feat(engine): delegator seed-policy bodies (impersonate + query-timeout)"
```

---

## Task 4: Provision — generate delegator secret, inject user, seed policies

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs`
- Modify: `crates/egret-engine-mgmt/src/store.rs` (add `set_delegator_secret`)

Context: `resolve_auth` (engines.rs:1153) builds `AuthReq`; `inject_monitor_user` (engines.rs:~1220) is the injection precedent; `st.secrets.put(kind, plaintext) -> Uuid` (egret-secrets `store.rs:42`) creates a secret; `create_engine`/`update_engine` build the provision req then call the provisioner + (create only) `build_provision_req`.

- [ ] **Step 1: Add `set_delegator_secret` to the store (with test)**

In `store.rs`:

```rust
/// Persist the generated delegator secret id for an engine (idempotent bookkeeping).
pub async fn set_delegator_secret(&self, engine_id: Uuid, secret_id: Uuid) -> Result<(), EngineMgmtError> {
    sqlx::query("UPDATE engine_mgmt.engines SET delegator_secret_id = $2 WHERE id = $1")
        .bind(engine_id)
        .bind(secret_id)
        .execute(&self.db.pool)
        .await?;
    Ok(())
}
```

- [ ] **Step 2: Add `inject_delegator_user` (pure-ish, testable) to engines.rs**

Mirror `inject_monitor_user`, but bcrypt a given password:

```rust
/// Inject the `egret_delegator` PASSWORD user (bcrypt of `password`) into an
/// engine's auth block so egret can basic-auth as it for impersonate-mode
/// delegated queries. No-op when auth is `None` (must not turn auth on).
fn inject_delegator_user(auth: &mut Option<AuthReq>, password: &str) -> Result<(), CpError> {
    let Some(auth) = auth.as_mut() else { return Ok(()) };
    let bcrypt_hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
        .map_err(|e| CpError::BadRequest(format!("bcrypt delegator password: {e}")))?
        .replacen("$2b$", "$2y$", 1);
    auth.password_users.push(PasswordEntryReq {
        username: crate::delegator_policy::DELEGATOR_USERNAME.into(),
        bcrypt_hash,
    });
    Ok(())
}
```

- [ ] **Step 3: Add `ensure_delegator_secret` (generate once, idempotent)**

```rust
/// Ensure the engine has a stored `egret_delegator` password secret; generate one
/// on first call and persist its id on the engine. Returns the plaintext (for
/// bcrypting into `password.db`) and the secret id (for materialization).
async fn ensure_delegator_secret(st: &CpState, engine: &Engine) -> Result<(Uuid, String), CpError> {
    if let Some(id) = engine.delegator_secret_id {
        let pw = st.secrets.resolve_plaintext(None, id).await
            .map_err(|e| CpError::BadRequest(format!("resolve delegator secret: {e}")))?;
        return Ok((id, pw));
    }
    // 32-byte URL-safe random password.
    let pw: String = {
        use rand::RngCore;
        let mut b = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut b);
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
    };
    let id = st.secrets.put("engine-delegator", &pw).await
        .map_err(|e| CpError::BadRequest(format!("store delegator secret: {e}")))?;
    st.engine_mgmt.set_delegator_secret(engine.id, id).await?;
    Ok((id, pw))
}
```

(Confirm `rand` + `base64` are workspace deps of `egret-engine-mgmt`; both are already used elsewhere in the workspace — add to `Cargo.toml` if missing. `base64::engine::general_purpose::URL_SAFE_NO_PAD` is the 0.22 API.)

- [ ] **Step 4: Add `seed_delegator_policies`**

```rust
/// Seed the two impersonate-mode Ranger policies on the engine's bound trino
/// service. Idempotent: skips a policy whose name already exists on the service.
async fn seed_delegator_policies(st: &CpState, service_name: &str) -> Result<(), CpError> {
    use crate::delegator_policy as dp;
    let Some(svc) = st.policy_store.get_service_opt(service_name).await
        .map_err(|e| CpError::BadRequest(format!("policy service: {e}")))? else { return Ok(()) };
    for (name, body) in [
        (dp::IMPERSONATE_POLICY_NAME, dp::delegator_impersonate_policy(service_name)),
        (dp::QUERY_TIMEOUT_POLICY_NAME, dp::query_timeout_policy(service_name)),
    ] {
        if st.policy_store.policy_exists(svc.id, name).await
            .map_err(|e| CpError::BadRequest(format!("policy exists: {e}")))? { continue; }
        st.policy_store.create_policy(svc.id, name, &body).await
            .map_err(|e| CpError::BadRequest(format!("seed policy: {e}")))?;
    }
    Ok(())
}
```

If `PolicyStore` lacks `get_service_opt`/`policy_exists`, add thin wrappers in `crates/egret-policy/src/store.rs` (a `SELECT id FROM policy.authz_services WHERE name=$1`, and `SELECT EXISTS(SELECT 1 FROM policy.authz_policies WHERE service_id=$1 AND name=$2)`), each with a one-line unit or integration test. Confirm `CpState` exposes `policy_store: PolicyStore` (state.rs:28 references a policy store for auto-bind — reuse it; add the field if only a narrower handle exists).

- [ ] **Step 5: Wire the injection into `build_provision_req`**

`build_provision_req` (engines.rs:461) is where `resolve_auth` runs and `inject_monitor_user(&mut auth, engine.id)?` is already called, and where the ranger `service_name` for `RangerBinding` is computed — do all delegator wiring here so it applies to both `create_engine` and `update_engine` (both call `build_provision_req`). Immediately **after** the `inject_monitor_user(...)` call:

```rust
if engine.is_impersonate_delegation() {
    let (_secret_id, pw) = ensure_delegator_secret(st, engine).await?;
    inject_delegator_user(&mut auth, &pw)?;
    if let Some(service_name) = ranger_service_name.as_deref() {
        // `ranger_service_name` = the same value build_provision_req assigns to
        // RangerBinding.service_name (None when the engine has no ranger binding).
        seed_delegator_policies(st, service_name).await?;
    }
}
```

`ensure_delegator_secret`/`inject_delegator_user` require an enabled auth block; `inject_delegator_user` is a no-op when `auth` is `None`, and `ensure_delegator_secret` is only reached inside the impersonate branch — guard the whole block with `if let Some(auth_ref) = auth.as_ref()` if `build_provision_req` can produce `auth = None`. Use the local variable `build_provision_req` already binds for the ranger service name (grep `RangerBinding` / `service_name:` within engines.rs:461–560); do not recompute it.

- [ ] **Step 6: Provisioner test — delegator user rendered**

In `crates/egret-orchestrator/src/trino.rs` tests, extend `auth_config_password_and_jwt` (or add a test) asserting that when `password_users` contains `egret_delegator`, `password.db` contains `egret_delegator:$2y$`. (The renderer already emits every `password_users` entry — this guards the wiring end shape.)

```rust
assert!(cm["data"]["password.db"].as_str().unwrap().contains("egret_delegator:$2y$"));
```

(Add an `egret_delegator` `PasswordEntry` to that test's `password_users` vec.)

- [ ] **Step 7: Run tests**

Run: `cargo test -p egret-engine-mgmt -p egret-orchestrator --lib`
Expected: PASS. Then `cargo build -p egret-bin` to confirm the whole wiring compiles.

- [ ] **Step 8: fmt + clippy + commit**

```bash
cargo fmt -p egret-engine-mgmt -p egret-policy && cargo clippy -p egret-engine-mgmt --lib --tests
git add crates/egret-engine-mgmt crates/egret-policy crates/egret-orchestrator
git commit -m "feat(engine): provision egret_delegator user + secret + seed Ranger policies"
```

---

## Task 5: Materialize the delegated connection with the delegator secret + impersonate options

**Files:**
- Modify: `crates/egret-query/src/registry/repo.rs` (`create_provisioned`)
- Modify: `crates/egret-engine-mgmt/src/assignment.rs`

- [ ] **Step 1: Add `secret_id` to `create_provisioned`**

Change the signature (repo.rs:139) to accept `secret_id: Option<Uuid>` and bind it instead of the hardcoded `NULL`:

```rust
pub async fn create_provisioned(
    &self,
    workspace_id: Uuid,
    engine_id: Uuid,
    name: &str,
    engine: &str,
    host: &str,
    port: i32,
    options: &serde_json::Value,
    secret_id: Option<Uuid>,
    created_by: Option<UserId>,
) -> Result<Database, DatabaseError> {
    sqlx::query_as::<_, Row>(
        "INSERT INTO query.databases \
           (name, engine, host, port, options, secret_id, created_by, workspace_id, delegated, engine_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, true, $9) \
         RETURNING id, name, engine, host, port, options, secret_id, status, delegated, engine_id",
    )
    .bind(name).bind(engine).bind(host).bind(port).bind(options)
    .bind(secret_id)
    .bind(created_by.map(|u| u.0))
    .bind(workspace_id).bind(engine_id)
    .fetch_one(&self.db.pool).await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => DatabaseError::AlreadyExists(name.to_string()),
        _ => DatabaseError::Db(e),
    })?
    .into_database()
}
```

- [ ] **Step 2: Update the assignment call site + gate (assignment.rs)**

Replace the `supports_jwt_delegation()` gate (assignment.rs:50) with `supports_delegation()`. Replace the `opts`/`create_provisioned` block (assignment.rs:69–83):

```rust
let name = dedup_name(&engine.name, &existing);
let (opts, secret_id) = if engine.is_impersonate_delegation() {
    (
        serde_json::json!({ "user": crate::delegator_policy::DELEGATOR_USERNAME, "impersonate": true }),
        engine.delegator_secret_id, // Some(..) after impersonate re-provision
    )
} else {
    (serde_json::json!({}), None) // jwt mode: mint path, no stored secret
};
self.databases
    .create_provisioned(
        workspace_id, engine_id, &name, &engine.r#type,
        &engine.coordinator_host(), engine.coordinator_port(),
        &opts, secret_id, Some(egret_core::UserId(by)),
    )
    .await
    .map_err(to_mgmt_err)?;
```

- [ ] **Step 3: Fix the other `create_provisioned` caller(s)**

Grep `create_provisioned(` across the workspace; every other call (and any test) must pass the new `secret_id` arg (`None` unless impersonate). Update tests to the new arity.

- [ ] **Step 4: Assignment unit test — impersonate materialization**

Add/extend a test in `assignment.rs` (or its test module) with a fake `DatabaseRepo` capturing the args, asserting: for an impersonate engine with `delegator_secret_id = Some(s)`, `create_provisioned` is called with `options == {"user":"egret_delegator","impersonate":true}` and `secret_id == Some(s)`; for a jwt engine, `options == {}` and `secret_id == None`.

- [ ] **Step 5: Run tests + build**

Run: `cargo test -p egret-query -p egret-engine-mgmt --lib` then `cargo build -p egret-bin`
Expected: PASS + builds.

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt -p egret-query -p egret-engine-mgmt && cargo clippy -p egret-query -p egret-engine-mgmt --lib --tests
git add crates/egret-query/src/registry/repo.rs crates/egret-engine-mgmt/src/assignment.rs
git commit -m "feat(engine): materialize impersonate delegated connection with delegator secret"
```

---

## Task 6: Query route — impersonate branch (run as caller, no mint)

**Files:**
- Modify: `crates/egret-query/src/execution/routes.rs` (the delegated branch, ~lines 123–144)

Context (current): `if db.delegated { mint JWT; session_user = ensure_username; (Some(username), Some(token.access_token)) } else { (Some(email), None) }`. `db.options` is a `ConnOptions` with an `impersonate: Option<bool>` field. The job-submit path opens `db.secret_id` and passes it to the connector as `secret`.

- [ ] **Step 1: Write the failing test** (integration test `crates/egret-query/tests/execution_http.rs` — the file already has a capturing `ConnectorFactory` recording `(session_user, bearer_token)`)

Add a test: a delegated database whose `options.impersonate == true` and `secret_id = Some(..)` submits a query → the captured `(session_user, bearer)` is `(Some(caller_username), None)` (NOT a minted JWT), i.e. the impersonate path runs as the caller with no bearer. Mirror the existing delegated test's setup but set `impersonate` + a secret on the DB row and assert `bearer == None`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p egret-query --test execution_http impersonate`
Expected: FAIL — impersonate DB still takes the mint path (bearer is `Some`).

- [ ] **Step 3: Implement the branch**

Replace the delegated computation:

```rust
let (session_user, bearer) = if db.delegated {
    if db.options.impersonate == Some(true) {
        // Impersonate mode: egret authenticates as the service user (basic-auth
        // from db.secret_id) and runs AS the caller via X-Trino-User. No JWT.
        let username = st.users.ensure_username(ctx.principal.user_id).await
            .map_err(EngineApiError::from)?;
        (Some(username), None)
    } else {
        // jwt mode: mint the caller's short-lived engine JWT (403 if non-OIDC).
        let token = st.engine_minter.mint(ctx.principal.user_id.0).await
            .map_err(|e| match e {
                egret_analytics_api::engine_token::BrokerError::NoRefreshToken =>
                    EngineApiError::from(egret_core::Error::Forbidden),
                other => EngineApiError::from(egret_core::Error::Internal(anyhow::anyhow!(other.to_string()))),
            })?;
        let username = st.users.ensure_username(ctx.principal.user_id).await
            .map_err(EngineApiError::from)?;
        (Some(username), Some(token.access_token))
    }
} else {
    (Some(ctx.principal.email.clone()), None)
};
```

(The connector already applies `impersonate` from `db.options`; the impersonate branch only needs `session_user` + no bearer. The service secret flows through the existing `secret_id` open path — do not change it.)

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p egret-query --test execution_http`
Expected: PASS (existing delegated/non-delegated tests still green; new impersonate test green).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt -p egret-query && cargo clippy -p egret-query --lib --tests
git add crates/egret-query/src/execution/routes.rs
git commit -m "feat(query): impersonate delegated queries run as caller (no JWT mint)"
```

---

## Final verification (whole feature)

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets` (no new warnings in touched crates)
- [ ] `cargo test -p egret-engine-mgmt -p egret-query -p egret-orchestrator -p egret-policy`
- [ ] `cargo build -p egret-bin`
- [ ] **Live (manual, per [[egret-manual-trino-integration-test]] / delegated smoke recipe):** re-provision a Ranger engine (default impersonate) via `PUT /api/control-plane/engines/{id}`; confirm `password.db` has `egret_delegator`, the two seed policies exist on the service, and the materialized delegated DB row has `secret_id` + `options.impersonate`. Assign to a workspace, submit a query as an end-user, assert `SELECT current_user` = the caller and that the caller's Ranger masking applies. (egret sets `X-Trino-User`; a different end-user cannot be run by construction.)

---

## Notes for the executor

- **Migration staleness:** touching `egret-metastore/src/lib.rs` (Task 1) is required or a running `serve` uses a stale embedded migrator.
- **Do not modify `TrinoConnector`** — its basic-auth + `impersonate` + `X-Trino-User=session_user` path already does the right thing.
- **jwt mode is untouched here** — the mint path stays for the follow-up TLS jwt spec.
- **Security invariant to preserve:** `X-Trino-User`/`session_user` is always derived from `ctx.principal` (the authenticated egret user), never from client input.
