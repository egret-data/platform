# E0 Foundation · Plan 1b — Migrate remaining error consumers onto the shared mapping

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete every remaining copy of the `egret_core::Error → HTTP` match arm by routing five crates through the shared mapping landed in P1 (`egret_core::ApiError` / `impl IntoResponse for Error`).

**Architecture:** Two shapes. (a) Crates with a two-arm `XxxApiError { Auth(core::Error), Domain(XxxError) }` become `type XxxApiError = ApiError<XxxError>` + a domain-only `impl IntoResponse for XxxError` (the `Auth` arm is now shared). (b) Crates whose HTTP error is a newtype wrapping only `core::Error` (`struct XxxApiError(pub Error)`) delete their copied match and delegate `into_response` to `self.0.into_response()`. This completes §4.2 of the [service-ready architecture design](../architecture/2026-07-08-service-ready-architecture-design.md); it depends on P1 (already merged, PR #175).

**Tech Stack:** Rust 2021, axum 0.8. Workspace under `crates/`.

**Scope — 5 crates:**
- Two-arm → `ApiError<D>`: **egret-database**, **egret-engine**.
- Newtype-over-core → delegate: **egret-notebook**, **egret-kernel**, **egret-auth** (`principal.rs`).

**Explicitly OUT of scope (do not touch):**
- **egret-controlplane** — `CpError` is a domain enum with a *translating* `From<egret_core::Error>` (it maps `Unauthorized|Forbidden → CpError::Forbidden`, i.e. both to 403, and `TooManyRequests → BadRequest`). It is **not** a copy of the shared arm; routing it through the shared mapping would change status codes (Unauthorized would become 401 instead of the intended 403). Leave `crates/egret-controlplane/src/error.rs` unchanged.
- **egret-catalog-engine** — has no domain error / `IntoResponse` at all (it is `anyhow`-only). Giving it one is an *introduce*, not a *migrate*, change and needs its own investigation → separate plan **E0·P1c**.

**Behavior delta (applies to all 5, disclosed):** the shared mapping renders `Error::{NotFound, InvalidInput, Conflict}` bodies via full `Display` (`"not found: X"`, `"invalid input: X"`, `"conflict: X"`). The current per-crate arms truncate these (drop the prefix or the detail). **Status codes are unchanged everywhere**; only the JSON `error` string for those three variants gains its type prefix. Each task greps that crate's tests for body-text assertions on those variants and confirms none break. This standardization is the intended effect of the effort.

---

## File structure

- `crates/egret-database/{Cargo.toml, src/error.rs}` — two-arm → `ApiError<DatabaseError>`.
- `crates/egret-engine/{Cargo.toml, src/error.rs}` — two-arm → `ApiError<EngineError>`.
- `crates/egret-notebook/{Cargo.toml, src/error.rs}` — newtype delegate.
- `crates/egret-kernel/{Cargo.toml, src/error.rs}` — newtype delegate.
- `crates/egret-auth/{Cargo.toml, src/principal.rs}` — newtype delegate (`AuthError`).

Each `Cargo.toml` change is the same: find the `egret-core` dependency (currently `egret-core.workspace = true`) and set it to `egret-core = { workspace = true, features = ["http"] }`.

---

## Task 1: egret-database → `ApiError<DatabaseError>`

**Files:** Modify `crates/egret-database/Cargo.toml`, `crates/egret-database/src/error.rs`

- [ ] **Step 1: Enable the http feature**

In `crates/egret-database/Cargo.toml` set: `egret-core = { workspace = true, features = ["http"] }`

- [ ] **Step 2: Rewrite `src/error.rs`**

Overwrite `crates/egret-database/src/error.rs` with (the `DatabaseError` enum + `From<SecretsError>` are unchanged; the two-arm `DbApiError` enum and its copied `Auth` match are replaced):

```rust
//! Domain + HTTP error types for the database registry.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::ApiError;
use egret_secrets::SecretsError;

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("secrets master_key is not configured")]
    SecretsNotConfigured,
    #[error(transparent)]
    Secrets(SecretsError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<SecretsError> for DatabaseError {
    fn from(e: SecretsError) -> Self {
        match e {
            SecretsError::NotConfigured => DatabaseError::SecretsNotConfigured,
            SecretsError::Db(db) => DatabaseError::Db(db),
            other => DatabaseError::Secrets(other),
        }
    }
}

/// Domain arm only — the shared `Auth` arm is in `egret_core::ApiError`.
impl IntoResponse for DatabaseError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            DatabaseError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            DatabaseError::AlreadyExists(_) => (StatusCode::CONFLICT, self.to_string()),
            DatabaseError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            DatabaseError::SecretsNotConfigured => (StatusCode::BAD_REQUEST, self.to_string()),
            DatabaseError::Secrets(_) | DatabaseError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

pub type DbApiError = ApiError<DatabaseError>;

impl From<DatabaseError> for DbApiError {
    fn from(e: DatabaseError) -> Self {
        ApiError::Domain(e)
    }
}
```

- [ ] **Step 3: Build**

Run: `cargo build -p egret-database`
Expected: clean. Call sites that write `DbApiError::Auth(..)`/`DbApiError::Domain(..)`/`.into()`/`?` still compile (`DbApiError` is now an alias for `ApiError<DatabaseError>`; the same variant names apply, `From<core::Error>` comes from core, `From<DatabaseError>` from this file). If any code *pattern-matches* `DbApiError` variants, adjust it to `ApiError::Auth(_)`/`ApiError::Domain(_)`. Do not change handler logic.

- [ ] **Step 4: Run tests + confirm no body-text assertions break**

Run: `cargo test -p egret-database`
Expected: PASS.
Then run: `grep -rn '"error"' crates/egret-database/tests` — inspect any hit; confirm none assert the old truncated `NotFound`/`InvalidInput`/`Conflict` **auth-arm** body text. Report what you found.

- [ ] **Step 5: Confirm the copied arm is gone**

Run: `grep -n "egret_core::Error::" crates/egret-database/src/error.rs`
Expected: **no output** (the translating/mapping references are removed; only the shared core mapping remains).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-database/Cargo.toml crates/egret-database/src/error.rs
git commit -m "refactor(database): use egret_core::ApiError, drop duplicated core-error mapping

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: egret-engine → `ApiError<EngineError>`

**Files:** Modify `crates/egret-engine/Cargo.toml`, `crates/egret-engine/src/error.rs`

- [ ] **Step 1: Enable the http feature**

In `crates/egret-engine/Cargo.toml` set: `egret-core = { workspace = true, features = ["http"] }`

- [ ] **Step 2: Rewrite `src/error.rs`**

Overwrite `crates/egret-engine/src/error.rs` with (the `EngineError` enum + `From<DatabaseError>` unchanged; the two-arm `EngineApiError` + its copied `Auth` match replaced; the domain arm — including the trailing `_ => BadRequest` catch-all — preserved exactly):

```rust
//! Engine error + HTTP mapping.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("connect: {0}")]
    Connect(String),
    #[error("engine: {0}")]
    Engine(String),
    #[error("timeout")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("limit: {0}")]
    Limit(String),
    #[error(transparent)]
    Storage(#[from] egret_storage::StorageError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<egret_database::DatabaseError> for EngineError {
    fn from(e: egret_database::DatabaseError) -> Self {
        match e {
            egret_database::DatabaseError::NotFound(s) => EngineError::NotFound(s),
            other => EngineError::Engine(other.to_string()),
        }
    }
}

/// Domain arm only — the shared `Auth` arm is in `egret_core::ApiError`.
impl IntoResponse for EngineError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            EngineError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            EngineError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            EngineError::BadRequest(_) | EngineError::Limit(_) => {
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            EngineError::Storage(_) | EngineError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
            _ => (StatusCode::BAD_REQUEST, self.to_string()),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

pub type EngineApiError = ApiError<EngineError>;

impl From<EngineError> for EngineApiError {
    fn from(e: EngineError) -> Self {
        ApiError::Domain(e)
    }
}
```

- [ ] **Step 3: Build**

Run: `cargo build -p egret-engine`
Expected: clean (same alias-compatibility note as Task 1 Step 3).

- [ ] **Step 4: Run tests + body-assertion check**

Run: `cargo test -p egret-engine`
Expected: PASS.
Then: `grep -rn '"error"' crates/egret-engine/tests` — confirm no test asserts the old truncated auth-arm body text. Report findings.

- [ ] **Step 5: Confirm the copied arm is gone**

Run: `grep -n "egret_core::Error::" crates/egret-engine/src/error.rs`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-engine/Cargo.toml crates/egret-engine/src/error.rs
git commit -m "refactor(engine): use egret_core::ApiError, drop duplicated core-error mapping

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: egret-notebook → delegate to the shared mapping

`NotebookApiError` is a newtype wrapping only `egret_core::Error`; it has no domain enum, so it does not use `ApiError<D>` — it just delegates.

**Files:** Modify `crates/egret-notebook/Cargo.toml`, `crates/egret-notebook/src/error.rs`

- [ ] **Step 1: Enable the http feature**

In `crates/egret-notebook/Cargo.toml` set: `egret-core = { workspace = true, features = ["http"] }`

- [ ] **Step 2: Rewrite `src/error.rs`**

Overwrite `crates/egret-notebook/src/error.rs` with (copied match deleted; `into_response` delegates; now-unused `StatusCode`/`Json` imports removed):

```rust
//! Notebook HTTP error — delegates to the shared `egret_core::Error` mapping.

use axum::response::{IntoResponse, Response};
use egret_core::Error;

#[derive(Debug)]
pub struct NotebookApiError(pub Error);

impl From<Error> for NotebookApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl From<sqlx::Error> for NotebookApiError {
    fn from(e: sqlx::Error) -> Self {
        Self(Error::Internal(anyhow::anyhow!(e)))
    }
}

impl IntoResponse for NotebookApiError {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}
```

- [ ] **Step 3: Build**

Run: `cargo build -p egret-notebook`
Expected: clean, no unused-import warnings.

- [ ] **Step 4: Run tests + body-assertion check**

Run: `cargo test -p egret-notebook`
Expected: PASS.
Then: `grep -rn '"error"' crates/egret-notebook/tests` — the notebook HTTP tests are extensive (`tests/http.rs`); confirm no test asserts a `NotFound`/`InvalidInput`/`Conflict` body as the bare inner message (they would now carry the `"not found: "/"invalid input: "/"conflict: "` prefix). If one does, that is a real body-text change — report it with the test name; do NOT silently edit the test. (Statuses are unchanged, so status-only assertions are unaffected.)

- [ ] **Step 5: Confirm the copied arm is gone**

Run: `grep -n "egret_core::Error::\|Error::NotFound(m)" crates/egret-notebook/src/error.rs`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-notebook/Cargo.toml crates/egret-notebook/src/error.rs
git commit -m "refactor(notebook): delegate to shared egret_core::Error mapping

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: egret-kernel → delegate to the shared mapping

Same shape as notebook. Keep the `KernelError` domain enum (used elsewhere; folded into `Error::Internal` via `From<KernelError>`).

**Files:** Modify `crates/egret-kernel/Cargo.toml`, `crates/egret-kernel/src/error.rs`

- [ ] **Step 1: Enable the http feature**

In `crates/egret-kernel/Cargo.toml` set: `egret-core = { workspace = true, features = ["http"] }`

- [ ] **Step 2: Rewrite `src/error.rs`**

Overwrite `crates/egret-kernel/src/error.rs` with:

```rust
//! Kernel error types — mirrors `egret_notebook::error`.

use axum::response::{IntoResponse, Response};
use egret_core::Error;

/// Domain-level kernel error.
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("gateway error: {0}")]
    Gateway(String),
}

/// HTTP-layer wrapper that delegates to the shared `egret_core::Error` mapping.
#[derive(Debug)]
pub struct KernelApiError(pub Error);

impl From<Error> for KernelApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl From<sqlx::Error> for KernelApiError {
    fn from(e: sqlx::Error) -> Self {
        Self(Error::Internal(anyhow::anyhow!(e)))
    }
}

impl From<KernelError> for KernelApiError {
    fn from(e: KernelError) -> Self {
        Self(Error::Internal(anyhow::anyhow!(e)))
    }
}

impl IntoResponse for KernelApiError {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}
```

- [ ] **Step 3: Build + confirm routes.rs has no second copy**

Run: `cargo build -p egret-kernel`
Expected: clean.
Run: `grep -n "egret_core::Error::\|Error::Unauthorized =>" crates/egret-kernel/src/routes.rs`
Expected: at most *constructions* of `Error::…(…)` (fine) — but **no** HTTP-status `match` block. If `routes.rs` contains its own copied core→HTTP match arm, STOP and report it (it would be an extra sub-task not covered by this plan's exact code).

- [ ] **Step 4: Run tests + body-assertion check**

Run: `cargo test -p egret-kernel`
Expected: PASS.
Then: `grep -rn '"error"' crates/egret-kernel/tests` — confirm no `NotFound`/`InvalidInput`/`Conflict` bare-message body assertion breaks (same prefix note as Task 3). Report findings.

- [ ] **Step 5: Confirm the copied arm is gone**

Run: `grep -n "Error::NotFound(m)\|Error::Unauthorized =>" crates/egret-kernel/src/error.rs`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-kernel/Cargo.toml crates/egret-kernel/src/error.rs
git commit -m "refactor(kernel): delegate to shared egret_core::Error mapping

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: egret-auth (`AuthError`) → delegate to the shared mapping

`AuthError` (in `principal.rs`) is a newtype over `core::Error` used as the `Auth` extractor's rejection. In practice it almost always carries `Error::Unauthorized` (→ 401, identical under the shared mapping); the other variants are theoretically reachable via `SessionService::validate`.

**Files:** Modify `crates/egret-auth/Cargo.toml`, `crates/egret-auth/src/principal.rs`

- [ ] **Step 1: Enable the http feature**

In `crates/egret-auth/Cargo.toml` set: `egret-core = { workspace = true, features = ["http"] }`

- [ ] **Step 2: Edit `src/principal.rs`**

Replace the `AuthError` `IntoResponse` impl (the block currently at lines 47–60, the copied 7-variant match) with a delegation:

```rust
impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}
```

Then remove the now-unused imports `use axum::http::StatusCode;` and `use axum::Json;` from the top of `principal.rs` (keep `use axum::response::{IntoResponse, Response};` and all other imports — the `Auth` extractor still needs them). Do NOT change the `Auth` extractor or `AuthError`'s struct/`From` impls.

- [ ] **Step 3: Build**

Run: `cargo build -p egret-auth`
Expected: clean, no unused-import warnings. (If `Json`/`StatusCode` are used elsewhere in `principal.rs`, keep them — but per the current file they are only used by the deleted match.)

- [ ] **Step 4: Run tests + body-assertion check**

Run: `cargo test -p egret-auth`
Expected: PASS.
Then: `grep -rn '"error"' crates/egret-auth/tests crates/egret-auth/src/principal.rs` — confirm nothing asserts the old fixed `"invalid input"`/`"not found"`/`"conflict"` strings for `AuthError`. Report findings.

- [ ] **Step 5: Confirm the copied arm is gone**

Run: `grep -n "Error::Unauthorized =>\|Error::NotFound(_) =>" crates/egret-auth/src/principal.rs`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-auth/Cargo.toml crates/egret-auth/src/principal.rs
git commit -m "refactor(auth): delegate AuthError to shared egret_core::Error mapping

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Workspace gate + copy sweep

**Files:** none (verification only).

- [ ] **Step 1: Format check**

Run: `cargo fmt --all --check`
Expected: no diff. (Run `cargo fmt --all` + commit `style: cargo fmt` if needed.)

- [ ] **Step 2: Clippy the five touched crates**

Run: `cargo clippy -p egret-database -p egret-engine -p egret-notebook -p egret-kernel -p egret-auth -- -D warnings`
Expected: no warnings (ignore the pre-existing vendored `third_party/mysql_async` lint if it surfaces transitively).

- [ ] **Step 3: Workspace build**

Run: `cargo build --workspace`
Expected: clean.

- [ ] **Step 4: Copy sweep — prove only legitimate references remain**

Run: `grep -rln "egret_core::Error::Unauthorized\|Error::Unauthorized =>" crates/*/src`
Expected files: `crates/egret-core/src/http.rs` (the shared mapping — legit) and `crates/egret-controlplane/src/error.rs` (the *translating* `From<core::Error>`, intentionally excluded — legit). **No other crate should appear.** If any of the five migrated crates still appears, the copy was not fully removed — fix before proceeding.

- [ ] **Step 5: Final commit (only if fmt changed anything)**

```bash
git add -A
git commit -m "style: cargo fmt after error-consumer migration" || echo "nothing to format"
```

---

## Follow-up (separate plans)

- **E0·P1c** — give `egret-catalog-engine` a real domain error + `IntoResponse` (today `anyhow`-only → opaque 500s). Different change (introduce, not migrate); needs a read of its handler/`mount` surface first.
- **E0·P2–P6** as listed in the [P1 plan](./2026-07-08-e0-p1-unified-error-http-plan.md) header.

---

## Self-review notes

- **Spec coverage:** completes §4.2's "migrate the other consumers" for all crates that actually held a *copy* of the shared arm (database, engine, notebook, kernel, auth). Controlplane is correctly excluded (translating enum, not a copy) and catalog-engine is correctly deferred (introduce, not migrate) — both stated with rationale.
- **Placeholder scan:** no `todo!()`, no "similar to Task N" — each task carries its full replacement code.
- **Type consistency:** two-arm crates use `pub type XxxApiError = ApiError<XxxError>` + `impl From<XxxError>` + `impl IntoResponse for XxxError`; newtype crates keep `struct XxxApiError(pub Error)` and delegate via `self.0.into_response()`. Every domain arm preserves the exact statuses from the pre-change code (verified against the current `error.rs` files, including egret-engine's `_ => BadRequest` catch-all).
- **Behavior delta** is identified, bounded (only `NotFound`/`InvalidInput`/`Conflict` body text, statuses unchanged), and each task verifies no test breaks rather than assuming.
