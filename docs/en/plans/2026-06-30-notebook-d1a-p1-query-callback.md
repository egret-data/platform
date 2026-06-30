# Notebook D1a · P1 — Kernel Token + Query Callback (Rust) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A token-authenticated `POST /api/kernel/query` endpoint that runs SQL on a workspace's enabled notebook engine as the end user (Ranger `X-Trino-User` impersonation) and returns JSON rows — plus a real `EGRET_KERNEL_TOKEN` minted at kernel spawn (replacing the D1b placeholder).

**Architecture:** The kernel token is a random bearer minted on the spawn path of `KernelManager::ensure_running`, hashed (SHA-256) onto the `kernels` row, injected into the kernel env, and revoked when the row is deleted. A `require_kernel_token` extractor resolves the bearer → `(user_id, workspace_id)`. The query handler lives in `egret-kernel`, gates the engine via `NotebookStore::list_engine_options`, and delegates execution to an injected `NotebookQueryRunner` trait (concrete `EngineQueryRunner` built in `egret-bin` from `DatabaseRepo` + `DefaultConnectorFactory`) — so `egret-kernel` needs no `egret-engine`/`egret-database` dependency, mirroring the launcher/proxy injection pattern.

**Tech Stack:** Rust (axum 0.8, sqlx, sha2/hex, tokio), Postgres. Engine path reuses `egret-engine` `ConnectorFactory`/`Connector::execute` + F1 impersonation.

**Spec:** `docs/en/specs/2026-06-30-notebook-d1a-kernel-image-design.md`

