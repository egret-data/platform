# E0 · P5 — Authorization gate relocation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the platform authorization gate (`require_permission` / `require_platform_admin`) out of `egret-controlplane` into `egret-auth::authz`, so any seam can gate without depending on the control-plane god-crate — with zero behavior change and zero call-site churn.

**Architecture:** Add `egret-auth::authz` with the two helpers returning `egret_core::Error::Forbidden` on denial; re-export at the crate root. `egret-controlplane::auth` becomes thin delegators that preserve the existing `Result<(), CpError>` signature via the already-present `From<egret_core::Error> for CpError` (`Forbidden → CpError::Forbidden → 403`), so the ~90 call sites are untouched. Design: [`docs/en/specs/2026-07-08-e0-p5-gate-relocation-design.md`](../specs/2026-07-08-e0-p5-gate-relocation-design.md).

**Tech Stack:** Rust 2021, workspace under `crates/`. No new dependencies (`egret-auth` already depends on `egret-core`; `egret-controlplane` already depends on `egret-auth`).

**Scope (from the design):** gate move only. Out of scope: `CpState`/router-assembly thinning (E1–E5), `egret-workspace`'s authorizer, and any authz-model change (Effort B).

---

## File structure

- `crates/egret-auth/src/authz.rs` — **new**; the two gate helpers + their unit tests. One responsibility: platform-permission checks over a `Principal`.
- `crates/egret-auth/src/lib.rs` — wire `pub mod authz;` + re-export the two fns at the crate root.
- `crates/egret-controlplane/src/auth.rs` — **rewrite** as thin delegators to `egret_auth`, preserving the `Result<(), CpError>` signature; keep a small delegation test.

---

## Task 1: Add `egret-auth::authz` (the new home)

**Files:**
- Create: `crates/egret-auth/src/authz.rs`
- Modify: `crates/egret-auth/src/lib.rs`

- [ ] **Step 1: Write the module with failing (stubbed) implementations + tests**

Create `crates/egret-auth/src/authz.rs`:

```rust
//! Platform authorization gate — permission / platform-admin checks over a
//! [`Principal`]. Returns [`egret_core::Error::Forbidden`] on denial (maps to
//! 403 via the shared HTTP mapping). Callable from any crate, so seams can gate
//! without depending on `egret-controlplane`.

use egret_core::{Error, Principal};

/// The platform role that bypasses permission checks (superuser).
const PLATFORM_ADMIN: &str = "platform_admin";

/// Gate an action on a specific permission key. `platform_admin` always passes;
/// others must hold the key. `Err(Error::Forbidden)` on denial.
pub fn require_permission(_principal: &Principal, _permission: &str) -> Result<(), Error> {
    todo!()
}

/// Hard `platform_admin`-only gate (no bypass — it *is* the admin check).
pub fn require_platform_admin(_principal: &Principal) -> Result<(), Error> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_core::ids::UserId;

    fn principal(roles: &[&str], perms: &[&str]) -> Principal {
        Principal {
            user_id: UserId::new(),
            email: "a@b.c".into(),
            platform_roles: roles.iter().map(|s| s.to_string()).collect(),
            permissions: perms.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn admin_passes_platform_admin_gate() {
        assert!(require_platform_admin(&principal(&["platform_admin"], &[])).is_ok());
    }

    #[test]
    fn non_admin_forbidden_on_platform_admin_gate() {
        assert!(matches!(
            require_platform_admin(&principal(&["data_steward"], &[])),
            Err(Error::Forbidden)
        ));
        assert!(matches!(
            require_platform_admin(&principal(&[], &[])),
            Err(Error::Forbidden)
        ));
    }

    #[test]
    fn permission_allows_when_held() {
        assert!(require_permission(&principal(&[], &["engine:create"]), "engine:create").is_ok());
    }

    #[test]
    fn permission_forbidden_when_missing() {
        assert!(matches!(
            require_permission(&principal(&[], &["engine:read"]), "engine:create"),
            Err(Error::Forbidden)
        ));
    }

    #[test]
    fn platform_admin_bypasses_any_permission() {
        let p = principal(&["platform_admin"], &[]);
        assert!(require_permission(&p, "engine:create").is_ok());
        assert!(require_permission(&p, "role:delete").is_ok());
    }
}
```

In `crates/egret-auth/src/lib.rs`, add `pub mod authz;` as the first `pub mod` line (keep the rest), and add the re-export after the module list:

```rust
pub mod authz;
```
```rust
pub use authz::{require_permission, require_platform_admin};
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p egret-auth authz`
Expected: FAIL — the 5 tests panic on `todo!()` ("not yet implemented").

- [ ] **Step 3: Implement the two helpers**

Replace both function bodies in `authz.rs`:

```rust
pub fn require_permission(principal: &Principal, permission: &str) -> Result<(), Error> {
    if principal.platform_roles.iter().any(|r| r == PLATFORM_ADMIN)
        || principal.permissions.iter().any(|p| p == permission)
    {
        Ok(())
    } else {
        Err(Error::Forbidden)
    }
}

pub fn require_platform_admin(principal: &Principal) -> Result<(), Error> {
    if principal.platform_roles.iter().any(|r| r == PLATFORM_ADMIN) {
        Ok(())
    } else {
        Err(Error::Forbidden)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p egret-auth authz`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/authz.rs crates/egret-auth/src/lib.rs
