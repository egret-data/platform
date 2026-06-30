# Notebook F2-P1 — Keycloak token broker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** egret persists each user's OIDC refresh token (encrypted) at login and mints short-lived, per-user **engine JWTs** from Keycloak, delivered through the notebook-runtime manifest plus a kernel-token-authed refresh endpoint.

**Architecture:** Keycloak issues access tokens whose `aud` includes the engines (added by audience mappers). egret keeps the user's refresh token in the existing `egret-secrets` vault (AES-256-GCM). An `EngineTokenMinter` seam (trait in `egret-notebook`, impl in `egret-bin`) does the `refresh_token` grant against Keycloak via an injectable `OidcTokenRefresher` (real reqwest impl + a fake for tests). The manifest carries the engine JWT + expiry; the kernel refreshes via `POST /api/ws/{ws}/notebook-runtime/token` (authed by the D1a `KernelIdentity`).

**Tech Stack:** Rust (axum, sqlx, reqwest, chrono, async-trait), `egret-secrets`, `openidconnect`, Keycloak realm JSON.

---

## File structure

- `deploy/keycloak/egret-realm.json` — add audience mappers (`egret-catalog`, `trino`, `starrocks`) + `offline_access` scope to the `egret` client.
- `migrations/0044_oidc_refresh_tokens.sql` — table keying a user to a vault secret id.
- `crates/egret-auth/src/oidc_refresh.rs` (new) — `OidcRefreshRepo` (set/get the secret id per user).
- `crates/egret-auth/src/oidc.rs` — `finish` returns `OidcLogin { claims, refresh_token }`.
- `crates/egret-auth/src/oidc_routes.rs` — persist the refresh token at login.
- `crates/egret-notebook/src/engine_token.rs` (new) — `EngineToken`, `EngineTokenMinter` trait, `BrokerError`, `FakeMinter`.
- `crates/egret-bin/src/engine_broker.rs` (new) — `OidcTokenRefresher` trait + `KeycloakRefresher` (reqwest) + `EngineTokenBroker` impl of `EngineTokenMinter`.
- `crates/egret-notebook/src/runtime.rs` — add `engine_jwt` + `engine_jwt_expires_at` to `NotebookRuntime`; `build_notebook_runtime` calls the minter.
- `crates/egret-kernel/src/routes.rs` — `POST /ws/{workspace}/notebook-runtime/token` (KernelIdentity auth) returning a fresh engine JWT.
- `crates/egret-bin/src/main.rs`, `crates/egret-server/src/lib.rs` — wire the broker into `NotebookState` + `KernelState`.

---

## Task 1: Keycloak realm — engine audiences + offline scope

**Files:** Modify `deploy/keycloak/egret-realm.json`.

- [ ] **Step 1: Add audience protocol mappers + offline scope to the `egret` client.** In the client's `protocolMappers` array (sibling of the existing `oidc-group-membership-mapper`), add three audience mappers and ensure the offline scope is requestable:

```json
{
  "name": "aud-egret-catalog",
  "protocol": "openid-connect",
  "protocolMapper": "oidc-audience-mapper",
  "config": { "included.custom.audience": "egret-catalog", "id.token.claim": "false", "access.token.claim": "true" }
},
{
  "name": "aud-trino",
  "protocol": "openid-connect",
  "protocolMapper": "oidc-audience-mapper",
  "config": { "included.custom.audience": "trino", "id.token.claim": "false", "access.token.claim": "true" }
},
{
  "name": "aud-starrocks",
  "protocol": "openid-connect",
  "protocolMapper": "oidc-audience-mapper",
  "config": { "included.custom.audience": "starrocks", "id.token.claim": "false", "access.token.claim": "true" }
}
```

Also add `"offline_access"` to the `egret` client's `defaultClientScopes` (so the refresh grant returns a long-lived refresh token).

- [ ] **Step 2: Restart Keycloak to re-import the realm.**

Run: `docker compose -f deploy/docker-compose.dev.yml up -d --force-recreate keycloak`
Expected: container healthy; realm `egret` re-imported.

- [ ] **Step 3: Smoke — verify access tokens carry the engine audiences.** Use the realm's direct-access grant (`directAccessGrantsEnabled: true`, user `tester`):