**Decisions/constraints:** Migrations APPEND-ONLY (next = `0043`). Git identity `hoangquocvietbk2.13@gmail.com`; Co-Authored-By trailer. Tests need `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret`. Token minted ONLY on spawn (reuse keeps the existing token). Always impersonate via `session_user=username` (the connection's `impersonate` flag — F1 — decides whether `X-Trino-User` is actually sent).

---

## File structure
- `migrations/0043_kernel_token.sql` (new) — add `token_hash`, `token_expires_at` to `kernels`.
- `crates/egret-kernel/src/kernel_token.rs` (new) — `mint()` / `hash_token()` / `TOKEN_TTL`.
- `crates/egret-kernel/src/registry.rs` (modify) — `set_token`, `resolve_token`.
- `crates/egret-kernel/src/manager.rs` (modify) — mint+inject+store on the spawn path.
- `crates/egret-kernel/src/query.rs` (new) — `NotebookQueryRunner` trait + `QueryRows`/`QueryColumn`/`QueryRunError` + `FakeQueryRunner`.
- `crates/egret-kernel/src/routes.rs` (modify) — `KernelIdentity` extractor, `KernelState.query_runner`, `kernel_query` handler + route.
- `crates/egret-kernel/src/lib.rs` (modify) — module decls + re-exports.
- `crates/egret-kernel/tests/http.rs` (modify) — `FakeQueryRunner` wiring + query tests.
- `crates/egret-bin/src/main.rs` (modify) — `EngineQueryRunner` adapter + wire `query_runner`.

---

## Task 1: Migration 0043 + token store methods (TDD)

**Files:** Create `migrations/0043_kernel_token.sql`; modify `crates/egret-kernel/src/registry.rs`, `crates/egret-metastore/src/lib.rs`.

- [ ] **Step 1** — `migrations/0043_kernel_token.sql` (append-only):
```sql
ALTER TABLE kernels ADD COLUMN token_hash text;
ALTER TABLE kernels ADD COLUMN token_expires_at timestamptz;
CREATE INDEX kernels_token_hash_idx ON kernels (token_hash);
```
Lightly touch `crates/egret-metastore/src/lib.rs` doc-comment to force migrator re-embed.

- [ ] **Step 2: Failing test** in `registry.rs` tests module (reuse the existing `seed` helper that returns `(ws, user)`):
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn set_and_resolve_token(pool: sqlx::PgPool) {
    let db = Db { pool };
    let reg = KernelRegistry::new(db.clone());
    let (ws, user) = seed(&db).await;
    let key = KernelKey { user_id: user, workspace_id: ws };
    reg.upsert(key, "http://127.0.0.1:1", Some(1), "ready").await.unwrap();

    // No token yet → resolve returns None.
    assert!(reg.resolve_token("deadbeef").await.unwrap().is_none());

    // Set a token → resolves to this key while unexpired.
    reg.set_token(key, "abc123", Utc::now() + chrono::Duration::hours(1)).await.unwrap();
    assert_eq!(reg.resolve_token("abc123").await.unwrap(), Some(key));

    // Expired token → not resolved.
    reg.set_token(key, "old", Utc::now() - chrono::Duration::minutes(1)).await.unwrap();
    assert!(reg.resolve_token("old").await.unwrap().is_none());

    // Deleting the row revokes the token.
    reg.set_token(key, "live", Utc::now() + chrono::Duration::hours(1)).await.unwrap();
    reg.delete(key).await.unwrap();
    assert!(reg.resolve_token("live").await.unwrap().is_none());
}
```
(`KernelKey` already derives `PartialEq, Eq`.)

- [ ] **Step 3: Run — fails** `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && cargo test -p egret-kernel set_and_resolve_token` → FAIL (methods missing).

- [ ] **Step 4: Implement** in `registry.rs`:
```rust
/// Set the hashed kernel token + its expiry for `key`.
pub async fn set_token(
    &self,
    key: KernelKey,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), KernelError> {
    sqlx::query(
        "UPDATE kernels SET token_hash = $3, token_expires_at = $4 \
         WHERE user_id = $1 AND workspace_id = $2",
    )
    .bind(key.user_id)
    .bind(key.workspace_id)
    .bind(token_hash)
    .bind(expires_at)
    .execute(&self.db.pool)
    .await?;
    Ok(())
}

/// Resolve a hashed kernel token to its `(user_id, workspace_id)`, if a live
/// (unexpired) kernel row carries it.
pub async fn resolve_token(&self, token_hash: &str) -> Result<Option<KernelKey>, KernelError> {
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT user_id, workspace_id FROM kernels \
         WHERE token_hash = $1 AND token_expires_at > now()",
    )
    .bind(token_hash)
    .fetch_optional(&self.db.pool)
    .await?;
    Ok(row.map(|(user_id, workspace_id)| KernelKey { user_id, workspace_id }))
}
```

- [ ] **Step 5: Run — passes**; `cargo fmt --all`; `cargo clippy -p egret-kernel --all-targets -- -D warnings`.

- [ ] **Step 6: Commit**
```
git add migrations/0043_kernel_token.sql crates/egret-metastore/src/lib.rs crates/egret-kernel/src/registry.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): kernel-token columns + registry set/resolve_token

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Token mint helper + mint-on-spawn wiring (TDD)

**Files:** Create `crates/egret-kernel/src/kernel_token.rs`; modify `crates/egret-kernel/src/manager.rs`, `crates/egret-kernel/src/lib.rs`, `crates/egret-kernel/Cargo.toml`.

- [ ] **Step 1: Deps** — ensure `crates/egret-kernel/Cargo.toml` has `sha2`, `hex`, `rand` (check `[workspace.dependencies]`; egret-catalog already hashes tokens with sha2/hex — reuse the same versions via `.workspace = true`; add `rand.workspace = true` if present, else `rand = "0.8"`).

- [ ] **Step 2: Create `kernel_token.rs`** with unit tests:
```rust
//! Kernel callback bearer token: random plaintext + SHA-256 hash for at-rest storage.

use std::time::Duration;

use sha2::{Digest, Sha256};

/// Token lifetime backstop (the row delete on stop/reap is the primary revocation).
pub const TOKEN_TTL: Duration = Duration::from_secs(12 * 3600);

/// Mint a fresh token: returns `(plaintext, sha256_hex_hash)`.
pub fn mint() -> (String, String) {
    let raw: [u8; 32] = rand::random();
    let token = format!("egk_{}", hex::encode(raw));
    let hash = hash_token(&token);
    (token, hash)
}

/// SHA-256 hex of a token (for lookup + at-rest comparison).
pub fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_is_unique_and_hash_is_stable() {
        let (t1, h1) = mint();
        let (t2, _h2) = mint();
        assert_ne!(t1, t2, "tokens must be unique");
        assert!(t1.starts_with("egk_"));
        assert_eq!(hash_token(&t1), h1, "hash must be deterministic");
        assert_ne!(hash_token(&t1), hash_token(&t2));
        assert_eq!(h1.len(), 64, "sha256 hex is 64 chars");
    }
}
```
Add `pub mod kernel_token;` to `lib.rs`.

