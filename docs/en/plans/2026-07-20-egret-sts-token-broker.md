# Egret STS / Token Broker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make egret's machine-IdP the single issuer of per-user engine tokens (like catalog tokens already work), so any authenticated user — local or SSO — can query engines without a Keycloak refresh token.

**Architecture:** Replace the Keycloak refresh-grant `EngineTokenMinter` with one that signs an `aud:"engine"` JWT via `PgSigningKeyStore` + `sign_claims`, sourcing `preferred_username`/groups from egret's DB. Point every engine's JWT trust at egret's own JWKS/issuer (unconditional of OIDC config). Consumers (kernel/SQL Lab/BI) are unchanged — they go through the `EngineTokenMinter` trait.

**Tech Stack:** Rust (axum, sqlx, jsonwebtoken RS256), `egret-machine-idp`, `egret-auth`, `egret-engine-mgmt`; Playwright e2e on kind; StarRocks 4.1 / Trino JWT auth.

**Spec:** `docs/en/specs/2026-07-20-egret-sts-token-broker-design.md`

---

## File Structure

| File | Responsibility | Change |
| --- | --- | --- |
| `crates/egret-auth/src/groups.rs` | group membership queries | Add `GroupRepo::group_names(user_id) -> Vec<String>` |
| `crates/egret-bin/src/engine_minter.rs` | **new** — machine-IdP `EngineTokenMinter` + pure `engine_claims` | Create (replaces `engine_broker.rs`) |
| `crates/egret-bin/src/engine_broker.rs` | old Keycloak minter | Delete |
| `crates/egret-bin/src/lib.rs` | module exports | swap `engine_broker` → `engine_minter` |
| `crates/egret-bin/src/main.rs` | composition root: minter wiring (`:1151`) + engine trust (`:1514`) | Modify |
| `crates/egret-engine-mgmt/src/engines.rs` | StarRocks JWT integration DDL (`:2358` audience) | `"starrocks"` → `"engine"` |
| `crates/egret-auth/src/oidc_routes.rs` | OIDC login callback | Persist groups (JIT write-through) |
| `crates/egret-auth/src/session.rs` | session lifetime | Sliding + idle + disable-invalidates |
| `frontend/tests/e2e/sts-broker-kind.spec.ts` | **new** live e2e | Create |
| `justfile` | e2e recipe | Add `e2e-kind-sts` |

---

## Task 1: Group-names query + pure `engine_claims` builder

**Files:**
- Modify: `crates/egret-auth/src/groups.rs`
- Create: `crates/egret-bin/src/engine_minter.rs`
- Test: inline `#[cfg(test)]` in both

- [ ] **Step 1: Write the failing test for `engine_claims`**