Run:
```bash
curl -s -d 'grant_type=password&client_id=egret&client_secret=egret-secret&username=tester&password=tester&scope=openid offline_access' \
  http://localhost:8081/realms/egret/protocol/openid-connect/token | \
  python3 -c 'import sys,json,base64; t=json.load(sys.stdin)["access_token"]; p=t.split(".")[1]; p+="="*(-len(p)%4); print(json.loads(base64.urlsafe_b64decode(p))["aud"])'
```
Expected: a list containing `egret-catalog`, `trino`, `starrocks`. (If `tester`'s password differs in the realm, use the one in `egret-realm.json`.)

- [ ] **Step 4: Commit**

```bash
git add deploy/keycloak/egret-realm.json
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(keycloak): engine audience mappers + offline scope on egret client

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Migration + repo for the stored refresh token (TDD)

**Files:** Create `migrations/0044_oidc_refresh_tokens.sql`, `crates/egret-auth/src/oidc_refresh.rs`, test in `crates/egret-auth/tests/oidc_refresh.rs`.

- [ ] **Step 1: Migration**

```sql
-- F2-P1: per-user OIDC refresh token, stored as a reference to an encrypted
-- vault secret (secrets.id). One row per user; replaced on each login.
CREATE TABLE oidc_refresh_tokens (
    user_id    uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    secret_id  uuid NOT NULL REFERENCES secrets(id) ON DELETE CASCADE,
    updated_at timestamptz NOT NULL DEFAULT now()
);
```

- [ ] **Step 2: Failing test** `crates/egret-auth/tests/oidc_refresh.rs`:

```rust
use egret_auth::oidc_refresh::OidcRefreshRepo;
use egret_core::Db;
use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
async fn set_then_get_roundtrips(pool: sqlx::PgPool) {
    let db = Db::from(pool);
    // a user + a secret row to reference
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, external_subject, email, name) \
         VALUES ('oidc','sub-1','u@x','U') RETURNING id")
        .fetch_one(db.pool()).await.unwrap();
    let secret_id: Uuid = sqlx::query_scalar(
        "INSERT INTO secrets (kind, nonce, ciphertext, key_id) \
         VALUES ('oidc_refresh', '\\x00', '\\x00', 'k1') RETURNING id")
        .fetch_one(db.pool()).await.unwrap();

    let repo = OidcRefreshRepo::new(db);
    assert!(repo.get(user_id).await.unwrap().is_none());
    repo.set(user_id, secret_id).await.unwrap();
    assert_eq!(repo.get(user_id).await.unwrap(), Some(secret_id));
}
```

- [ ] **Step 3: Run — fails** `cargo test -p egret-auth --test oidc_refresh` → FAIL (module `oidc_refresh` missing).

- [ ] **Step 4: Implement** `crates/egret-auth/src/oidc_refresh.rs`:

```rust
//! Per-user OIDC refresh-token reference (the token bytes live in `secrets`).
use egret_core::{Db, Error, Result};
use uuid::Uuid;

#[derive(Clone)]
pub struct OidcRefreshRepo {
    db: Db,
}

impl OidcRefreshRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Upsert the vault secret id holding `user_id`'s refresh token.
    pub async fn set(&self, user_id: Uuid, secret_id: Uuid) -> Result<()> {
        sqlx::query(
            "INSERT INTO oidc_refresh_tokens (user_id, secret_id) VALUES ($1, $2) \
             ON CONFLICT (user_id) DO UPDATE SET secret_id = $2, updated_at = now()",
        )
        .bind(user_id)
        .bind(secret_id)
        .execute(self.db.pool())
        .await
        .map_err(Error::from)?;
        Ok(())
    }

    /// The vault secret id for `user_id`, if any.
    pub async fn get(&self, user_id: Uuid) -> Result<Option<Uuid>> {
        let row: Option<Uuid> = sqlx::query_scalar(
            "SELECT secret_id FROM oidc_refresh_tokens WHERE user_id = $1",
        )
        .bind(user_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(Error::from)?;
        Ok(row)
    }
}
```

Add `pub mod oidc_refresh;` to `crates/egret-auth/src/lib.rs` (next to `pub mod oidc;`).

- [ ] **Step 5: Run — passes** `cargo test -p egret-auth --test oidc_refresh`.

- [ ] **Step 6: Commit**

```bash
git add migrations/0044_oidc_refresh_tokens.sql crates/egret-auth/src/oidc_refresh.rs crates/egret-auth/src/lib.rs crates/egret-auth/tests/oidc_refresh.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(auth): oidc_refresh_tokens table + OidcRefreshRepo

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Surface the refresh token from `OidcService::finish`