git commit -m "feat(auth): add authz gate (require_permission/require_platform_admin)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Make `egret-controlplane::auth` a thin delegator

**Files:**
- Modify (rewrite): `crates/egret-controlplane/src/auth.rs`

- [ ] **Step 1: Rewrite `auth.rs` as delegators (+ delegation tests)**

Overwrite `crates/egret-controlplane/src/auth.rs` with:

```rust
//! Platform-admin guard for control-plane routes.
//!
//! The gate logic now lives in [`egret_auth::authz`] so any crate can gate
//! without depending on `egret-controlplane`. These are thin adapters that
//! preserve the `Result<(), CpError>` signature the call sites expect —
//! `egret_core::Error::Forbidden` maps to `CpError::Forbidden` (→ 403) via the
//! existing `From<egret_core::Error>` impl in `crate::error`.

use egret_core::Principal;

use crate::error::CpError;

/// Reject any caller lacking the `platform_admin` role. Delegates to `egret-auth`.
pub fn require_platform_admin(principal: &Principal) -> Result<(), CpError> {
    egret_auth::require_platform_admin(principal).map_err(CpError::from)
}

/// Gate a control-plane action on a permission key. `platform_admin` always
/// passes. Delegates to `egret-auth`.
pub fn require_permission(principal: &Principal, permission: &str) -> Result<(), CpError> {
    egret_auth::require_permission(principal, permission).map_err(CpError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_core::ids::UserId;

    fn principal(roles: &[&str], perms: &[&str]) -> Principal {
        Principal {
            user_id: UserId::new(),
            email: "a@b.c".into(),
            platform_roles: roles.iter().map(|s| s.to_string()).collect(),
            permissions: perms.iter().map(|s| s.to_string()).collect(),
        }
    }

    // The delegators must preserve the 403 mapping: a denied gate → CpError::Forbidden.
    #[test]
    fn delegator_maps_denial_to_cp_forbidden() {
        assert!(matches!(
            require_platform_admin(&principal(&[], &[])),
            Err(CpError::Forbidden)
        ));
        assert!(matches!(
            require_permission(&principal(&[], &["engine:read"]), "engine:create"),
            Err(CpError::Forbidden)
        ));
    }

    #[test]
    fn delegator_allows_when_authorized() {
        assert!(require_platform_admin(&principal(&["platform_admin"], &[])).is_ok());
        assert!(require_permission(&principal(&[], &["engine:create"]), "engine:create").is_ok());
    }
}
```

- [ ] **Step 2: Build (call sites unchanged)**

Run: `cargo build -p egret-controlplane`
Expected: clean. The ~90 `require_*(&p, "key")?` call sites compile unchanged (same signature). If any call site *pattern-matched* the old direct `CpError::Forbidden` return in a way that now breaks, STOP and report — none is expected (the return type is identical).

- [ ] **Step 3: Run the control-plane tests**

Run: `cargo test -p egret-controlplane`
Expected: PASS — the new delegation tests plus all existing handler tests (403-on-missing-permission behavior is preserved by the `CpError::from` mapping).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-controlplane/src/auth.rs
git commit -m "refactor(controlplane): delegate auth gate to egret_auth::authz

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Workspace gate + no-reverse-dep proof

**Files:** none (verification only).

- [ ] **Step 1: Format check**

Run: `cargo fmt --all --check`
Expected: no diff (else `cargo fmt --all` + commit `style: cargo fmt`).

- [ ] **Step 2: Clippy the two crates**

Run: `cargo clippy -p egret-auth -p egret-controlplane -- -D warnings`
Expected: no warnings (ignore any pre-existing vendored `third_party/mysql_async` lint).

- [ ] **Step 3: Workspace build + tests for the two crates**

Run: `cargo build --workspace`
Expected: clean.
Run: `cargo test -p egret-auth -p egret-controlplane`
Expected: PASS.

- [ ] **Step 4: No-reverse-dep proof**

The whole point is that gating no longer requires `egret-controlplane`. Confirm:
- `grep -n "egret-controlplane" crates/egret-auth/Cargo.toml` → **no output** (egret-auth must not depend on the control-plane crate).
- `grep -n "egret-core" crates/egret-auth/Cargo.toml` → present (the gate's only dependency, already there — no new dep added).
Report both results.

- [ ] **Step 5: Final commit (only if fmt changed anything)**

```bash
git add -A
git commit -m "style: cargo fmt after gate relocation" || echo "nothing to format"
```

---

## Self-review notes

- **Spec coverage:** §3.1 (new `egret-auth::authz` returning `egret_core::Error::Forbidden` + re-export) → Task 1; §3.2 (thin delegators, zero churn, preserved 403 mapping) → Task 2; §5 verification (unit + regression + no-reverse-dep + gate) → Task 3. §4 scope-outs (aggregator/`CpState`, `egret-workspace`, model change) are not touched by any task — correct.
- **Placeholder scan:** the only `todo!()` is the deliberate red-test stub in Task 1 Step 1, replaced in Step 3.
- **Type consistency:** `require_permission(&Principal, &str) -> Result<(), egret_core::Error>` and `require_platform_admin(&Principal) -> Result<(), egret_core::Error>` in `egret-auth`; the `egret-controlplane` delegators keep `-> Result<(), CpError>` via `.map_err(CpError::from)`. `Principal` test helper fields (`user_id`, `email`, `platform_roles`, `permissions`) match the struct used in the pre-existing control-plane tests.