- [ ] **Step 3: Failing test** in `manager.rs` tests — token minted + stored on spawn, NOT re-minted on reuse:
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn ensure_running_mints_token_on_spawn_only(pool: sqlx::PgPool) {
    let db = Db { pool };
    let reg = KernelRegistry::new(db.clone());
    let launcher = Arc::new(FakeLauncher::default());
    let mgr = KernelManager::new(reg, launcher.clone(), Duration::from_secs(1800));
    let (ws, user) = seed_kernel(&db).await;
    let key = KernelKey { user_id: user, workspace_id: ws };
    let env = KernelEnv::new();

    // Spawn: env handed to the launcher must carry EGRET_KERNEL_TOKEN, and the
    // registry must resolve that token back to this key.
    mgr.ensure_running(key, &env).await.unwrap();
    let plaintext = {
        let g = launcher.inner.lock().unwrap();
        g.ensured[0].1.get("EGRET_KERNEL_TOKEN").cloned()
    };
    let plaintext = plaintext.expect("spawn env must include EGRET_KERNEL_TOKEN");
    assert!(!plaintext.is_empty());
    let hash = crate::kernel_token::hash_token(&plaintext);
    assert_eq!(mgr.registry().resolve_token(&hash).await.unwrap(), Some(key));

    // Reuse (launcher reports Ready): no new ensure() call → token unchanged.
    mgr.ensure_running(key, &env).await.unwrap();
    assert_eq!(launcher.inner.lock().unwrap().ensured.len(), 1, "reuse must not re-spawn");
    assert_eq!(mgr.registry().resolve_token(&hash).await.unwrap(), Some(key), "token must persist across reuse");
}
```

- [ ] **Step 4: Run — fails** → FAIL.

- [ ] **Step 5: Implement** — in `manager.rs`, change the spawn branch of `ensure_running` (the code AFTER the reuse `return`):
```rust
        // Spawn (or re-spawn after stale/dead). Mint a fresh kernel token and
        // inject it into the env so the kernel can call egret back.
        let (token_plain, token_hash) = crate::kernel_token::mint();
        let mut env = env.clone();
        env.insert("EGRET_KERNEL_TOKEN".to_string(), token_plain);

        let h = self
            .launcher
            .ensure(key, &env, Duration::from_secs(90))
            .await?;
        self.registry
            .upsert(key, &h.url, h.pid, h.phase.as_str())
            .await?;
        let expires_at = chrono::Utc::now()
            + chrono::Duration::from_std(crate::kernel_token::TOKEN_TTL).unwrap();
        self.registry.set_token(key, &token_hash, expires_at).await?;
        Ok(KernelStatus {
            phase: h.phase,
            url: h.url,
        })
```
(The reuse branch is unchanged and returns before this. `manifest_to_env` is untouched — callers still pass `""`; this insert overrides it on spawn.)

- [ ] **Step 6: Run — passes**; clippy clean. Confirm the full crate still passes: `cargo test -p egret-kernel`.

- [ ] **Step 7: Commit**
```
git add crates/egret-kernel/src/kernel_token.rs crates/egret-kernel/src/manager.rs crates/egret-kernel/src/lib.rs crates/egret-kernel/Cargo.toml Cargo.toml
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): mint EGRET_KERNEL_TOKEN on spawn (hashed, revoked on stop)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `NotebookQueryRunner` trait + types + fake (TDD)

**Files:** Create `crates/egret-kernel/src/query.rs`; modify `crates/egret-kernel/src/lib.rs`.

