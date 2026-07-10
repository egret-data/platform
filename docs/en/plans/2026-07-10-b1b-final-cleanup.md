# B1b-final — Enforcement-unification cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fold the last inline admin check (`set_platform_roles`) into the admin-graph `forbid`, and delete the now-dead legacy gate primitives. After this, all authorization enforcement flows through the Cedar PDP `require()`.

**Architecture:** Add `user:assign_platform_role` to `ADMIN_GRAPH_ACTIONS` (auto-covered by the existing `forbid`); migrate `set_platform_roles` to `require()`; delete `egret_auth::{require_permission, require_platform_admin}`, the workspace `Authorizer` gate/permission methods, and the `egret_controlplane::auth` admin wrapper. KEEP `Principal.permissions` (feeds the FE `/me`), `Principal.platform_roles`, `Authorizer::{new, workspace_roles_for, replace_platform_roles}`, and the legacy tables/triggers (the data-model flip is B2).

**Tech Stack:** Rust, axum, `cedar-policy` (via the wired PDP).

**Reference:** [B1b-final design](../specs/2026-07-10-b1b-final-cleanup-design.md).

---

## Task 1: Fold `set_platform_roles` into the admin-graph `forbid`

**Files:**
- Modify: `crates/egret-core/src/rbac.rs`, `crates/egret-workspace/src/routes.rs`
- Test: `crates/egret-workspace/tests/workspace_db.rs`; extend `crates/egret-auth/tests/admin_graph_forbid.rs`

- [ ] **Step 1: Add the admin-graph action.** In `crates/egret-core/src/rbac.rs`, add `"user:assign_platform_role"` to `ADMIN_GRAPH_ACTIONS`:
```rust
pub static ADMIN_GRAPH_ACTIONS: &[&str] = &[
    "role:create", "role:update", "role:delete",
    "role:assign_permission", "user:assign_role", "group:manage_members",
    "user:assign_platform_role",
];
```
The `is_admin_graph`/non-grantable test (from B1b-2) automatically covers the new key (`assert!(!is_known(a))` for each). Run `rtk proxy cargo test -p egret-core rbac` → still passes (the new action is not in the grantable `PERMISSIONS`).

