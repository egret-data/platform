# Notebook F2-P2 — egret-catalog accepts OIDC JWT Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** The egret-catalog (embedded lakekeeper Iceberg REST) accepts a Keycloak **access-token JWT** for authentication — mapping the user to a default `read/write` grant on namespace `notebook_{username}` in the request's workspace warehouse — while keeping the existing opaque `catalog_tokens` path for service/engine tokens (hybrid).

**Architecture:** `front_auth` gains a JWT branch. A Keycloak access token is verified by `lakekeeper::limes::jwks::JWKSWebAuthenticator` (already a transitive dep — validates signature/iss/aud/exp + fetches & caches JWKS). The verified `sub` resolves to an egret user; the request's path prefix (a lakekeeper warehouse UUID) reverse-resolves to `(workspace_id, warehouse_row_id)`; workspace membership is checked; and a `TokenAuth` carrying one grant `{notebook_{username}: rw}` is produced and fed into the EXISTING downstream pipeline (ownership prefix check → `token_request_metadata` → per-grant `EgretAuthorizer`). Opaque tokens still work; selection is by token shape (a JWT has three `.`-separated segments).

**Tech Stack:** Rust, axum middleware, `lakekeeper::limes`, sqlx, existing `CatalogStore`/`UserRepo`/`Authorizer`.

**Scope note (MVP):** P2 grants the user `rw` on their OWN `notebook_{username}` namespace only. Group-based **read** on shared/governed namespaces is DEFERRED (needs a "shared namespace" definition + a group-names-for-user query) — call it P2-followup. Full real-Keycloak end-to-end (JWT → catalog → table op against MinIO) is **P5**; P2's automated tests cover the verify→map→grant logic without MinIO.

---

## File structure
- `crates/egret-config/src/lib.rs` — add `catalog_audience: String` to `OidcConfig` (default `"egret-catalog"`).
- `crates/egret-catalog/src/service/warehouse.rs` — add `warehouse_by_lakekeeper_id` (reverse resolver).
- `crates/egret-catalog-engine/src/jwt_auth.rs` (new) — `AccessTokenVerifier` trait + `LimesVerifier` (real) + `CatalogJwtAuth` (verify→map→`TokenAuth`) + a fake verifier for tests.
- `crates/egret-catalog-engine/src/front_auth.rs` — JWT-vs-opaque branch; `FrontAuthState` gains an optional `Arc<CatalogJwtAuth>`.
- `crates/egret-catalog-engine/src/mount.rs` — `mount_catalog_engine` gains the JWT-auth dependency; build it from `OidcConfig` + `Db`.
- `crates/egret-bin/src/main.rs` — pass `OidcConfig` + `Db` into `mount_catalog_engine`.

---

## Task 1: Config — catalog audience (TDD-lite)

**Files:** `crates/egret-config/src/lib.rs`.

- [ ] **Step 1: Add the field** to `OidcConfig` (struct around lib.rs:19-28):
```rust
    /// Audience the catalog requires in an engine JWT's `aud` claim.
    pub catalog_audience: String,
```
- [ ] **Step 2: Default** — in the `Default for OidcConfig` impl (around lib.rs:186), add `catalog_audience: "egret-catalog".into(),`.
- [ ] **Step 3: Build** `cargo build -p egret-config` → succeeds. If a test asserts the full default struct, update it.
- [ ] **Step 4: Commit**
```bash
git add crates/egret-config/src/lib.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(config): OidcConfig.catalog_audience (default egret-catalog)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Reverse resolver — lakekeeper UUID → (workspace_id, warehouse row id) (TDD)

**Files:** `crates/egret-catalog/src/service/warehouse.rs`; test alongside the crate's existing warehouse tests (find them first; if there's a `tests/` integration test for warehouse, add there, else a `#[cfg(test)]` unit test in the module).

The forward query (warehouse.rs:201) is `SELECT warehouse_id FROM workspace_warehouses WHERE id=$1 AND workspace_id=$2`. The table has columns `id` (egret row), `workspace_id`, `warehouse_id` (lakekeeper UUID). Invert it.

