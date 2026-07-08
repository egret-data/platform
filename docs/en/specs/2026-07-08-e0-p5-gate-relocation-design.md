# E0 · P5 — Authorization gate relocation (design)

> **Status:** Design (approved to plan) · **Date:** 2026-07-08 · **Epic:** E0 Foundation of the [service-ready architecture](../architecture/2026-07-08-service-ready-architecture-design.md) (§4.1).
> **One-line goal:** Move the platform authorization gate out of the `egret-controlplane` god-crate into `egret-auth`, so every future seam (E1–E5) and Effort B can gate requests **without depending on `egret-controlplane`** — with zero behavior change and zero churn at the ~90 existing call sites.

---

## 1. Why

`egret-controlplane` is the god-crate the seam extraction (E1–E5) must dismantle. Its two gate helpers — `require_permission` / `require_platform_admin` (`crates/egret-controlplane/src/auth.rs`) — are a **foundation concern** that every seam needs, but they currently live inside the crate we're trying to break apart. Any domain module we extract to a seam would have to keep depending on `egret-controlplane` just to gate. Relocating the gate to `egret-auth` (the natural home — it already owns sessions, `Principal`, users/groups/roles) removes that reverse dependency and unblocks E1–E5.

This is **E0 §4.1's gate move only**. It is a *relocation*, not a redesign — redesigning the authorization model (unifying the two authorizers, object-level scoping, per-user delegation) is **Effort B**, which plugs in here later.

## 2. Current state (verified)

`crates/egret-controlplane/src/auth.rs`:
```rust
pub fn require_platform_admin(principal: &Principal) -> Result<(), CpError> {
    if principal.platform_roles.iter().any(|r| r == "platform_admin") { Ok(()) }
    else { Err(CpError::Forbidden) }
}
pub fn require_permission(principal: &Principal, permission: &str) -> Result<(), CpError> {
    if principal.platform_roles.iter().any(|r| r == "platform_admin")
        || principal.permissions.iter().any(|p| p == permission) { Ok(()) }
    else { Err(CpError::Forbidden) }
}
```
- Used at ~90 call sites across `access/`, `storage.rs`, `secrets.rs`, `engines.rs`, `warehouses.rs`, `policies/` — all as `require_*(&p, "key")?` inside handlers returning `Result<_, CpError>`.
- Dependency facts: `egret-controlplane` already depends on `egret-auth`; `egret-auth` already depends on `egret-core` (which owns `Principal` and the `rbac` permission catalog). So the target home is dependency-clean and acyclic.
- `crates/egret-controlplane/src/error.rs` already has `impl From<egret_core::Error> for CpError` mapping `Unauthorized | Forbidden → CpError::Forbidden`.

## 3. Design

### 3.1 New home — `egret-auth::authz`
A new module `crates/egret-auth/src/authz.rs` with the two helpers, returning the canonical error instead of a control-plane-specific one:

```rust
use egret_core::{Error, Principal};

const PLATFORM_ADMIN: &str = "platform_admin";

/// Gate on a specific permission key. `platform_admin` always passes (superuser).
pub fn require_permission(principal: &Principal, permission: &str) -> Result<(), Error> {
    if principal.platform_roles.iter().any(|r| r == PLATFORM_ADMIN)
        || principal.permissions.iter().any(|p| p == permission)
    { Ok(()) } else { Err(Error::Forbidden) }
}

/// Hard `platform_admin`-only gate (no bypass — it *is* the admin check).
pub fn require_platform_admin(principal: &Principal) -> Result<(), Error> {
    if principal.platform_roles.iter().any(|r| r == PLATFORM_ADMIN) { Ok(()) }
    else { Err(Error::Forbidden) }
}
```
Re-exported at the crate root: `pub use authz::{require_permission, require_platform_admin};` so callers write `egret_auth::require_permission(...)`. The existing unit tests move with it (behavior is unchanged).

**Error-type choice:** return `egret_core::Error::Forbidden`, not a bespoke `AuthzError`. It already maps to 403 via the shared `IntoResponse` (P1), any crate can `?` it, and it composes with existing `From<egret_core::Error>` impls. A dedicated error would add a type every seam must learn for no gain.

### 3.2 Zero-churn at `egret-controlplane`
Keep `crate::auth::require_permission` / `require_platform_admin` with their **exact current signature** `Result<(), CpError>`, re-implemented as thin delegators:

```rust
// crates/egret-controlplane/src/auth.rs  (now a thin adapter)
use egret_core::Principal;
use crate::error::CpError;

pub fn require_permission(p: &Principal, permission: &str) -> Result<(), CpError> {
    egret_auth::require_permission(p, permission).map_err(CpError::from)
}
pub fn require_platform_admin(p: &Principal) -> Result<(), CpError> {
    egret_auth::require_platform_admin(p).map_err(CpError::from)
}
```
`CpError::from(Error::Forbidden) == CpError::Forbidden` (existing impl) → still 403. **The ~90 call sites do not change at all.** The control-plane tests that assert 403 on a missing permission stay green.

### 3.3 Future seams
E1–E5 seam crates call `egret_auth::require_permission(&principal, "engine:create")?` directly, receiving `egret_core::Error` (→ 403 via the shared mapping), with **no dependency on `egret-controlplane`**.

## 4. Scope boundaries (explicit)

- **IN:** relocate the two platform-gate helpers to `egret-auth::authz`; make `egret-controlplane`'s versions thin delegators; move the unit tests.
- **OUT — aggregator thinning:** `egret-controlplane` only becomes a genuinely thin aggregator as domains leave for seams; that happens incrementally in **E1–E5** (each extraction removes a module + a `CpState` field). P5 does **not** restructure the router assembly or dismantle `CpState` — that would be speculative pre-work before any domain has moved.
- **OUT — `egret-workspace`:** its separate authorizer (`require_workspace` / `require_platform` in `egret-workspace/src/rbac.rs`) is untouched. Unifying the two authorizers is **Effort B**.
- **OUT — behavior/model change:** no permission-catalog change, no new gating, no status change.

## 5. Verification

- **Unit:** the moved tests in `egret-auth::authz` (admin passes, non-admin forbidden, permission held/missing, platform_admin bypass) — same assertions as today, now asserting `Err(egret_core::Error::Forbidden)`.
- **Regression:** `cargo test -p egret-controlplane` stays green (the delegators preserve `Result<(), CpError>` and the 403 mapping).
- **No-reverse-dep proof:** after the move, `grep` confirms no seam-candidate module needs `egret-controlplane` for gating; `egret-auth` gains no new dependency (it already has `egret-core`).
- **Gate:** `cargo fmt --all --check`, `cargo clippy -p egret-auth -p egret-controlplane -- -D warnings`, `cargo build --workspace`.

## 6. Decisions locked

1. Gate lives in **`egret-auth::authz`**, returns **`egret_core::Error::Forbidden`** (not a bespoke error).
2. `egret-controlplane` keeps **thin delegators** with the old `Result<(), CpError>` signature → **zero call-site churn**, identical 403 behavior.
3. **P5 = gate move only.** Aggregator thinning is realized incrementally in E1–E5; `egret-workspace`'s authorizer and the authz-model redesign are out of scope (Effort B).

---

*Design for E0·P5. Next: a bite-sized implementation plan, then subagent-driven execution.*