**Files:** Modify `crates/egret-auth/src/oidc.rs`.

- [ ] **Step 1: Add the return struct** near `OidcClaims` (`oidc.rs:9`):

```rust
/// Result of a completed login: normalized claims + the raw refresh token
/// (present only if the IdP issued one — e.g. `offline_access` scope).
#[derive(Debug, Clone)]
pub struct OidcLogin {
    pub claims: OidcClaims,
    pub refresh_token: Option<String>,
}
```

- [ ] **Step 2: Change `finish` to return `OidcLogin`** (`oidc.rs:187`). Keep `verify_from_response` returning `OidcClaims`; wrap it:

```rust
pub async fn finish(&self, code: &str, pkce_verifier: &str, nonce: &str) -> Result<OidcLogin> {
    let token_response = self
        .client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .map_err(oidc_err)?
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()))
        .request_async(&self.http)
        .await
        .map_err(oidc_err)?;
    let claims = self.verify_from_response(&token_response, nonce)?;
    let refresh_token = token_response
        .refresh_token()
        .map(|r| r.secret().to_string());
    Ok(OidcLogin { claims, refresh_token })
}
```

(`refresh_token()` and `RefreshToken::secret()` come from the already-imported `openidconnect::TokenResponse` trait, sibling to the `.id_token()` call.)

- [ ] **Step 3: Build to find the broken caller** `cargo build -p egret-auth` → expect an error in `oidc_routes.rs` (callsite returns `OidcClaims`). That callsite is fixed in Task 4.

- [ ] **Step 4: Commit (compiles after Task 4; commit together).** Defer — staged with Task 4.

---

## Task 4: Persist the refresh token at login

**Files:** Modify `crates/egret-auth/src/oidc_routes.rs`; `crates/egret-bin/src/main.rs` (wire deps).

- [ ] **Step 1: Extend `OidcRoutesState`** (`oidc_routes.rs:22`) with the vault + repo:

```rust
    pub secrets: egret_secrets::SecretStore,
    pub refresh_repo: crate::oidc_refresh::OidcRefreshRepo,
```

- [ ] **Step 2: In `oidc_callback`** (`oidc_routes.rs:85`), update the `finish` callsite and persist after session creation (`oidc_routes.rs:112-146`):

```rust
    let login = st.oidc.finish(&q.code, &state.pkce_verifier, &state.nonce).await
        .map_err(/* keep existing error mapping */)?;
    let user_id = provision_and_sync(&st.users, &st.groups, &login.claims).await
        .map_err(/* existing */)?;
    let issued = st.sessions.create(user_id, "", "").await
        .map_err(/* existing */)?;

    // Persist the refresh token (encrypted) for the engine-token broker.
    if let Some(rt) = login.refresh_token.as_deref() {
        let secret_id = st.secrets.put("oidc_refresh", rt).await
            .map_err(|_| /* existing 500 mapping */ )?;
        st.refresh_repo.set(user_id, secret_id).await
            .map_err(|_| /* existing 500 mapping */ )?;
    }
```

(Use `claims` → `login.claims` everywhere else in the handler.)

- [ ] **Step 3: Wire deps where `OidcRoutesState` is built** in `crates/egret-bin/src/main.rs:463-477`: pass the already-constructed `SecretStore` (`main.rs:369-380`) and `OidcRefreshRepo::new(db.clone())` into the struct.

- [ ] **Step 4: Build + existing tests** `cargo build -p egret-auth -p egret-bin && cargo test -p egret-auth` → PASS.

- [ ] **Step 5: Commit (Task 3 + 4 together)**