- [ ] **Step 1: Failing test** (sqlx::test; insert a workspace + a workspace_warehouses row directly — NO MinIO needed). Read a neighboring warehouse test for the exact insert columns of `workspace_warehouses` (it has more NOT NULL columns — copy them). Skeleton:
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn warehouse_by_lakekeeper_id_roundtrips(pool: sqlx::PgPool) {
    let store = CatalogStore::new(Db { pool });
    // seed a workspace + workspace_warehouses row (copy required columns from an existing test);
    // let row_id, ws_id, lakekeeper_uuid be the inserted values.
    let got = store.warehouse_by_lakekeeper_id(lakekeeper_uuid).await.unwrap();
    assert_eq!(got, Some((ws_id, row_id)));
    assert_eq!(store.warehouse_by_lakekeeper_id(uuid::Uuid::new_v4()).await.unwrap(), None);
}
```
- [ ] **Step 2: Run — fails** (method missing).
- [ ] **Step 3: Implement** in `warehouse.rs` (next to `lakekeeper_id`):
```rust
/// Reverse of `lakekeeper_id`: map a lakekeeper warehouse UUID back to the
/// egret `(workspace_id, warehouse_row_id)`. Used by JWT catalog auth, where the
/// request path prefix is the lakekeeper UUID and the token has no workspace.
pub async fn warehouse_by_lakekeeper_id(
    &self,
    lakekeeper_uuid: Uuid,
) -> Result<Option<(Uuid, Uuid)>, CatalogError> {
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT workspace_id, id FROM workspace_warehouses WHERE warehouse_id = $1",
    )
    .bind(lakekeeper_uuid)
    .fetch_optional(self.pool())
    .await
    .map_err(CatalogError::Db)?;
    Ok(row)
}
```
- [ ] **Step 4: Run — passes.**
- [ ] **Step 5: Commit**
```bash
git add crates/egret-catalog/src/service/warehouse.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(catalog): warehouse_by_lakekeeper_id reverse resolver

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: JWT verify → user → grant mapping (TDD, no MinIO)

**Files:** Create `crates/egret-catalog-engine/src/jwt_auth.rs`; add `pub mod jwt_auth;` to `crates/egret-catalog-engine/src/lib.rs`; test in `crates/egret-catalog-engine/tests/jwt_auth.rs`.

Design: a `CatalogJwtAuth` that depends on an `AccessTokenVerifier` trait (so tests inject a fake — no live IdP) plus the shared `Db`. It produces an `egret_catalog::service::TokenAuth` identical in shape to what opaque tokens produce, so the rest of `front_auth` is unchanged.

- [ ] **Step 1: Skeleton + trait + real impl**
```rust
//! OIDC-JWT authentication for the catalog: verify a Keycloak access token,
//! map the subject to an egret user, and produce a default `notebook_{username}`
//! read/write grant. Verification is behind a trait so the mapping is unit-testable.
use std::sync::Arc;
use uuid::Uuid;
use egret_catalog::service::{CatalogStore, Grant, TokenAuth};
use egret_metastore::Db;

/// Verifies a Keycloak access-token JWT and returns its subject (`sub`).
#[async_trait::async_trait]
pub trait AccessTokenVerifier: Send + Sync {
    async fn verify(&self, token: &str) -> Result<String, JwtAuthError>;
}

#[derive(Debug)]
pub enum JwtAuthError {
    Unauthenticated, // bad/expired token, unknown user
    Forbidden,       // not a member of the target workspace / unknown warehouse
}

pub struct CatalogJwtAuth {
    verifier: Arc<dyn AccessTokenVerifier>,
    store: Arc<CatalogStore>,
    db: Db,
}

impl CatalogJwtAuth {
    pub fn new(verifier: Arc<dyn AccessTokenVerifier>, store: Arc<CatalogStore>, db: Db) -> Self {
        Self { verifier, store, db }
    }

    /// Authenticate a bearer JWT for a request targeting `prefix_lakekeeper_uuid`
    /// (the `{prefix}` path segment). Returns the same `TokenAuth` shape opaque
    /// tokens produce, with one grant: rw on `notebook_{username}`.
    pub async fn authenticate(
        &self,
        token: &str,
        prefix_lakekeeper_uuid: Uuid,
    ) -> Result<TokenAuth, JwtAuthError> {
        // 1. verify JWT (sig/iss/aud/exp) -> subject
        let subject = self.verifier.verify(token).await?;
        // 2. subject -> egret user (provider='oidc', external_subject=subject)
        let user = egret_auth::users::UserRepo::new(self.db.clone())
            .find_by_external_subject(&subject)
            .await
            .map_err(|_| JwtAuthError::Unauthenticated)?
            .ok_or(JwtAuthError::Unauthenticated)?;
        // 3. prefix (lakekeeper uuid) -> (workspace_id, warehouse_row_id)
        let (workspace_id, warehouse_row_id) = self
            .store
            .warehouse_by_lakekeeper_id(prefix_lakekeeper_uuid)
            .await
            .map_err(|_| JwtAuthError::Forbidden)?
            .ok_or(JwtAuthError::Forbidden)?;
        // 4. membership: user must have >=1 role in that workspace
        let roles = egret_workspace::Authorizer::new(self.db.clone())
            .workspace_roles_for(user.id, workspace_id)
            .await
            .map_err(|_| JwtAuthError::Forbidden)?;
        if roles.is_empty() {
            return Err(JwtAuthError::Forbidden);
        }
        // 5. default grant: rw on notebook_{username}
        let ns = format!("notebook_{}", user.username);
        let perms = vec!["catalog:read".to_string(), "catalog:write".to_string()];
        Ok(TokenAuth {
            workspace_id,
            warehouse_id: warehouse_row_id,
            grants: vec![Grant { namespace: ns, table: None, permissions: perms.clone() }],
            permissions: perms,
        })
    }
}
```
VERIFY exact signatures while implementing and adapt: `UserRepo::new`/`find_by_external_subject` (egret-auth — used in oidc.rs; confirm it returns a struct with `id: UserId` and `username: String`; if `username` is `Option`/needs `ensure_username`, call the right accessor), `egret_workspace::Authorizer::new(...).workspace_roles_for(user_id, ws_id)` (rbac.rs:64 — confirm constructor + the `UserId` vs `Uuid` type; `user.id` may be `UserId(Uuid)` → pass as the fn expects), `egret_catalog::service::{TokenAuth, Grant, CatalogStore}` field/visibility (Grant fields are `namespace`, `table: Option<String>`, `permissions: Vec<String>`; ensure they're `pub` and re-exported — if `Grant`/`TokenAuth` aren't public from `egret_catalog::service`, add the re-export). Add deps to `crates/egret-catalog-engine/Cargo.toml` if missing: `egret-auth`, `egret-workspace`, `egret-catalog`, `egret-metastore`, `async-trait`, `uuid` (most already present — egret-catalog-engine already uses egret-catalog + lakekeeper). Check there's NO dependency cycle (egret-auth/egret-workspace must not depend on egret-catalog-engine).