- [ ] **Step 1: Create `query.rs`**:
```rust
//! `NotebookQueryRunner` — the engine-execution seam for the kernel query callback.
//! The concrete runner (egret-engine + egret-database) is injected from egret-bin,
//! so this crate stays free of an engine dependency.

use async_trait::async_trait;
use serde::Serialize;
use uuid::Uuid;

/// A result column (name + engine type label).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct QueryColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// A fully-collected query result for the kernel callback.
#[derive(Debug, Clone, Serialize)]
pub struct QueryRows {
    pub columns: Vec<QueryColumn>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub truncated: bool,
}

/// Failure running a notebook query.
#[derive(Debug)]
pub enum QueryRunError {
    /// The database/engine could not be resolved.
    NotFound(String),
    /// The engine rejected the query (SQL error, timeout, connection failure).
    Engine(String),
}

#[async_trait]
pub trait NotebookQueryRunner: Send + Sync {
    /// Run `sql` against `database_id` as `session_user` (Trino `X-Trino-User`
    /// when the connection opts into impersonation), capped at `max_rows`.
    async fn run(
        &self,
        database_id: Uuid,
        session_user: String,
        sql: String,
        max_rows: usize,
    ) -> Result<QueryRows, QueryRunError>;
}

/// Test double: returns canned rows and records the last call.
#[cfg(any(test, feature = "test-util"))]
#[derive(Default)]
pub struct FakeQueryRunner {
    pub last: std::sync::Mutex<Option<(Uuid, String, String, usize)>>,
}

#[cfg(any(test, feature = "test-util"))]
#[async_trait]
impl NotebookQueryRunner for FakeQueryRunner {
    async fn run(
        &self,
        database_id: Uuid,
        session_user: String,
        sql: String,
        max_rows: usize,
    ) -> Result<QueryRows, QueryRunError> {
        *self.last.lock().unwrap() = Some((database_id, session_user, sql, max_rows));
        Ok(QueryRows {
            columns: vec![QueryColumn { name: "n".into(), ty: "integer".into() }],
            rows: vec![vec![serde_json::json!(1)]],
            truncated: false,
        })
    }
}
```
Add to `lib.rs`: `pub mod query;` and `pub use query::{NotebookQueryRunner, QueryColumn, QueryRows, QueryRunError};`. Add a `test-util` feature to `crates/egret-kernel/Cargo.toml` (`[features]\ntest-util = []`) so the integration test (a separate crate) can construct `FakeQueryRunner`; and add `egret-kernel = { path = "...", features = ["test-util"] }` is NOT needed — instead enable it in `[dev-dependencies]`? Simpler: gate `FakeQueryRunner` on `#[cfg(any(test, feature = "test-util"))]` AND add `egret-kernel.workspace`-style dev usage. **Concrete approach:** in `crates/egret-kernel/Cargo.toml` add `[features] test-util = []`, and in its `[dev-dependencies]` add `egret-kernel = { path = ".", features = ["test-util"] }` so `tests/http.rs` sees `FakeQueryRunner`. (This self-dev-dep pattern is how a crate exposes test-only helpers to its own integration tests.)

- [ ] **Step 2: Run** `cargo build -p egret-kernel --features test-util` → compiles. (No behavior test needed yet — exercised in Task 4.)

- [ ] **Step 3: Commit**
```
git add crates/egret-kernel/src/query.rs crates/egret-kernel/src/lib.rs crates/egret-kernel/Cargo.toml
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): NotebookQueryRunner trait + QueryRows types + fake

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `KernelIdentity` extractor + query handler + route (TDD http)

**Files:** Modify `crates/egret-kernel/src/routes.rs`, `crates/egret-kernel/tests/http.rs`.

- [ ] **Step 1: KernelState field + extractor + route** in `routes.rs`:
Add field to `KernelState`: `pub query_runner: Arc<dyn crate::query::NotebookQueryRunner>`. Add imports (`use axum::http::header::AUTHORIZATION; use axum::extract::FromRequestParts; use axum::http::request::Parts;`). Add:
```rust
/// Identity resolved from an `EGRET_KERNEL_TOKEN` bearer (the kernel-callback plane).
pub struct KernelIdentity {
    pub user_id: Uuid,
    pub workspace_id: Uuid,
}