```bash
git add crates/egret-auth/src/oidc.rs crates/egret-auth/src/oidc_routes.rs crates/egret-bin/src/main.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(auth): capture + persist OIDC refresh token at login

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `EngineTokenMinter` seam + Keycloak broker (TDD)

**Files:** Create `crates/egret-notebook/src/engine_token.rs` (trait + fake) and `crates/egret-bin/src/engine_broker.rs` (impl); test `crates/egret-bin/tests/engine_broker.rs`.

- [ ] **Step 1: Define the seam** `crates/egret-notebook/src/engine_token.rs`:

```rust
//! The engine-token seam. Concrete minting (Keycloak refresh grant) is injected
//! from egret-bin, so this crate stays free of an HTTP/IdP dependency.
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct EngineToken {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("no stored refresh token for user")]
    NoRefreshToken,
    #[error("idp refresh failed: {0}")]
    Idp(String),
    #[error(transparent)]
    Other(#[from] egret_core::Error),
}

#[async_trait::async_trait]
pub trait EngineTokenMinter: Send + Sync {
    /// Mint a fresh, short-lived engine access token for `user_id`.
    async fn mint(&self, user_id: Uuid) -> Result<EngineToken, BrokerError>;
}

/// Test double: returns a canned token and records the last user.
#[derive(Default)]
pub struct FakeMinter {
    pub last: std::sync::Mutex<Option<Uuid>>,
}

#[async_trait::async_trait]
impl EngineTokenMinter for FakeMinter {
    async fn mint(&self, user_id: Uuid) -> Result<EngineToken, BrokerError> {
        *self.last.lock().unwrap() = Some(user_id);
        Ok(EngineToken {
            access_token: "fake.jwt.token".into(),
            expires_at: DateTime::<Utc>::from_timestamp(4102444800, 0).unwrap(), // 2100-01-01
        })
    }
}
```

Add `pub mod engine_token;` to `crates/egret-notebook/src/lib.rs`. Add deps to `crates/egret-notebook/Cargo.toml` if missing: `async-trait`, `chrono` (with `serde`), `thiserror`.

- [ ] **Step 2: Failing test** `crates/egret-bin/tests/engine_broker.rs` (fakes the Keycloak call via the `OidcTokenRefresher` seam + a real `SecretStore` on a scratch DB):

```rust
use egret_bin::engine_broker::{EngineTokenBroker, OidcTokenRefresher, RefreshedToken};
use egret_notebook::engine_token::EngineTokenMinter;
use egret_auth::oidc_refresh::OidcRefreshRepo;
use egret_core::Db;
use egret_secrets::{Crypto, SecretStore};
use uuid::Uuid;

struct FakeRefresher;
#[async_trait::async_trait]
impl OidcTokenRefresher for FakeRefresher {
    async fn refresh(&self, refresh_token: &str) -> Result<RefreshedToken, String> {
        assert_eq!(refresh_token, "stored-rt");
        Ok(RefreshedToken { access_token: "new.access".into(), refresh_token: Some("rotated-rt".into()), expires_in_secs: 300 })
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn mint_uses_stored_token_and_rotates(pool: sqlx::PgPool) {
    let db = Db::from(pool);
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, external_subject, email, name) \
         VALUES ('oidc','s','u@x','U') RETURNING id").fetch_one(db.pool()).await.unwrap();

    let crypto = Crypto::new(&[7u8; 32]).unwrap();
    let secrets = SecretStore::new(db.clone(), Some(crypto));
    let repo = OidcRefreshRepo::new(db.clone());
    let sid = secrets.put("oidc_refresh", "stored-rt").await.unwrap();
    repo.set(user_id, sid).await.unwrap();

    let broker = EngineTokenBroker::new(secrets.clone(), repo.clone(), FakeRefresher);
    let tok = broker.mint(user_id).await.unwrap();
    assert_eq!(tok.access_token, "new.access");
    // rotated refresh token persisted back into the same secret row
    assert_eq!(secrets.get(sid).await.unwrap(), "rotated-rt");
}
```

- [ ] **Step 3: Run — fails** `cargo test -p egret-bin --test engine_broker` → FAIL (module missing).

- [ ] **Step 4: Implement** `crates/egret-bin/src/engine_broker.rs`:

```rust
//! Keycloak-backed `EngineTokenMinter`: refresh-token grant → short-lived
//! engine access token; rotates the stored refresh token when the IdP returns one.
use async_trait::async_trait;
use chrono::{Duration, Utc};
use egret_auth::oidc_refresh::OidcRefreshRepo;
use egret_notebook::engine_token::{BrokerError, EngineToken, EngineTokenMinter};
use egret_secrets::SecretStore;
use uuid::Uuid;

/// The IdP refresh-grant call (seam so the broker is unit-testable without HTTP).
#[async_trait]
pub trait OidcTokenRefresher: Send + Sync {
    async fn refresh(&self, refresh_token: &str) -> Result<RefreshedToken, String>;
}

pub struct RefreshedToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in_secs: i64,
}