- [ ] **Step 2: Real verifier `LimesVerifier`** (in the same file) wrapping `lakekeeper::limes::jwks::JWKSWebAuthenticator`:
```rust
pub struct LimesVerifier {
    inner: lakekeeper::limes::jwks::JWKSWebAuthenticator,
}
impl LimesVerifier {
    /// `issuer_url` e.g. `http://keycloak:8081/realms/egret`; `audience` = config.oidc.catalog_audience.
    pub async fn new(issuer_url: &str, audience: String) -> anyhow::Result<Self> {
        let inner = lakekeeper::limes::jwks::JWKSWebAuthenticator::new(issuer_url, None)
            .await?
            .set_accepted_audiences(vec![audience]);
        Ok(Self { inner })
    }
}
#[async_trait::async_trait]
impl AccessTokenVerifier for LimesVerifier {
    async fn verify(&self, token: &str) -> Result<String, JwtAuthError> {
        use lakekeeper::limes::Authenticator;
        let auth = self.inner.authenticate(token).await.map_err(|_| JwtAuthError::Unauthenticated)?;
        // `Authentication` carries the subject; read it via the limes API.
        Ok(auth.subject().to_string())
    }
}
```
VERIFY the exact limes API at impl time (anchors: `lakekeeper::limes::jwks::JWKSWebAuthenticator::{new,set_accepted_audiences}`, the `Authenticator` trait's `authenticate`, and how `Authentication` exposes the subject — it may be `.subject()`, `.actor()`, or a field; read `lakekeeper`'s re-exported `limes` source / `front_auth.rs:76` already imports `lakekeeper::limes::Authentication`). If `JWKSWebAuthenticator::new` returns a non-anyhow error, map it. Builder methods are chainable per the anchors.

- [ ] **Step 3: Failing test** `crates/egret-catalog-engine/tests/jwt_auth.rs` (fake verifier + seeded DB, NO MinIO):
```rust
use std::sync::Arc;
use egret_catalog_engine::jwt_auth::{AccessTokenVerifier, CatalogJwtAuth, JwtAuthError};
use egret_catalog::service::CatalogStore;
use egret_metastore::Db;
use uuid::Uuid;