impl FromRequestParts<KernelState> for KernelIdentity {
    type Rejection = KernelApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        st: &KernelState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or(KernelApiError(egret_core::Error::Unauthorized))?;
        let hash = crate::kernel_token::hash_token(token);
        let key = st
            .manager
            .registry()
            .resolve_token(&hash)
            .await
            .map_err(KernelApiError::from)?
            .ok_or(KernelApiError(egret_core::Error::Unauthorized))?;
        Ok(KernelIdentity { user_id: key.user_id, workspace_id: key.workspace_id })
    }
}
```
Add the route in `kernel_router()`: `.route("/kernel/query", post(kernel_query))`. Add the handler:
```rust
#[derive(serde::Deserialize)]
struct QueryReq {
    engine: String,
    sql: String,
    #[serde(default)]
    max_rows: Option<usize>,
}

/// `POST /api/kernel/query` — run SQL on an enabled notebook engine as the
/// token's user (Ranger `X-Trino-User`). Auth = `EGRET_KERNEL_TOKEN` bearer.
async fn kernel_query(
    State(st): State<KernelState>,
    ident: KernelIdentity,
    Json(req): Json<QueryReq>,
) -> Result<Json<serde_json::Value>, KernelApiError> {
    // Engine must be enabled for notebooks in the token's workspace.
    let engines = st
        .notebook
        .store
        .list_engine_options(ident.workspace_id)
        .await
        .map_err(|e| KernelApiError(e.0))?;
    let eng = engines
        .into_iter()
        .find(|e| e.enabled && (e.id.to_string() == req.engine || e.name == req.engine))
        .ok_or(KernelApiError(egret_core::Error::Forbidden))?;

    let username = st
        .notebook
        .users
        .ensure_username(egret_core::UserId(ident.user_id))
        .await
        .map_err(|e| KernelApiError(e.0))?;

    let max_rows = req.max_rows.unwrap_or(10_000).min(100_000);
    let rows = st
        .query_runner
        .run(eng.id, username, req.sql, max_rows)
        .await
        .map_err(|e| match e {
            crate::query::QueryRunError::NotFound(m) => {
                KernelApiError(egret_core::Error::NotFound(m))
            }
            crate::query::QueryRunError::Engine(m) => {
                KernelApiError(egret_core::Error::InvalidInput(m))
            }
        })?;

    Ok(Json(serde_json::json!({
        "columns": rows.columns,
        "rows": rows.rows,
        "truncated": rows.truncated,
    })))
}
```
> Verify-first: confirm `UserRepo::ensure_username` takes `egret_core::UserId` (it's on `st.notebook.users`); if it takes a raw `Uuid`, drop the `UserId(..)` wrapper. Confirm `EngineOption` exposes `id`, `name`, `enabled` (it does — `egret_notebook::model::EngineOption`).

- [ ] **Step 2: Failing http tests** in `tests/http.rs`. First extend `setup()`'s `KernelState { ... }` with `query_runner: Arc::new(egret_kernel::query::FakeQueryRunner::default())`. Add a helper to seed an enabled notebook engine + mint a token row, and a bearer `req`:
```rust
/// Seed a database (engine) row + enable it for notebooks; return its id.
async fn seed_enabled_engine(db: &Db, ws: WorkspaceId, owner: UserId) -> Uuid {
    let dbid: Uuid = sqlx::query_scalar(
        "INSERT INTO databases (workspace_id, name, engine, host, port, created_by) \
         VALUES ($1,'trino-dev','trino','localhost',8080,$2) RETURNING id",
    )
    .bind(ws.0).bind(owner.0).fetch_one(&db.pool).await.expect("seed database");
    sqlx::query("INSERT INTO notebook_engine_access (workspace_id, database_id) VALUES ($1,$2)")
        .bind(ws.0).bind(dbid).execute(&db.pool).await.expect("enable engine");
    dbid
}