/// Real impl: POST the Keycloak token endpoint with grant_type=refresh_token.
pub struct KeycloakRefresher {
    pub http: reqwest::Client,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
}

#[async_trait]
impl OidcTokenRefresher for KeycloakRefresher {
    async fn refresh(&self, refresh_token: &str) -> Result<RefreshedToken, String> {
        #[derive(serde::Deserialize)]
        struct Resp { access_token: String, refresh_token: Option<String>, expires_in: i64 }
        let resp = self.http.post(&self.token_url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &self.client_id),
                ("client_secret", &self.client_secret),
                ("refresh_token", refresh_token),
                ("scope", "openid offline_access"),
            ])
            .send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("token endpoint {}", resp.status()));
        }
        let r: Resp = resp.json().await.map_err(|e| e.to_string())?;
        Ok(RefreshedToken { access_token: r.access_token, refresh_token: r.refresh_token, expires_in_secs: r.expires_in })
    }
}

pub struct EngineTokenBroker<R: OidcTokenRefresher> {
    secrets: SecretStore,
    repo: OidcRefreshRepo,
    refresher: R,
}

impl<R: OidcTokenRefresher> EngineTokenBroker<R> {
    pub fn new(secrets: SecretStore, repo: OidcRefreshRepo, refresher: R) -> Self {
        Self { secrets, repo, refresher }
    }
}

#[async_trait]
impl<R: OidcTokenRefresher> EngineTokenMinter for EngineTokenBroker<R> {
    async fn mint(&self, user_id: Uuid) -> Result<EngineToken, BrokerError> {
        let secret_id = self.repo.get(user_id).await?.ok_or(BrokerError::NoRefreshToken)?;
        let refresh_token = self.secrets.get(secret_id).await
            .map_err(|e| BrokerError::Idp(e.to_string()))?;
        let refreshed = self.refresher.refresh(&refresh_token).await.map_err(BrokerError::Idp)?;
        // Rotate the stored refresh token if the IdP issued a new one.
        if let Some(new_rt) = refreshed.refresh_token {
            if new_rt != refresh_token {
                self.secrets.update(secret_id, &new_rt).await
                    .map_err(|e| BrokerError::Idp(e.to_string()))?;
            }
        }
        Ok(EngineToken {
            access_token: refreshed.access_token,
            expires_at: Utc::now() + Duration::seconds(refreshed.expires_in_secs),
        })
    }
}
```

Ensure `crates/egret-bin` exposes a lib target (`engine_broker` must be importable from `tests/`). If egret-bin is binary-only, add `src/lib.rs` with `pub mod engine_broker;` and have `main.rs` use it; add `egret-notebook`, `egret-auth`, `egret-secrets`, `async-trait`, `chrono`, `reqwest`, `serde` to `egret-bin`'s deps (most already present).

- [ ] **Step 5: Run — passes** `cargo test -p egret-bin --test engine_broker`.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-notebook/src/engine_token.rs crates/egret-notebook/src/lib.rs crates/egret-notebook/Cargo.toml crates/egret-bin/src/engine_broker.rs crates/egret-bin/src/lib.rs crates/egret-bin/Cargo.toml crates/egret-bin/tests/engine_broker.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(notebook): EngineTokenMinter seam + Keycloak refresh-grant broker

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Add the engine JWT to the runtime manifest (TDD)

**Files:** Modify `crates/egret-notebook/src/runtime.rs` (struct + builder + `NotebookState`).

- [ ] **Step 1: Extend `NotebookRuntime`** (`runtime.rs:30`):

```rust
    pub engine_jwt: String,
    pub engine_jwt_expires_at: chrono::DateTime<chrono::Utc>,