struct FakeVerifier(&'static str); // returns this subject
#[async_trait::async_trait]
impl AccessTokenVerifier for FakeVerifier {
    async fn verify(&self, _t: &str) -> Result<String, JwtAuthError> { Ok(self.0.to_string()) }
}

#[sqlx::test(migrations = "../../migrations")]
async fn jwt_member_gets_notebook_rw_grant(pool: sqlx::PgPool) {
    let db = Db { pool };
    // seed: user (provider='oidc', external_subject='sub-1', username derived),
    //   workspace, workspace_warehouses row (lakekeeper_uuid), and a workspace_members
    //   row granting the user a role. Copy the exact insert columns from an existing
    //   egret-workspace / catalog test. Capture lakekeeper_uuid + expected ws_id/row_id + username.
    let store = Arc::new(CatalogStore::new(db.clone()));
    let auth = CatalogJwtAuth::new(Arc::new(FakeVerifier("sub-1")), store, db);
    let ta = auth.authenticate("any.jwt.here", lakekeeper_uuid).await.unwrap();
    assert_eq!(ta.workspace_id, ws_id);
    assert_eq!(ta.grants.len(), 1);
    assert_eq!(ta.grants[0].namespace, format!("notebook_{username}"));
    assert!(ta.grants[0].permissions.iter().any(|p| p=="catalog:write"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn jwt_non_member_is_forbidden(pool: sqlx::PgPool) {
    // same seed but DO NOT add the workspace_members row -> expect Err(Forbidden)
    // ... assert matches!(err, JwtAuthError::Forbidden)
}

#[sqlx::test(migrations = "../../migrations")]
async fn jwt_unknown_subject_is_unauthenticated(pool: sqlx::PgPool) {
    // FakeVerifier returns a subject with no matching user -> Err(Unauthenticated)
}
```
Use the existing test helpers for seeding users/workspaces/members (read `crates/egret-workspace/tests` or `crates/egret-catalog-engine/tests/front_auth.rs` `seed_workspace_with_warehouse` for the column lists — but DO NOT call the MinIO-backed `provision_workspace_warehouse`; insert the `workspace_warehouses` row directly with a random `warehouse_id` UUID, since the reverse resolver only reads that table).

- [ ] **Step 4: Run — fails** then **Step 5: implement** until **Step 6: passes** `cargo test -p egret-catalog-engine --test jwt_auth` (postgres up + DATABASE_URL set).
- [ ] **Step 7: Commit**
```bash
git add crates/egret-catalog-engine/src/jwt_auth.rs crates/egret-catalog-engine/src/lib.rs crates/egret-catalog-engine/Cargo.toml crates/egret-catalog-engine/tests/jwt_auth.rs crates/egret-catalog/src/service/*.rs Cargo.lock
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(catalog): JWT auth mapping -> notebook_{user} rw grant (verify behind a seam)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: front_auth JWT branch + wiring

**Files:** `crates/egret-catalog-engine/src/front_auth.rs`, `mount.rs`, `crates/egret-bin/src/main.rs`.

- [ ] **Step 1: Extend `FrontAuthState`** (front_auth.rs:82) with an optional JWT authenticator:
```rust
    pub jwt: Option<std::sync::Arc<crate::jwt_auth::CatalogJwtAuth>>,
```
Update `FrontAuthState::new` to take it (or add a `with_jwt`); keep existing callers compiling.

- [ ] **Step 2: Branch in `front_auth`** (the AUTHN block, front_auth.rs:208-225). After extracting `token`, before `authenticate_token`, detect a JWT and route:
```rust
    let looks_like_jwt = token.split('.').count() == 3;
    let auth = if looks_like_jwt {
        match &state.jwt {
            Some(j) => {
                // the prefix segment is the lakekeeper warehouse UUID
                let path = req.uri().path().to_owned();
                let prefix = first_path_segment(&path).ok_or(StatusCode::UNAUTHORIZED)?;
                if prefix == "config" {
                    // /config has no warehouse prefix; JWT can't scope it -> 401 here (opaque tokens handle /config)
                    return Err(StatusCode::UNAUTHORIZED);
                }
                let wh = Uuid::parse_str(prefix).map_err(|_| StatusCode::UNAUTHORIZED)?;
                j.authenticate(token, wh).await.map_err(|e| match e {
                    crate::jwt_auth::JwtAuthError::Unauthenticated => StatusCode::UNAUTHORIZED,
                    crate::jwt_auth::JwtAuthError::Forbidden => StatusCode::FORBIDDEN,
                })?
            }
            None => return Err(StatusCode::UNAUTHORIZED), // JWT presented but OIDC not configured
        }
    } else {
        state.store.authenticate_token(token).await.map_err(|_| StatusCode::UNAUTHORIZED)?
    };
```
Everything after this (the ownership prefix check at :229-248, `required_permission` gate, `token_request_metadata`, principal injection) stays UNCHANGED — `auth` is a `TokenAuth` either way, and for the JWT path `auth.warehouse_id` was resolved FROM the prefix so the ownership check passes. (Note: the JWT path already computed `prefix`/`wh`; the existing ownership block recomputes from the path — that's fine, it'll match. Don't remove the existing block.)

- [ ] **Step 3: Wire `mount_catalog_engine`** (mount.rs:244). Add params so the JWT auth can be built: the function should accept the `OidcConfig` (or just `Option<Arc<CatalogJwtAuth>>`) and pass it into `FrontAuthState`. Prefer passing a pre-built `Option<Arc<CatalogJwtAuth>>` so `mount.rs` stays IdP-agnostic and the async `LimesVerifier::new` happens in `main.rs`.

- [ ] **Step 4: Build the verifier + auth in `main.rs`** where `mount_catalog_engine` is called. When `config.oidc.enabled`:
```rust
let catalog_jwt = if config.oidc.enabled {
    let verifier = std::sync::Arc::new(
        egret_catalog_engine::jwt_auth::LimesVerifier::new(
            &config.oidc.issuer_url, config.oidc.catalog_audience.clone()).await?);
    Some(std::sync::Arc::new(egret_catalog_engine::jwt_auth::CatalogJwtAuth::new(
        verifier, store_arc.clone(), db.clone())))
} else { None };
```
Pass `catalog_jwt` into `mount_catalog_engine`. (`store_arc` is the `Arc<CatalogStore>` already built for the catalog; `db` is the shared pool — both are in scope at the catalog wiring site. Confirm names.)

- [ ] **Step 5: Build** `cargo build --workspace` → success. Fix the CE-2 integration test that calls `mount_catalog_engine` (`crates/egret-catalog-engine/tests/front_auth.rs`) to pass `None` for the new param (opaque-token tests don't use JWT).

- [ ] **Step 6: fmt + clippy** `cargo fmt --all` ; `cargo clippy -p egret-catalog-engine -p egret-catalog -p egret-bin --all-targets -- -D warnings`.

- [ ] **Step 7: Commit**
```bash
git add crates/egret-catalog-engine/src/front_auth.rs crates/egret-catalog-engine/src/mount.rs crates/egret-bin/src/main.rs crates/egret-catalog-engine/tests/front_auth.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(catalog): front_auth accepts OIDC JWT (hybrid with opaque tokens)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Full local CI + smoke note

- [ ] **Step 1:** `cargo fmt --all --check` → clean.
- [ ] **Step 2:** `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- [ ] **Step 3:** `cargo test --workspace` (postgres + minio up, `warehouse` bucket created, `DATABASE_URL` set). Green except the known infra-flaky `egret-catalog-engine` MinIO-S3 tests + the pre-existing `egret-engine` open_secret NULL-ws tests — classify any failure; a failure in egret-catalog-engine `jwt_auth` (no MinIO) or egret-catalog/egret-config/egret-bin = BLOCKER.
- [ ] **Step 4:** Document that real-Keycloak end-to-end (a real engine JWT → catalog table create under `notebook_{user}`, + a cross-user 403) is exercised in **F2-P5**, not here (it needs MinIO + Keycloak + a minted user token).

---

## Self-review
- **Spec coverage (F2 §4.3 — catalog JWT authn, MVP slice):** JWKS verify (T3 via limes), subject→user→`notebook_{user}` rw grant (T3), membership gate (T3), hybrid with opaque tokens (T4 branch), config audience (T1), reverse resolver (T2). Group-based shared reads explicitly DEFERRED (scope note). ✔
- **Placeholder scan:** the test seeding steps say "copy column lists from existing tests" because those inserts already exist in-repo; the limes `Authentication::subject()` accessor + a couple of egret repo signatures are marked "verify at impl time" because they're external/cross-crate — these are integration points, not logic placeholders. The algorithm + types are fully specified.
- **Type consistency:** `CatalogJwtAuth::authenticate(token, prefix_uuid) -> Result<TokenAuth, JwtAuthError>`; `AccessTokenVerifier::verify(&str) -> Result<String, JwtAuthError>`; `JwtAuthError::{Unauthenticated→401, Forbidden→403}`; `warehouse_by_lakekeeper_id -> Option<(workspace_id, row_id)>`; `TokenAuth{workspace_id, warehouse_id, grants, permissions}` with `Grant{namespace, table, permissions}` — used identically across tasks and matching the opaque-token shape so downstream is unchanged.
- **Security:** JWT verified for sig/iss/aud/exp by limes; user must be a workspace member; grant is scoped to the user's OWN namespace only; opaque-token path untouched; `/config` (no warehouse prefix) rejects JWT (opaque tokens handle it). The engine JWT (access token) is short-lived (F2-P1).