/// Mint a kernel token for (ws,user) directly via the registry; return the plaintext bearer.
async fn mint_kernel_token(db: &Db, ws: WorkspaceId, user: UserId) -> String {
    let reg = KernelRegistry::new(db.clone());
    let key = egret_kernel::registry::KernelKey { user_id: user.0, workspace_id: ws.0 };
    reg.upsert(key, "http://127.0.0.1:1", Some(1), "ready").await.unwrap();
    let (plain, hash) = egret_kernel::kernel_token::mint();
    reg.set_token(key, &hash, chrono::Utc::now() + chrono::Duration::hours(1)).await.unwrap();
    plain
}

fn bearer_post(uri: &str, token: &str, body: serde_json::Value) -> HttpRequest<Body> {
    HttpRequest::builder()
        .method("POST")
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}
```
> Verify-first: confirm the `databases` table columns (`workspace_id,name,engine,host,port,created_by`) and `notebook_engine_access` columns (`workspace_id,database_id`) by reading the relevant migrations; adjust the INSERTs to the actual schema. If `databases` requires more NOT NULL columns, add them.

Tests:
```rust
#[sqlx::test(migrations = "../../migrations")]
async fn kernel_query_runs_for_valid_token_and_enabled_engine(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let dbid = seed_enabled_engine(&f.db, f.ws, f.owner).await;
    let token = mint_kernel_token(&f.db, f.ws, f.owner).await;

    let resp = f.app.clone().oneshot(bearer_post(
        "/kernel/query", &token,
        serde_json::json!({ "engine": dbid.to_string(), "sql": "SELECT 1" }),
    )).await.unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "valid query → 200; body={json}");
    assert!(json["columns"].is_array() && json["rows"].is_array(), "body={json}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn kernel_query_rejects_bad_token(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_enabled_engine(&f.db, f.ws, f.owner).await;
    let resp = f.app.clone().oneshot(bearer_post(
        "/kernel/query", "egk_not_a_real_token",
        serde_json::json!({ "engine": "trino-dev", "sql": "SELECT 1" }),
    )).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "bad token → 401");
}

#[sqlx::test(migrations = "../../migrations")]
async fn kernel_query_rejects_disabled_engine(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    // A database that is NOT enabled for notebooks.
    let dbid: Uuid = sqlx::query_scalar(
        "INSERT INTO databases (workspace_id, name, engine, host, port, created_by) \
         VALUES ($1,'secret','trino','localhost',8080,$2) RETURNING id",
    ).bind(f.ws.0).bind(f.owner.0).fetch_one(&f.db.pool).await.unwrap();
    let token = mint_kernel_token(&f.db, f.ws, f.owner).await;

    let resp = f.app.clone().oneshot(bearer_post(
        "/kernel/query", &token,
        serde_json::json!({ "engine": dbid.to_string(), "sql": "SELECT 1" }),
    )).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "disabled engine → 403");
}
```

- [ ] **Step 3: Run — fails** `cargo test -p egret-kernel --test http kernel_query` → FAIL (route/field missing).

- [ ] **Step 4: Implement** the field + extractor + handler + route (Step 1 code). Update `setup()` as in Step 2.

- [ ] **Step 5: Run — passes** `cargo test -p egret-kernel --test http` (existing 12 + new 3). `cargo fmt --all`; clippy clean.

- [ ] **Step 6: Commit**
```
git add crates/egret-kernel/src/routes.rs crates/egret-kernel/tests/http.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): POST /kernel/query — token-authed governed query callback

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `EngineQueryRunner` adapter + wire egret-bin