```

- [ ] **Step 2: Add the minter to `NotebookState`** (`crates/egret-notebook/src/routes.rs:47`) as `pub engine_minter: std::sync::Arc<dyn crate::engine_token::EngineTokenMinter>,`.

- [ ] **Step 3: Failing test** `crates/egret-notebook/tests/runtime_engine_jwt.rs` — assert the builder calls the minter and fills the manifest. (Use the existing notebook test harness pattern; inject `Arc::new(FakeMinter::default())`.) Minimal shape:

```rust
// Arrange a NotebookState with FakeMinter; provision a workspace + user (reuse
// the helpers in crates/egret-notebook/tests for state setup), then:
let m = egret_notebook::runtime::build_notebook_runtime(&state, ws, user_id).await.unwrap();
assert_eq!(m.engine_jwt, "fake.jwt.token");
assert!(m.engine_jwt_expires_at > chrono::Utc::now());
```

- [ ] **Step 4: Run — fails** `cargo test -p egret-notebook --test runtime_engine_jwt` → FAIL (field missing / not minted).

- [ ] **Step 5: Implement in `build_notebook_runtime`** (`runtime.rs:49`, where the manifest is assembled at `runtime.rs:108-118`):

```rust
    let engine_tok = st.engine_minter.mint(user_id.into()).await
        .map_err(|e| NotebookApiError::from(egret_core::Error::Internal(e.to_string())))?;
    // ... in the NotebookRuntime { .. } literal:
    engine_jwt: engine_tok.access_token,
    engine_jwt_expires_at: engine_tok.expires_at,