- [ ] **Step 2: Migrate the gate.** In `crates/egret-workspace/src/routes.rs`, `set_platform_roles` (~line 384) currently has:
```rust
if !p.platform_roles.iter().any(|r| r == "platform_admin") {
    return Err(AuthError(Error::Forbidden));
}
```
Replace that block with:
```rust
egret_core::authz::require(&*st.pdp, &p, "user:assign_platform_role")
    .await
    .map_err(AuthError)?;
```
(Match the handler's actual state var `st` and principal `p`; `WorkspaceState.pdp` exists from B1b-3. `require` returns `egret_core::Result<()>`; map its `Err` to `AuthError` as the surrounding code maps other errors — check the exact error-wrapping used by neighboring handlers.) Leave the `list_my_workspaces` `is_admin` filter (~line 253) untouched — it is a display filter, not a gate.

- [ ] **Step 3: Test.** Extend `crates/egret-workspace/tests/workspace_db.rs` (it already builds a real `CedarPdp` via `ws_state`, from B1b-3). Add: a `platform_admin` PUTs `/users/{id}/platform-roles` → 2xx; a non-`platform_admin` (seed one holding an unrelated platform role) → 403. Also optionally extend `crates/egret-auth/tests/admin_graph_forbid.rs`'s action loop to include `user:assign_platform_role` (it iterates `ADMIN_GRAPH_ACTIONS`, so it may already cover it automatically — verify it still passes).

- [ ] **Step 4: Run.** `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-workspace -p egret-auth -p egret-core -- --nocapture` → PASS (the new set_platform_roles test + admin_graph_forbid + rbac). Confirm they RAN.

- [ ] **Step 5: Commit.**
```bash
git add crates/egret-core/src/rbac.rs crates/egret-workspace/src/routes.rs crates/egret-workspace/tests/workspace_db.rs crates/egret-auth/tests/admin_graph_forbid.rs
git commit -m "feat(authz): fold set_platform_roles into the admin-graph forbid

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Delete the dead legacy gate primitives

**Files:**
- Delete/modify: `crates/egret-auth/src/authz.rs`, `crates/egret-auth/src/lib.rs`
- Modify: `crates/egret-workspace/src/rbac.rs`, `crates/egret-controlplane/src/auth.rs` (+ its `mod` decl)

- [ ] **Step 1: Confirm the removals are truly unreferenced.** Run each and expect NO handler/production hits (only the definitions + their own tests):
  - `rtk proxy grep -rn "require_permission(\|require_platform_admin(" crates --include=*.rs | grep -v "/tests/"`
  - `rtk proxy grep -rn "\.require_workspace(\|\.require_platform(\|\.workspace_permissions(\|\.platform_permissions(" crates --include=*.rs | grep "/src/"`
  If any NON-definition, non-test caller appears, STOP — it means a handler wasn't migrated; report it (do not delete a live primitive).

- [ ] **Step 2: Delete `egret_auth::authz`.** Confirm `crates/egret-auth/src/authz.rs` contains ONLY `require_permission`, `require_platform_admin`, and their `#[cfg(test)]` module (read it). If so, `/bin/rm -f crates/egret-auth/src/authz.rs` and remove `pub mod authz;` + the `pub use authz::{require_permission, require_platform_admin};` re-export from `crates/egret-auth/src/lib.rs`. If it contains anything else, remove only the two functions + their tests + the two re-exports.

- [ ] **Step 3: Remove the dead workspace `Authorizer` methods.** In `crates/egret-workspace/src/rbac.rs`, delete the methods `require_workspace`, `require_platform`, `workspace_permissions`, `platform_permissions` (and any `#[cfg(test)]` tests that exercise only those). KEEP `new`, `workspace_roles_for`, `replace_platform_roles`. Fix now-unused imports (e.g. `WorkspaceContext`, `Error` if only those methods used them) — the compiler/clippy will flag them.

- [ ] **Step 4: Remove the controlplane admin wrapper.** In `crates/egret-controlplane/src/auth.rs`, delete `require_platform_admin` + its tests. If the file is then empty (or holds only unused items), `/bin/rm -f` it and remove its `mod auth;` declaration + any `use crate::auth::require_platform_admin;`. If it still holds other live items, remove only the wrapper.

- [ ] **Step 5: Build + clippy prove nothing referenced them.** `rtk proxy cargo build --workspace` → SUCCESS. `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` → clean (fix any now-unused import the removals exposed). If the build fails because something DID reference a deleted item, that reference is a missed migration — migrate it (or, if it's a test, update it), do not re-add the dead primitive.

- [ ] **Step 6: Commit.**
```bash
git add -A
git commit -m "refactor(authz): delete the dead legacy gate primitives (enforcement now PDP-only)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
(Review `git status` first — the staged set should be only authz.rs deletion, lib.rs, rbac.rs, auth.rs, and any import fixups.)

---

## Task 3: Verify + full gate

- [ ] **Step 1: Verify enforcement is fully unified.** Confirm the only remaining references to the deleted symbols are gone and the KEPT ones are intact:
  - `rtk proxy grep -rn "require_permission\|require_platform_admin" crates --include=*.rs | grep "/src/"` → NO hits (all deleted).
  - `rtk proxy grep -rn "workspace_roles_for\|replace_platform_roles" crates/egret-workspace/src` → still present + called by `context.rs`/`routes.rs` (KEPT correctly).
  - `rtk proxy grep -rn "principal.permissions\|permissions: principal.permissions" crates/egret-auth/src/routes.rs` → the `/me` `permissions` field is still there (KEPT — FE hint).
  Report the outputs.

- [ ] **Step 2: Full local CI gate (sequential — never two workspace test runs at once):**
  - `rtk proxy cargo fmt --all --check` (fmt + restage if needed)
  - `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` → clean
  - `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test --workspace` → 0 failures
  - Frontend gate: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` → green. The `/me` `permissions` field is unchanged, so FE gating is unaffected — confirm the FE build/tests pass.

- [ ] **Step 3: Commit** (only if Step 1/2 required changes)
```bash
git add -u
git commit -m "test(authz): verify enforcement unified on the PDP + gate green (B1b-final)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Spec coverage:** §3 removals → Task 2; §4 `set_platform_roles` fold → Task 1; §2 "what stays" → enforced by Task 2 Step 3/4 (keep `workspace_roles_for`/`replace_platform_roles`) + Task 3 Step 1 (verify `Principal.permissions`/`/me` intact).
- **Behavior preservation:** `set_platform_roles` forbid-unless-admin ≡ the inline `platform_roles.contains("platform_admin")` check; the removals are pure dead-code deletion (Task 2 Step 1 proves no live caller).
- **Ordering:** Task 1 (fold) before Task 2 (delete) is safe; the deletions don't touch `set_platform_roles`. `require_permission` reads `Principal.permissions` but is deleted, and the field stays for `/me` — no conflict.
- **No placeholders:** exact grep commands, exact methods to keep vs delete, exact new action string.

---

## Next: B2 (separate epic — object-level scoping)

Object-level grants (`scope_id`/`object_type` on `access_grants`), resource-hierarchy inheritance, and object-share UIs — AND the deferred **data-model flip**: repoint grant writers at `access_grants`, derive `Principal.platform_roles`/`workspace_role` from it, and drop the `0068`/`0069` triggers + the two legacy tables. Then B3 (per-user data-plane delegation) and B4 (`audit_log`).