**Files:** Modify `crates/egret-bin/src/main.rs` (+ `crates/egret-bin/Cargo.toml` if egret-engine/egret-database aren't deps).

- [ ] **Step 1: Implement the adapter** near the kernel wiring in `main.rs`:
```rust
struct EngineQueryRunner {
    databases: egret_database::repo::DatabaseRepo,
    factory: std::sync::Arc<dyn egret_engine::connector::ConnectorFactory>,
}

#[async_trait::async_trait]
impl egret_kernel::query::NotebookQueryRunner for EngineQueryRunner {
    async fn run(
        &self,
        database_id: uuid::Uuid,
        session_user: String,
        sql: String,
        max_rows: usize,
    ) -> Result<egret_kernel::query::QueryRows, egret_kernel::query::QueryRunError> {
        use egret_kernel::query::{QueryColumn, QueryRows, QueryRunError};
        let db = self
            .databases
            .get(database_id)
            .await
            .map_err(|e| QueryRunError::NotFound(e.to_string()))?;
        let secret = self
            .databases
            .open_secret(database_id)
            .await
            .map_err(|e| QueryRunError::Engine(e.to_string()))?;
        let connector = self
            .factory
            .build(&db, secret, Some(session_user))
            .await
            .map_err(|e| QueryRunError::Engine(e.to_string()))?;
        let limits = egret_engine::model::Limits {
            max_rows,
            timeout: std::time::Duration::from_secs(60),
        };
        let fut = connector.execute(&sql, &limits, tokio_util::sync::CancellationToken::new());
        let result = tokio::time::timeout(limits.timeout, fut)
            .await
            .map_err(|_| QueryRunError::Engine("query timed out".into()))?
            .map_err(|e| QueryRunError::Engine(e.to_string()))?;
        Ok(QueryRows {
            columns: result
                .columns
                .into_iter()
                .map(|c| QueryColumn { name: c.name, ty: c.ty })
                .collect(),
            rows: result.rows,
            truncated: result.truncated,
        })
    }
}
```
> Verify-first: confirm the `DatabaseRepo` path (`egret_database::repo::DatabaseRepo` vs a re-export) and that `egret_engine::model::Limits`/`Column.ty` are the actual public names (they are per the engine crate). Add `egret-engine` / `egret-database` to `crates/egret-bin/Cargo.toml` only if not already present (they are — the engine state is built here).

- [ ] **Step 2: Wire the field** in the `KernelState { ... }` literal (the kernel-wiring block near where `notebook_kernels`/`proxy` were added):
```rust
        query_runner: std::sync::Arc::new(EngineQueryRunner {
            databases: egret_database::repo::DatabaseRepo::new(db.clone()),
            factory: std::sync::Arc::new(egret_engine::connector::DefaultConnectorFactory),
        }),
```
> Verify-first: reuse the existing `DatabaseRepo`/factory if one is already constructed for the engine state (avoid a second instance); otherwise construct as shown.

- [ ] **Step 3: Build** `cargo build -p egret-bin` → compiles. `cargo clippy -p egret-bin --all-targets -- -D warnings` clean.

- [ ] **Step 4: Commit**
```
git add crates/egret-bin/src/main.rs crates/egret-bin/Cargo.toml
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel): wire EngineQueryRunner (egret-engine impersonated exec) into KernelState

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Full local CI (P1)

- [ ] **Step 1** — `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret`; `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast` → all green (2 pre-existing ignores expected).
- [ ] **Step 2** — Frontend unaffected (no FE changes); skip unless touched.
- [ ] **Step 3** — Do NOT open the PR yet; P1 + P2 ship together on the same `feat/notebook-d1a-kernel` branch (one D1a PR). Proceed to the P2 plan.

---

## Self-review notes (author)
- **Spec coverage:** kernel token §3.1 (T1 columns+store, T2 mint-on-spawn, revoke-on-delete via existing `delete`), query callback §3.2 (T4 handler: token auth + enabled-engine gate + `X-Trino-User` via injected runner + JSON), runner seam (T3 trait/types/fake, T5 adapter). ✔
- **Type consistency:** `KernelRegistry::{set_token,resolve_token}`, `kernel_token::{mint,hash_token,TOKEN_TTL}`, `NotebookQueryRunner::run(database_id, session_user, sql, max_rows) -> QueryRows`, `QueryColumn{name,ty(rename "type")}`, `KernelIdentity{user_id,workspace_id}`, `KernelState.query_runner`, `EngineQueryRunner` align across tasks. ✔
- **Verify-first points flagged** (read-before-write): `databases`/`notebook_engine_access` exact columns; `UserRepo::ensure_username` arg type; `DatabaseRepo` import path; reuse existing factory/repo in egret-bin. ✔
- Migration append-only (0043 adds columns). Token minted on spawn only; reuse keeps it; row-delete revokes. Engine gate prevents querying non-enabled engines. ✔
