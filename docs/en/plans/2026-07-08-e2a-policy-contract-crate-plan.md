# E2a — `egret-policy-api` contract crate + `PolicyPort` (plan)

> **Status:** Plan · **Date:** 2026-07-08 · **Part:** E2a of [E2 decomposition](./2026-07-08-e2-policy-seam-decomposition.md) · **Precedent:** E1a (#179 — leaf contract-crate seed) · **Branch:** `feat/e2a-policy-contract-crate`

## Goal
Seed the Policy/Ranger seam's **contract**: a leaf crate `egret-policy-api` holding the pure-serde wire DTOs + a `PolicyPort` trait, then prove it by routing the seam's one durable cross-seam consumer — **engine-mgmt auto-bind** — through the Port. No schema move, no router move, no crate merge (those are E2b–E2d). Low-risk, self-contained.

## Why the Port is exactly three methods
The only code that reaches into the policy store from **outside** the seam is `egret-engine-mgmt`'s engine-create auto-bind (`engines.rs:1403-1438`). It calls exactly:
- `egret_authz::seed_trino(&store)` — ensure the `trino` service-def exists
- `store.list_services()` — validate an admin-chosen `authz_service_id` exists (reads `.id`)
- `store.create_service(name, "trino", {})` — mint a fresh profile for the engine (reads `.id`)

`set_engine_authz_service` writes `engine_mgmt.engines` — engine-mgmt's **own** table, not policy — so it stays out of the Port. controlplane's `policies/` module also uses `AuthzStore` today, but it **moves into the seam in E2d**, so wiring it through the Port now would be churn we immediately undo → not in E2a.

## Deliverables

### 1. New leaf crate `crates/egret-policy-api`
`Cargo.toml`: `serde` (derive), `serde_json`, `async-trait`, `thiserror`. **No sqlx, no axum, no egret-* deps** (leaf, like `egret-engine-mgmt-api`; auto-included via `members = ["crates/*"]`). **No `utoipa` yet** — `utoipa` is unused workspace-wide today; E1a's contract crate is deliberately serde-only, and OpenAPI/`ToSchema` derive is deferred to E0·P4 (the first seam that emits an OpenAPI doc). Adding it here would be an unused heavy dep (YAGNI). The DTOs derive `serde` only; the `ToSchema` derive is a one-line add when P4 lands.

`src/lib.rs`:
```rust
//! Policy/Ranger seam contract: wire DTOs + PolicyPort. Leaf crate (serde-only).
pub mod dto;
pub mod port;
pub use dto::{PolicyService, ServicePolicies, ServiceTags};
pub use port::{PolicyError, PolicyPort};
```

`src/dto.rs`:
- **Move** `ServicePolicies` + `ServiceTags` verbatim from `egret-ranger/src/dto.rs` (already pure `serde`, same derives). These are the engine-facing download wire contract.
- **New** `PolicyService { id: i64, name: String, r#type: String }` — the Port's return DTO (plain serde). Minimal projection of `authz_services` (the fields any cross-seam caller may need; engine-mgmt uses `.id`).

`src/port.rs`:
```rust
#[async_trait::async_trait]
pub trait PolicyPort: Send + Sync {
    /// Ensure the built-in `trino` service-def is seeded (idempotent).
    async fn ensure_trino_def(&self) -> Result<(), PolicyError>;
    /// All policy profiles (services).
    async fn list_services(&self) -> Result<Vec<PolicyService>, PolicyError>;
    /// Create a new policy profile; errors `AlreadyExists` on name clash.
    async fn create_service(&self, name: &str, type_: &str, config: &serde_json::Value)
        -> Result<PolicyService, PolicyError>;
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("policy resource not found: {0}")] NotFound(String),
    #[error("policy resource already exists: {0}")] AlreadyExists(String),
    #[error("policy store error: {0}")] Backend(String),
}
```
`PolicyError` is a transport-neutral error (no sqlx leak). `AuthzError` maps into it in the impl.

### 2. Implement `PolicyPort` for `AuthzStore` (in `egret-authz`)
- Add `egret-policy-api` dep to `egret-authz/Cargo.toml`.
- New `egret-authz/src/port_impl.rs`: `impl PolicyPort for AuthzStore` delegating to existing methods; convert `ServiceRow → PolicyService` (`id/name/r#type`) and `AuthzError → PolicyError` (`NotFound→NotFound`, `AlreadyExists→AlreadyExists`, else `Backend(e.to_string())`). `ensure_trino_def` calls the existing `service_def::seed_trino(self)`.
- Keep the concrete `ServiceRow`/`create_service`/`list_services` as-is (in-seam callers + ranger still use them directly). `PolicyService` is the *boundary* type only.

### 3. Re-export moved DTOs from `egret-ranger` (no behavior change)
`egret-ranger/src/dto.rs` → `pub use egret_policy_api::{ServicePolicies, ServiceTags};` (add the leaf dep). Ranger's `download.rs`/`crud.rs` construction sites unchanged (same type, same fields). Keeps E2a a pure move — the crate merge is E2b.

### 4. Decouple engine-mgmt onto `PolicyPort` (`Arc<dyn PolicyPort>`)
engine-mgmt uses the policy store in exactly one place (engine-create auto-bind) and touches only these three methods — so it can depend on the *interface*, not the concrete store. **As-built (went beyond the original "keep concrete" note — clean and low-risk since the coupling is so narrow):**
- `EngineMgmtState.authz` → `Arc<dyn egret_policy_api::PolicyPort>` (was concrete `AuthzStore`). Method calls on the trait object need no `use` import.
- auto-bind: `st.authz.ensure_trino_def()` (replaces the `egret_authz::seed_trino` free fn); `st.authz.list_services()` yields `Vec<PolicyService>` (`.iter().any(|s| s.id == sid)` unchanged); `st.authz.create_service(&b.name, "trino", &json!({}))` → `PolicyService` (`.id` unchanged).
- **`egret-authz` dropped from engine-mgmt's normal deps → `dev-dependencies`** (only the two test-state constructions build a concrete `AuthzStore`; production wires it in egret-bin). engine-mgmt now depends on `egret-policy-api` (interface) only.
- Removed the now-dead `impl From<AuthzError> for EngineHttpError` (auto-bind swallows errors best-effort; no `?` ever produced an `AuthzError`).
- **egret-bin** composition wraps it: `authz: Arc::new(AuthzStore::new(db))` (unsized-coerces to `Arc<dyn PolicyPort>`). `CpState.authz` stays concrete `AuthzStore` (controlplane's policies module still uses inherent methods; it moves in E2d).

## TDD
1. **Contract test** in `egret-policy-api/tests/` — `ServicePolicies`/`ServiceTags` serde round-trip (camelCase, `NON_EMPTY`/skip-none) matches the pre-move wire bytes (pin a JSON vector, like E0·P2's sha256 vector proved format-preservation). Guards the DTO move.
2. **Port impl test** in `egret-authz/tests/` — against a live test DB: `ensure_trino_def` idempotent; `create_service` returns a `PolicyService` with the right `id/name/type`; duplicate name → `PolicyError::AlreadyExists`.
3. Existing `egret-ranger` `http_ranger.rs` / `ranger_engine_gate.rs` + `egret-authz` store tests must stay green (proves the re-export + Port impl didn't change behavior).

## Done when
- New crate compiles; `PolicyPort` impl'd for `AuthzStore`; DTOs moved + re-exported; engine-mgmt auto-bind goes through the trait.
- `cargo fmt --all --check`, `cargo clippy --all-targets`, `cargo test --workspace` green locally.
- FE untouched (no wire change) — but run FE lint/typecheck/build per project practice before merge.
- Squash-merge (user runs `--admin`).

## Explicitly deferred
- Crate merge / rename → E2b. Schema `SET SCHEMA policy` + drop `policy_audit` → E2c. Move `policies/` admin router + strip `CpState.authz` (route its `AuthzStore` use through the Port too) → E2d — engine-mgmt's `Arc<dyn PolicyPort>` decoupling already landed in E2a. Anon-surface fix → E2e. OpenAPI emit + FE codegen → E0·P4 follow-on. `policy.changed` event → first consumer.