Create `crates/egret-bin/src/engine_minter.rs` with only the test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_claims_shape() {
        let c = engine_claims("alice", &["data-team".into()], "https://egret.example/", 1_000, 900);
        assert_eq!(c["iss"], "https://egret.example"); // trailing slash trimmed
        assert_eq!(c["sub"], "alice");
        assert_eq!(c["preferred_username"], "alice");
        assert_eq!(c["aud"], "engine");
        assert_eq!(c["groups"], serde_json::json!(["data-team"]));
        assert_eq!(c["iat"], 1_000);
        assert_eq!(c["exp"], 1_900);
    }

    #[test]
    fn engine_claims_empty_groups_is_empty_array() {
        let c = engine_claims("bob", &[], "https://e/", 5, 10);
        assert_eq!(c["groups"], serde_json::json!([]));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p egret-bin engine_claims_shape`
Expected: FAIL — `cannot find function engine_claims`.

- [ ] **Step 3: Implement `engine_claims`**

At the top of `crates/egret-bin/src/engine_minter.rs`:

```rust
//! Machine-IdP `EngineTokenMinter`: sign a per-user `aud:"engine"` JWT with
//! egret's own key (no Keycloak, no refresh token). Mirrors the catalog mint
//! (`egret-notebook/src/runtime.rs::mint_catalog_jwt`).

/// Build the `aud:"engine"` claim set. Pure (no crypto/DB) so the claim contract
/// is unit-testable. `preferred_username` MUST equal `UserRepo::ensure_username`
/// so StarRocks/Trino `principal_field=preferred_username` + Ranger match.
pub(crate) fn engine_claims(
    username: &str,
    groups: &[String],
    issuer: &str,
    iat_ts: i64,
    ttl_secs: i64,
) -> serde_json::Value {
    serde_json::json!({
        "iss": issuer.trim_end_matches('/'),
        "sub": username,
        "preferred_username": username,
        "groups": groups,
        "aud": "engine",
        "iat": iat_ts,
        "exp": iat_ts + ttl_secs,
    })
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p egret-bin engine_claims`
Expected: PASS (2 tests).

- [ ] **Step 5: Write the failing test for `GroupRepo::group_names`**

In `crates/egret-auth/src/groups.rs` `#[cfg(test)]` module (follow the existing `sqlx::test` pattern in that file):

```rust
    #[sqlx::test]
    async fn group_names_returns_membership(pool: sqlx::PgPool) {
        let repo = GroupRepo::new(Db::from_pool(pool.clone()));
        let uid = seed_user(&pool, "u@egret.local").await;
        let gid = repo.create("data-team").await.unwrap();
        repo.add_member(gid, uid).await.unwrap();
        let names = repo.group_names(uid).await.unwrap();
        assert_eq!(names, vec!["data-team".to_string()]);
    }
```

(Reuse the file's existing `seed_user` / `Db::from_pool` helpers; if `create`/`add_member` differ in name, use the file's actual constructors — they're already exercised by neighboring tests.)

- [ ] **Step 6: Run it to verify it fails**

Run: `cargo test -p egret-auth group_names_returns_membership`
Expected: FAIL — `no method named group_names`.

- [ ] **Step 7: Implement `GroupRepo::group_names`**

In `crates/egret-auth/src/groups.rs` (the query already exists inline at line ~173; extract it as a method):

```rust
    /// Group names a user belongs to (for engine-token `groups` claim).
    pub async fn group_names(&self, user_id: UserId) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id \
             WHERE ug.user_id = $1 ORDER BY g.name",
        )
        .bind(user_id.0)
        .fetch_all(self.db.pool())
        .await?;
        Ok(rows.into_iter().map(|(n,)| n).collect())
    }
```

(Match `UserId`'s field access + `self.db.pool()` to the file's existing convention — copy from a neighboring method in the same file.)

- [ ] **Step 8: Run it to verify it passes**

Run: `cargo test -p egret-auth group_names_returns_membership`
Expected: PASS.

- [ ] **Step 9: Register the new module**

In `crates/egret-bin/src/lib.rs`, add `pub mod engine_minter;` (keep `pub mod engine_broker;` for now — deleted in Task 4).

- [ ] **Step 10: Commit**

```bash
git add crates/egret-auth/src/groups.rs crates/egret-bin/src/engine_minter.rs crates/egret-bin/src/lib.rs
git commit -m "feat(sts): engine_claims builder + GroupRepo::group_names"
```

---

## Task 2: `MachineIdpEngineMinter` (the new `EngineTokenMinter`)

**Files:**
- Modify: `crates/egret-bin/src/engine_minter.rs`
- Test: `crates/egret-bin/tests/engine_minter.rs` (new, DB-backed)

- [ ] **Step 1: Write the failing integration test**

Create `crates/egret-bin/tests/engine_minter.rs`:

```rust
//! MachineIdpEngineMinter mints an egret-signed engine JWT for ANY user —
//! including a local user with no refresh token (the regression this fixes).
use egret_bin::engine_minter::MachineIdpEngineMinter;
use egret_notebook::engine_token::EngineTokenMinter;

#[sqlx::test]
async fn mints_egret_signed_token_for_local_user(pool: sqlx::PgPool) {
    let db = egret_metastore::Db::from_pool(pool.clone());
    // A local user with NO oidc_refresh_tokens row (the failing case today).
    let uid = seed_local_user(&pool, "admin@egret.local").await;
    let keys = egret_machine_idp::PgSigningKeyStore::new(db.clone());

    let minter = MachineIdpEngineMinter::new(
        keys.clone(),
        egret_auth::users::UserRepo::new(db.clone()),
        egret_auth::groups::GroupRepo::new(db.clone()),
        "https://egret.example".into(),
        900,
    );

    let tok = minter.mint(uid).await.expect("mint must succeed with no refresh token");

    // Verify signature + claims against the active JWKS.
    let jwk = active_public_jwk(&keys).await;
    let claims = egret_machine_idp::verify_with_jwk(&tok.access_token, &jwk).unwrap();
    assert_eq!(claims["aud"], "engine");
    assert_eq!(claims["preferred_username"], "admin"); // ensure_username slug
    assert!(tok.expires_at > chrono::Utc::now());
}
```

Add the two helpers at the bottom of the test file (mirror `seed_user` from `groups.rs` tests; `active_public_jwk` calls `keys.active()` and returns `km.public_jwk`).

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p egret-bin --test engine_minter`
Expected: FAIL — `cannot find struct MachineIdpEngineMinter`.

- [ ] **Step 3: Implement `MachineIdpEngineMinter`**

Append to `crates/egret-bin/src/engine_minter.rs`:

```rust
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use egret_auth::groups::GroupRepo;
use egret_auth::users::UserRepo;
use egret_machine_idp::{sign_claims, PgSigningKeyStore, SigningKeyStore};
use egret_notebook::engine_token::{BrokerError, EngineToken, EngineTokenMinter};
use uuid::Uuid;

pub struct MachineIdpEngineMinter {
    keys: PgSigningKeyStore,
    users: UserRepo,
    groups: GroupRepo,
    issuer: String,
    ttl_secs: i64,
}

impl MachineIdpEngineMinter {
    pub fn new(
        keys: PgSigningKeyStore,
        users: UserRepo,
        groups: GroupRepo,
        issuer: String,
        ttl_secs: i64,
    ) -> Self {
        Self { keys, users, groups, issuer, ttl_secs }
    }
}

#[async_trait]
impl EngineTokenMinter for MachineIdpEngineMinter {
    async fn mint(&self, user_id: Uuid) -> Result<EngineToken, BrokerError> {
        let idp = |e: String| BrokerError::Idp(e);
        let username = self
            .users
            .ensure_username(egret_auth::UserId(user_id))
            .await
            .map_err(|e| idp(e.to_string()))?;
        let groups = self
            .groups
            .group_names(egret_auth::UserId(user_id))
            .await
            .map_err(|e| idp(e.to_string()))?;
        let km = self.keys.active().await.map_err(|e| idp(e.to_string()))?;
        let iat = Utc::now().timestamp();
        let claims = engine_claims(&username, &groups, &self.issuer, iat, self.ttl_secs);
        let jwt = sign_claims(&km, &claims).map_err(|e| idp(e.to_string()))?;
        let expires_at: DateTime<Utc> =
            DateTime::from_timestamp(iat + self.ttl_secs, 0).ok_or_else(|| idp("exp out of range".into()))?;
        Ok(EngineToken { access_token: jwt, expires_at })
    }
}
```

(Adjust `egret_auth::UserId(user_id)` to the crate's actual `UserId` constructor — check `users.rs`. Adjust `UserRepo::new`/`GroupRepo::new` signatures to the file's real constructors.)

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p egret-bin --test engine_minter`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-bin/src/engine_minter.rs crates/egret-bin/tests/engine_minter.rs
git commit -m "feat(sts): MachineIdpEngineMinter signs aud:engine JWT (no refresh token)"
```

---

## Task 3: Point engine JWT trust at egret's machine-IdP

**Files:**
- Modify: `crates/egret-bin/src/main.rs:1511-1530` (engine_jwks_uri / engine_oidc_issuer)
- Modify: `crates/egret-engine-mgmt/src/engines.rs:~2358` (StarRocks audience)
- Test: `crates/egret-engine-mgmt/src/engines.rs` DDL-template test

- [ ] **Step 1: Write the failing test for the StarRocks audience**

In `engines.rs` tests, assert the JWT integration DDL requires `aud:"engine"`:

```rust
    #[test]
    fn jwt_integration_requires_engine_audience() {
        let stmts = starrocks_jwt_integration_sql(
            "https://egret/.well-known/jwks.json",
            "https://egret",
            "engine",
        );
        assert!(stmts[0].contains("\"required_audience\"=\"engine\""));
        assert!(stmts[0].contains("\"required_issuer\"=\"https://egret\""));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p egret-engine-mgmt jwt_integration_requires_engine_audience`
Expected: currently PASS for the string-format but the CALLER passes `"starrocks"`. So instead assert at the call site. Change the test to build the provision req and assert audience — OR simpler, verify the constant. Run and confirm it compiles; if the helper already takes `audience` as a param this passes trivially — the real change is the caller (Step 3).

- [ ] **Step 3: Change the audience the caller passes**

In `crates/egret-engine-mgmt/src/engines.rs` (both `StarRocksDelegation` and `StarRocksDelegationBootstrap` construction, ~line 2358 and ~2366), change:

```rust
        audience: "starrocks".into(),
```

to:

```rust
        audience: "engine".into(),
```

(both occurrences). For Trino: find where `trinoConfig.auth.jwt.requiredAudience` / the jwt config audience is set in `build_provision_req` and set it to `"engine"` too (grep `requiredAudience` / `required_audience` in `engines.rs`).

- [ ] **Step 4: Point the trust config at the machine-IdP (unconditional)**

In `crates/egret-bin/src/main.rs`, replace the OIDC-gated block at `:1514-1530`:

```rust
        engine_jwks_uri: if config.oidc.enabled && !config.oidc.issuer_url.is_empty() {
            format!("{}/protocol/openid-connect/certs", config.oidc.issuer_url.trim_end_matches('/'))
        } else { String::new() },
        engine_oidc_issuer: if config.oidc.enabled { config.oidc.issuer_url.clone() } else { String::new() },
```

with (egret's machine-IdP issuer = `config.catalog.public_url`, same value used for `idp_issuer`/`IdpState.issuer`):

```rust
        // Engines trust egret's OWN machine-IdP (the STS): the same issuer that
        // signs catalog + engine tokens, independent of whether external OIDC is
        // configured. JWKS lives at `/.well-known/jwks.json`.
        engine_jwks_uri: format!(
            "{}/.well-known/jwks.json",
            egret_public_url.trim_end_matches('/')
        ),
        engine_oidc_issuer: egret_public_url.clone(),
```

where, just above this struct literal, bind:

```rust
        let egret_public_url = config
            .catalog
            .public_url
            .clone()
            .unwrap_or_else(|| "http://localhost:8080".to_string());
```

- [ ] **Step 5: Run the tests + build**

Run: `cargo test -p egret-engine-mgmt jwt_integration && cargo build -p egret-bin`
Expected: PASS + builds.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-bin/src/main.rs crates/egret-engine-mgmt/src/engines.rs
git commit -m "feat(sts): engines trust egret machine-IdP issuer + aud:engine (unconditional of OIDC)"
```

---

## Task 4: Swap the minter; delete the Keycloak broker

**Files:**
- Modify: `crates/egret-bin/src/main.rs:1151-1160`
- Delete: `crates/egret-bin/src/engine_broker.rs`
- Modify: `crates/egret-bin/src/lib.rs`

- [ ] **Step 1: Swap the construction**

In `crates/egret-bin/src/main.rs`, replace the `engine_minter` block at `:1151`:

```rust
    let engine_minter: std::sync::Arc<dyn egret_notebook::engine_token::EngineTokenMinter> = {
        use egret_bin::engine_broker::{EngineTokenBroker, KeycloakRefresher};
        std::sync::Arc::new(EngineTokenBroker::new( /* secrets, repo, KeycloakRefresher{..} */ ))
    };
```

with:

```rust
    let engine_minter: std::sync::Arc<dyn egret_notebook::engine_token::EngineTokenMinter> =
        std::sync::Arc::new(egret_bin::engine_minter::MachineIdpEngineMinter::new(
            egret_machine_idp::PgSigningKeyStore::new(db.clone()),
            egret_auth::users::UserRepo::new(db.clone()),
            egret_auth::groups::GroupRepo::new(db.clone()),
            egret_public_url.clone(), // same binding added in Task 3 Step 4
            900,                      // 15-min engine-token TTL (revocation granularity)
        ));
```

Ensure `egret_public_url` is bound before this line (move the Task 3 Step 4 `let egret_public_url = …` up so both uses see it).

- [ ] **Step 2: Delete the Keycloak broker + its export**

```bash
git rm crates/egret-bin/src/engine_broker.rs
```

In `crates/egret-bin/src/lib.rs`, remove `pub mod engine_broker;`. Delete `crates/egret-bin/tests/engine_broker.rs` if it references the removed type (`git rm`).

- [ ] **Step 3: Build the workspace + run consumer tests**

Run: `cargo build --workspace 2>&1 | tail -5`
Expected: builds (no reference to `engine_broker`).
Run: `cargo test -p egret-kernel -p egret-notebook -p egret-bi 2>&1 | grep "test result"`
Expected: all pass — consumers use the trait, not the impl.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(sts): wire MachineIdpEngineMinter; remove Keycloak engine broker"
```

---

## Task 5: JIT group write-through on OIDC login

**Files:**
- Modify: `crates/egret-auth/src/oidc_routes.rs` (login callback)
- Test: `crates/egret-auth/tests/oidc_jit.rs` (new) or inline

- [ ] **Step 1: Write the failing test**

Assert that after a callback with a `groups` claim, `user_groups` contains those groups (so the engine token later carries them). Follow the existing OIDC-callback test harness in `egret-auth` (search `tests/oidc_keycloak.rs` for the callback helper):

```rust
    #[sqlx::test]
    async fn oidc_callback_persists_groups_to_user_groups(pool: sqlx::PgPool) {
        let db = egret_metastore::Db::from_pool(pool.clone());
        let uid = handle_callback_with_claims(&db, "u@egret.local", &["data-team", "analysts"]).await;
        let names = egret_auth::groups::GroupRepo::new(db).group_names(egret_auth::UserId(uid)).await.unwrap();
        assert_eq!(names, vec!["analysts".to_string(), "data-team".to_string()]); // sorted
    }
```

- [ ] **Step 2: Run it to verify it fails / or confirm existing behavior**

Run: `cargo test -p egret-auth oidc_callback_persists_groups`
Expected: FAIL if group sync is not persisted (roles.rs:159 says "OIDC groups sync into user_groups" — if already done, this test PASSES and Task 5 is a no-op verification; if it only reads the claim transiently, it FAILS).

- [ ] **Step 3: Implement group write-through (only if Step 2 failed)**

In `oidc_routes.rs` callback, after create-or-link user, upsert groups. Use `GroupRepo`'s existing `set_user_groups`/replace pattern (groups.rs:52 `DELETE … WHERE user_id` + `INSERT`):

```rust
    // JIT: mirror the IdP's group claim into egret's user_groups so egret-minted
    // engine tokens carry groups (egret DB is the runtime authz source).
    let group_names: Vec<String> = id_token_groups(&claims); // existing claim extractor
    groups.replace_user_groups(user_id, &group_names).await?; // creates missing groups + rebinds
```

If `replace_user_groups` doesn't exist, add it to `groups.rs` mirroring the DELETE+INSERT at lines 52-61 (create-group-if-missing by name, then bind). Keep it idempotent.

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p egret-auth oidc_callback_persists_groups`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/oidc_routes.rs crates/egret-auth/src/groups.rs crates/egret-auth/tests/oidc_jit.rs
git commit -m "feat(sts): JIT-persist OIDC groups to user_groups for engine-token claims"
```

---

## Task 6: Independent session lifetime (sliding + idle + disable-invalidates)

**Files:**
- Modify: `crates/egret-auth/src/session.rs`
- Test: inline `#[cfg(test)]` in `session.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[sqlx::test]
    async fn disabled_user_session_is_invalid(pool: sqlx::PgPool) {
        let repo = SessionRepo::new(egret_metastore::Db::from_pool(pool.clone()));
        let uid = seed_user(&pool, "u@egret.local").await;
        let sid = repo.create(uid).await.unwrap();
        disable_user(&pool, uid).await;
        assert!(repo.resolve(sid).await.unwrap().is_none()); // disabled → no identity
    }

    #[sqlx::test]
    async fn idle_session_expires(pool: sqlx::PgPool) {
        let repo = SessionRepo::new(egret_metastore::Db::from_pool(pool.clone()));
        let uid = seed_user(&pool, "u2@egret.local").await;
        let sid = repo.create(uid).await.unwrap();
        set_last_seen(&pool, sid, chrono::Utc::now() - chrono::Duration::hours(9)).await; // > idle window
        assert!(repo.resolve(sid).await.unwrap().is_none());
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p egret-auth disabled_user_session_is_invalid idle_session_expires`
Expected: FAIL (idle/disable checks not enforced in `resolve`).

- [ ] **Step 3: Enforce sliding + idle + disable in `resolve`**

In `session.rs::resolve`, add to the session lookup query (or post-filter): reject when the user is disabled, or `last_seen_at < now() - IDLE_WINDOW`, or `created_at < now() - MAX_LIFETIME`; on success, bump `last_seen_at` (sliding):

```rust
    const IDLE_WINDOW: chrono::Duration = chrono::Duration::hours(8);
    const MAX_LIFETIME: chrono::Duration = chrono::Duration::hours(24);
    // in resolve(): after fetching the row,
    if user.disabled
        || row.last_seen_at < now - IDLE_WINDOW
        || row.created_at < now - MAX_LIFETIME
    {
        return Ok(None);
    }
    self.touch_last_seen(session_id, now).await?; // sliding
```

Add a `last_seen_at TIMESTAMPTZ` column to `sessions` via a new migration if absent (`crates/egret-metastore/migrations/00XX_session_last_seen.sql`, `ALTER TABLE sessions ADD COLUMN last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()`), then `touch metastore/src/lib.rs` (per the sqlx-migration-staleness note).

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p egret-auth disabled_user_session_is_invalid idle_session_expires`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/session.rs crates/egret-metastore/migrations/ crates/egret-metastore/src/lib.rs
git commit -m "feat(sts): independent egret session (sliding + idle + disable-invalidates)"
```

---

## Task 7: Live e2e — local admin, SSO parity, OIDC-off

**Files:**
- Create: `frontend/tests/e2e/sts-broker-kind.spec.ts`
- Modify: `justfile` (add `e2e-kind-sts`)

- [ ] **Step 1: Write the e2e spec**

```ts
// Proves the broker end-to-end: engine tokens are egret-signed, no Keycloak
// refresh token required. Opt-in (EGRET_E2E_KIND=1), Ranger-on deploy.
import { test, expect } from "@playwright/test";
import { provisionStarRocksJwt, enableNotebookEngine, runNotebookCell,
         loginLocalAdmin, loginViaOidc } from "./helpers/sts";

test("local admin queries StarRocks via egret-minted token", async ({ browser }) => {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await loginLocalAdmin(page); // admin@egret.local — NO refresh token
  const eng = await provisionStarRocksJwt(page, "sts-sr");
  await enableNotebookEngine(page, "demo", eng.id);
  const out = await runNotebookCell(page, "demo",
    `df = starrocks("SELECT 1 AS one", engine="sts-sr"); display(df)`);
  expect(out).toContain("1 row"); // GREEN ROW without Keycloak
});

test("SSO enforcement parity: admin full vs member masked", async ({ browser }) => {
  // admin-sso (OIDC → JIT) sees full rows; member-sso masked — same as pre-broker.
  // Reuse the per-user-enforcement assertions, now driven by egret-minted tokens.
});
```

Add `frontend/tests/e2e/helpers/sts.ts` reusing the existing helpers from `per-user-enforcement-kind.spec.ts` (login, provision, notebook run).

- [ ] **Step 2: Add the recipe**

In `justfile`, after `e2e-kind-per-user`:

```make
# Opt-in STS/broker e2e: local admin + SSO both query via egret-minted engine tokens.
e2e-kind-sts: deploy-kind-governance
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
      EGRET_E2E_BUCKET=e2e EGRET_E2E_S3_ENDPOINT=http://minio.egret-system.svc.cluster.local:9000 \
      npx playwright test sts-broker-kind
```

- [ ] **Step 3: Run it live**

Run: `just e2e-kind-sts`
Expected: both tests pass; local-admin test shows the green row with **no** `NoRefreshToken` in backend logs.

- [ ] **Step 4: Commit**

```bash
git add frontend/tests/e2e/sts-broker-kind.spec.ts frontend/tests/e2e/helpers/sts.ts justfile
git commit -m "test(sts): live e2e — local admin + SSO parity via egret-minted engine tokens"
```

---

## Final verification (before finishing the branch)

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace` (DATABASE_URL set; the only expected non-run is env-gated DB tests without a DB)
- [ ] `cd frontend && npm run lint && npx tsc --noEmit && npm run build`
- [ ] `just e2e-kind-sts` green
- [ ] Backend logs during the local-admin query contain **no** `no stored refresh token for user`

## Rollout notes (not tasks — for the operator)

- **Existing engines** trusting Keycloak must be re-provisioned (dev) or dual-trusted then cut over (prod): add an `egret_machine` security integration pointing at egret's JWKS alongside `egret_oidc`, converge to egret-only. New provisions trust egret from Task 3.
- **Sub-project 2** (local-user CRUD + UI) builds on this; not in this plan.
