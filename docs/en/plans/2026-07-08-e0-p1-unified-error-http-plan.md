# E0 Foundation · Plan 1 — Unified error→HTTP mapping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the `egret_core::Error → HTTP` mapping (currently copy-pasted in 7 crates) into `egret-core` once, expose a generic `ApiError<D>`, and migrate `egret-bi` onto it as the reference consumer.

**Architecture:** Add a feature-gated `http` module to the leaf crate `egret-core` that maps the canonical `Error` to a status + JSON body (`impl IntoResponse`), plus a generic `ApiError<D> { Auth(Error), Domain(D) }` whose `Auth` arm delegates to that shared mapping and whose `Domain` arm delegates to the seam's own `D: IntoResponse`. Each seam then keeps only its **domain** mapping. This is §4.2 of the [service-ready architecture design](../architecture/2026-07-08-service-ready-architecture-design.md).

**Tech Stack:** Rust 2021, axum 0.8, thiserror 2, serde_json 1. Cargo workspace under `crates/`. `just` for CI (`cargo fmt --all --check`, `cargo clippy`, `cargo test`).

**Where this sits:** This is **Plan 1 of the E0 (Foundation) epic**. Sibling E0 plans (write after this one lands): **P2** shared token primitive in `egret-core`; **P3** the `egret-events` in-proc bus crate; **P4** OpenAPI/utoipa emit + FE api-client codegen; **P5** move the auth gate into `egret-auth` + stand up `egret-controlplane` as a thin aggregator; **P6** revise the architecture doc to v0.3. Plan 1 is a prerequisite for P5 (the aggregator's error type) and unblocks the other 6 consumers' migration (a mechanical follow-up, one task per crate, done after this reference migration proves the pattern).

---

## File structure

- `crates/egret-core/Cargo.toml` — add optional `axum` + `serde_json` deps behind a new `http` feature.
- `crates/egret-core/src/http.rs` — **new**; the shared `Error` HTTP mapping + `ApiError<D>`. One responsibility: turning core errors into HTTP responses.
- `crates/egret-core/src/lib.rs` — wire the feature-gated module + re-export `ApiError`.
- `crates/egret-bi/Cargo.toml` — enable the `egret-core` `http` feature.
- `crates/egret-bi/src/error.rs` — replace the bespoke `BiApiError` enum (which duplicates the core-`Error` mapping) with `ApiError<BiError>` + a domain-only `IntoResponse for BiError`.

---

## Task 1: Add the `http` feature to `egret-core`

**Files:**
- Modify: `crates/egret-core/Cargo.toml`

- [ ] **Step 1: Add optional deps + feature**

Replace the `[dependencies]` block and add a `[features]` block so the file reads:

```toml
[package]
name = "egret-core"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
thiserror.workspace = true
anyhow.workspace = true
serde.workspace = true
uuid.workspace = true
axum = { workspace = true, optional = true }
serde_json = { workspace = true, optional = true }

[features]
# Opt-in HTTP mapping (Error -> Response, ApiError<D>). Pure-domain consumers
# of egret-core do not pull axum unless they enable this.
http = ["dep:axum", "dep:serde_json"]

[dev-dependencies]
serde_json.workspace = true
axum.workspace = true
```

- [ ] **Step 2: Verify it still builds without the feature**

Run: `cargo build -p egret-core`
Expected: builds clean (axum/serde_json not compiled — feature off by default).

- [ ] **Step 3: Commit**

```bash
git add crates/egret-core/Cargo.toml
git commit -m "chore(core): add opt-in http feature to egret-core"
```

---

## Task 2: `Error::status_code()` — the pure mapping

**Files:**
- Create: `crates/egret-core/src/http.rs`
- Modify: `crates/egret-core/src/lib.rs`
- Test: `crates/egret-core/src/http.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Create `crates/egret-core/src/http.rs` with only the test + a stub:

```rust
//! HTTP mapping for the canonical `egret_core::Error` (feature = "http").
//!
//! This is the single source of truth for how core errors become HTTP
//! responses. Previously this match arm was copy-pasted in 7 crates.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::Error;

impl Error {
    /// The HTTP status this error maps to.
    pub fn status_code(&self) -> StatusCode {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_are_stable() {
        assert_eq!(Error::NotFound("x".into()).status_code(), StatusCode::NOT_FOUND);
        assert_eq!(Error::InvalidInput("x".into()).status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(Error::Unauthorized.status_code(), StatusCode::UNAUTHORIZED);
        assert_eq!(Error::Forbidden.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(Error::Conflict("x".into()).status_code(), StatusCode::CONFLICT);
        assert_eq!(Error::TooManyRequests.status_code(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            Error::Internal(anyhow::anyhow!("boom")).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
```

Then wire the module in `crates/egret-core/src/lib.rs` — add after the existing `pub mod` lines:

```rust
#[cfg(feature = "http")]
pub mod http;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p egret-core --features http status_codes_are_stable`
Expected: FAIL — panics on `todo!()` (`not yet implemented`).

- [ ] **Step 3: Implement the mapping**

Replace the `status_code` body:

```rust
    pub fn status_code(&self) -> StatusCode {
        match self {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::InvalidInput(_) => StatusCode::BAD_REQUEST,
            Error::Unauthorized => StatusCode::UNAUTHORIZED,
            Error::Forbidden => StatusCode::FORBIDDEN,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p egret-core --features http status_codes_are_stable`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-core/src/http.rs crates/egret-core/src/lib.rs
git commit -m "feat(core): Error::status_code() shared HTTP mapping"
```

---

## Task 3: `impl IntoResponse for Error`

**Files:**
- Modify: `crates/egret-core/src/http.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/egret-core/src/http.rs`:

```rust
    #[test]
    fn error_into_response_carries_status() {
        let resp = Error::Forbidden.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let resp = Error::NotFound("user".into()).into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p egret-core --features http error_into_response_carries_status`
Expected: FAIL to compile — `Error` does not implement `IntoResponse`.

- [ ] **Step 3: Implement `IntoResponse`**

Add below the `impl Error` block in `http.rs`. Internal errors must not leak details in the body:

```rust
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status_code();
        // Public-safe message: expose the specific message for client-facing
        // errors; hide internals for 5xx.
        let msg = match &self {
            Error::Internal(_) => "internal error".to_string(),
            Error::Unauthorized => "unauthorized".to_string(),
            Error::Forbidden => "forbidden".to_string(),
            Error::TooManyRequests => "too many requests".to_string(),
            other => other.to_string(),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p egret-core --features http error_into_response_carries_status`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-core/src/http.rs
git commit -m "feat(core): impl IntoResponse for Error"
```

---

## Task 4: Generic `ApiError<D>`

**Files:**
- Modify: `crates/egret-core/src/http.rs`
- Modify: `crates/egret-core/src/lib.rs` (re-export)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `http.rs`. `StatusCode` implements `IntoResponse`, so it stands in for a domain error `D`:

```rust
    #[test]
    fn api_error_auth_arm_uses_core_mapping() {
        let e: ApiError<StatusCode> = Error::Forbidden.into();
        assert_eq!(e.into_response().status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn api_error_domain_arm_delegates_to_d() {
        let e: ApiError<StatusCode> = ApiError::Domain(StatusCode::IM_A_TEAPOT);
        assert_eq!(e.into_response().status(), StatusCode::IM_A_TEAPOT);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p egret-core --features http api_error`
Expected: FAIL to compile — `ApiError` undefined.

- [ ] **Step 3: Implement `ApiError<D>`**

Add to `http.rs`:

```rust
/// The uniform API-error envelope every seam returns from its handlers.
///
/// `Auth` covers authn/authz/validation via the shared core mapping; `Domain`
/// is the seam's own error, which supplies its own `IntoResponse`. A handler
/// returns `Result<T, ApiError<MyDomainError>>` and `?` works on both a
/// `core::Error` and `MyDomainError` (given the seam adds `From<MyDomainError>`).
pub enum ApiError<D> {
    Auth(Error),
    Domain(D),
}

impl<D> From<Error> for ApiError<D> {
    fn from(e: Error) -> Self {
        ApiError::Auth(e)
    }
}

impl<D: IntoResponse> IntoResponse for ApiError<D> {
    fn into_response(self) -> Response {
        match self {
            ApiError::Auth(e) => e.into_response(),
            ApiError::Domain(d) => d.into_response(),
        }
    }
}
```

Re-export from `crates/egret-core/src/lib.rs` — add after the existing `pub use` lines:

```rust
#[cfg(feature = "http")]
pub use http::ApiError;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p egret-core --features http api_error`
Expected: PASS (both tests).

- [ ] **Step 5: Run the whole core crate both ways**

Run: `cargo test -p egret-core && cargo test -p egret-core --features http`
Expected: PASS in both configurations (feature-off still compiles and passes).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-core/src/http.rs crates/egret-core/src/lib.rs
git commit -m "feat(core): generic ApiError<D> envelope"
```

---

## Task 5: Migrate `egret-bi` onto `ApiError<BiError>` (reference consumer)

This deletes egret-bi's copy of the core-`Error` match arm — the actual dedup win — while keeping its domain mapping.

**Files:**
- Modify: `crates/egret-bi/Cargo.toml`
- Modify: `crates/egret-bi/src/error.rs`

- [ ] **Step 1: Enable the `http` feature on the core dep**

In `crates/egret-bi/Cargo.toml`, change the `egret-core` dependency line to:

```toml
egret-core = { workspace = true, features = ["http"] }
```

- [ ] **Step 2: Replace `error.rs` with the domain-only mapping**

Overwrite `crates/egret-bi/src/error.rs` with:

```rust
//! BI domain error + its HTTP mapping. The auth/validation arm now lives in
//! `egret_core::ApiError` (shared); this file keeps only the BI-domain arm.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum BiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden")]
    Forbidden,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("engine: {0}")]
    Engine(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<egret_database::DatabaseError> for BiError {
    fn from(e: egret_database::DatabaseError) -> Self {
        match e {
            egret_database::DatabaseError::NotFound(s) => BiError::NotFound(s),
            other => BiError::Engine(other.to_string()),
        }
    }
}
impl From<egret_engine::EngineError> for BiError {
    fn from(e: egret_engine::EngineError) -> Self {
        match e {
            egret_engine::EngineError::NotFound(s) => BiError::NotFound(s),
            other => BiError::Engine(other.to_string()),
        }
    }
}

/// Domain arm only — the shared `Auth` arm is in `egret_core::ApiError`.
impl IntoResponse for BiError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            BiError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            BiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            BiError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            BiError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            BiError::Engine(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            BiError::Db(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

/// The BI handler error type. `From<egret_core::Error>` comes from core;
/// this adds the domain-side `From` so `?` works on `BiError` too.
pub type BiApiError = ApiError<BiError>;

impl From<BiError> for BiApiError {
    fn from(e: BiError) -> Self {
        ApiError::Domain(e)
    }
}
```

> Note: existing call sites that write `BiApiError::Auth(..)` / `BiApiError::Domain(..)` / `.into()` keep compiling — `BiApiError` is now a type alias for `ApiError<BiError>`, and the two `From` impls (core's `From<Error>`, this file's `From<BiError>`) cover both `?` paths.

- [ ] **Step 3: Compile egret-bi**

Run: `cargo build -p egret-bi`
Expected: builds clean. If a handler used a `BiApiError` *pattern match* (rare) rather than construction, update it to match `ApiError::Auth(_) | ApiError::Domain(_)`; the audit found only construction sites, so none expected.

- [ ] **Step 4: Run the BI test suite**

Run: `cargo test -p egret-bi`
Expected: PASS — behavior is unchanged (same statuses, same JSON `{ "error": ... }` body).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-bi/Cargo.toml crates/egret-bi/src/error.rs
git commit -m "refactor(bi): use egret_core::ApiError, drop duplicated core-error mapping"
```

---

## Task 6: Workspace gate (fmt + clippy + build)

**Files:** none (verification only).

- [ ] **Step 1: Format check**

Run: `cargo fmt --all --check`
Expected: no diff. (If it reports files, run `cargo fmt --all` and re-run; commit any formatting with `style: cargo fmt`.)

- [ ] **Step 2: Clippy the two touched crates**

Run: `cargo clippy -p egret-core --features http -p egret-bi -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Workspace build**

Run: `cargo build --workspace`
Expected: builds clean — no other crate references egret-bi's old `BiApiError` internals.

- [ ] **Step 4: Final commit (if fmt changed anything)**

```bash
git add -A
git commit -m "style: cargo fmt after ApiError migration" || echo "nothing to format"
```

---

## Follow-up (separate plans, not in scope here)

- **E0-P1b — migrate the remaining 6 consumers** onto `ApiError<D>`: `egret-auth/users.rs`, `egret-controlplane/error.rs`, `egret-database/error.rs`, `egret-engine/error.rs`, `egret-kernel/routes.rs`, `egret-notebook/store.rs`. Each is the same mechanical change as Task 5 (enable `http` feature, keep the domain arm, delete the copied core-`Error` arm, alias to `ApiError<XxxError>`). One task per crate; write after reading each file's current error module. Also give **`egret-catalog-engine`** a real domain error + `IntoResponse` (it is `anyhow`-only today).
- **E0-P2..P6** as listed in the header.

---

## Self-review notes

- **Spec coverage:** implements design §4.2 (shared `Error→HTTP` in core + generic `ApiError<D>`, seam supplies only its Domain arm). The "give catalog-engine a domain error" and "migrate the other 6 copies" parts of §4.2 are explicitly deferred to E0-P1b (they need per-file reads).
- **Placeholder scan:** the only `todo!()` is the deliberate red-test stub in Task 2 Step 1, removed in Step 3.
- **Type consistency:** `status_code() -> StatusCode`, `ApiError<D>{Auth(Error),Domain(D)}`, `impl<D> From<Error>`, `impl<D: IntoResponse> IntoResponse`, `BiApiError = ApiError<BiError>` — names/signatures consistent across Tasks 2–5.