```

(Map `BrokerError` to the crate's existing `NotebookApiError` internal variant; if `user_id.into()` doesn't yield `Uuid`, use the existing `UserId`→`Uuid` accessor used elsewhere in this file.)

- [ ] **Step 6: Run — passes** `cargo test -p egret-notebook --test runtime_engine_jwt`.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-notebook/src/runtime.rs crates/egret-notebook/src/routes.rs crates/egret-notebook/tests/runtime_engine_jwt.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(notebook): manifest carries per-user engine JWT + expiry

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Kernel-token-authed refresh endpoint (TDD)

**Files:** Modify `crates/egret-kernel/src/routes.rs` (route + handler + `KernelState`).

- [ ] **Step 1: Add the minter to `KernelState`** (`crates/egret-kernel/src/routes.rs`, the state struct near `:52`) as `pub engine_minter: std::sync::Arc<dyn egret_notebook::engine_token::EngineTokenMinter>,` (add `egret-notebook` to `egret-kernel`'s deps; the seam trait lives there).

- [ ] **Step 2: Failing test** in `crates/egret-kernel/tests/http.rs` (extend the existing harness that already exercises `/kernel/query` with a kernel token):

```rust
#[sqlx::test(migrations = "../../migrations")]
async fn notebook_runtime_token_returns_engine_jwt(pool: sqlx::PgPool) {
    // Build app with Arc::new(FakeMinter::default()) as engine_minter and a kernel
    // row with a known token for (user, ws) — reuse the helper used by the
    // existing /kernel/query test.
    let (app, token, _ws) = setup_kernel_app(pool).await;
    let resp = app
        .oneshot(/* POST /api/ws/{ws}/notebook-runtime/token, Authorization: Bearer {token} */)
        .await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = /* read json */;
    assert_eq!(body["access_token"], "fake.jwt.token");

    // Without the bearer → 401.
    let unauth = app2.oneshot(/* same POST, no auth header */).await.unwrap();
    assert_eq!(unauth.status(), 401);
}
```

- [ ] **Step 3: Run — fails** `cargo test -p egret-kernel --test http notebook_runtime_token` → FAIL (route missing → 404).

- [ ] **Step 4: Implement handler + route.** Handler (uses the existing `KernelIdentity` extractor at `routes.rs:356`):

```rust
async fn notebook_runtime_token(
    id: KernelIdentity,
    axum::extract::State(st): axum::extract::State<KernelState>,
) -> Result<axum::Json<egret_notebook::engine_token::EngineToken>, KernelApiError> {
    let tok = st.engine_minter.mint(id.user_id).await
        .map_err(|e| KernelApiError(egret_core::Error::Internal(e.to_string())))?;
    Ok(axum::Json(tok))
}
```

Register in `kernel_router` (`routes.rs:80-95`, near `:94`):

```rust
.route("/ws/{workspace}/notebook-runtime/token", post(notebook_runtime_token))
```

(`EngineToken` already derives `Serialize`. The `{workspace}` segment is accepted for symmetry but the token is user-scoped from `KernelIdentity`; do NOT trust it for identity.)

- [ ] **Step 5: Run — passes** `cargo test -p egret-kernel --test http notebook_runtime_token`.

- [ ] **Step 6: Wire the real broker into both states in egret-bin** (`crates/egret-bin/src/main.rs` + `crates/egret-server/src/lib.rs:174-188`): construct one `Arc::new(EngineTokenBroker::new(secret_store, OidcRefreshRepo::new(db), KeycloakRefresher { http: oidc.http_client(), token_url: format!("{}/protocol/openid-connect/token", issuer), client_id, client_secret }))` and pass the same `Arc` into `NotebookState.engine_minter` and `KernelState.engine_minter`. (Expose the reqwest client from `OidcService` via a small `pub fn http_client(&self) -> reqwest::Client` accessor, or build a dedicated client; `token_url` derives from `config.oidc.issuer_url`.)

- [ ] **Step 7: Build the workspace** `cargo build --workspace` → PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-kernel/src/routes.rs crates/egret-kernel/Cargo.toml crates/egret-kernel/tests/http.rs crates/egret-bin/src/main.rs crates/egret-server/src/lib.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): POST notebook-runtime/token refresh endpoint + wire broker

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Full local CI

- [ ] **Step 1: Format** `cargo fmt --all` then `cargo fmt --all --check` → clean.
- [ ] **Step 2: Clippy** `cargo clippy --workspace --all-targets -- -D warnings` → 0 issues.
- [ ] **Step 3: Tests** (start postgres first: `docker compose -f deploy/docker-compose.dev.yml up -d postgres`; `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret`) `cargo test --workspace` → green (MinIO-gated `egret-catalog-engine/browse_api` needs MinIO up — see [[egret-run-full-ci-local-before-merge]]).
- [ ] **Step 4: Manual broker smoke (real Keycloak):** with the dev stack up and `[oidc]` configured against Keycloak in `egret.toml`, log in through the browser, then `POST /api/ws/{ws}/notebook-runtime/token` with a valid kernel token and assert a JWT whose `aud` contains the engines (decode like Task 1 Step 3).

---

## Self-review

- **Spec coverage (F2-P1 slice of §4.1/§4.2/§4.6/§11):** Keycloak audiences (T1) ✔; refresh-token persistence (T2–T4) ✔; broker via refresh grant (T5) ✔; manifest engine JWT (T6) ✔; kernel-token refresh endpoint (T7) ✔. (Catalog JWT authn = F2-P2; Trino/StarRocks = P3/P4 — out of this plan.)
- **Placeholder scan:** the only deliberately-abbreviated spots are test-harness setup in T6/T7 (they say "reuse the existing notebook/kernel test helpers" because those helpers already exist in-repo) and the existing error-mapping closures in T4 (match the handler's current mapping). No logic placeholders.
- **Type consistency:** `EngineToken{access_token, expires_at}`, `EngineTokenMinter::mint(user_id)`, `OidcTokenRefresher::refresh(&str)->RefreshedToken{access_token, refresh_token, expires_in_secs}`, `OidcLogin{claims, refresh_token}`, `OidcRefreshRepo::{set,get}` — used identically across tasks.
- **Decision applied:** "store the user's refresh token" (delegated flow) — T2–T4 persist; T5 mints via refresh grant; no token-exchange/service-account needed (audiences via realm mappers, T1).
