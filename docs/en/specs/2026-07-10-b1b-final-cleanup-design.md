# B1b-final — Enforcement-unification cleanup

> **Status:** Design (approved to plan) · **Date:** 2026-07-10 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) · **Predecessor:** [B1b-3 workspace migration](./2026-07-10-b1b-3-workspace-migration-design.md).
>
> Final increment of the B1b PEP cutover. Removes the now-dead legacy gate primitives and folds the last inline admin check (`set_platform_roles`) into the admin-graph `forbid`. After this, **all authorization enforcement flows through the Cedar PDP `require()`**. Low-risk cleanup.

---

## 1. Why & scope

B1b-1/2/3 migrated every handler onto `egret_core::authz::require()`. Two legacy gate primitives and their helpers are now unreferenced by any handler (they survive only as `pub` re-exports, so they raise no dead-code warning). One admin-only mutation — `set_platform_roles` in `egret-workspace` — still uses an inline `platform_roles.contains("platform_admin")` check (a review follow-up from B1b-2). This increment finishes the unification.

**In scope:** fold `set_platform_roles` into the admin-graph `forbid`; delete the dead `egret_auth::{require_permission, require_platform_admin}`, the workspace `Authorizer` gate methods + their now-dead permission helpers, and the `egret_controlplane::auth` admin wrapper.
**Out of scope (deferred to B2's grant-model work):** the expand→contract **data-model flip** — repointing grant writers at `access_grants`, deriving `Principal.platform_roles`/`workspace_role` from `access_grants`, and dropping the `0068`/`0069` sync triggers + the two legacy tables (`platform_role_assignments`, `workspace_members`). This is higher-risk (it reaches into the identity layer) and independent (the triggers keep `access_grants` correct meanwhile); B2 reworks the grant model anyway, so the flip belongs there.

## 2. What stays (explicitly — do NOT remove)

- **`Principal.permissions`** — it is NOT dead. It is serialized to the frontend by the `/me` endpoint (`egret-auth/src/routes.rs:196` → `MeResponse.permissions`) and drives the FE's permission-aware UI gating. It is no longer an enforcement input (the PDP enforces), but it remains an FE hint. Keep the field, `SessionService::permissions()`, and the `/me` field.
- **`Principal.platform_roles`** + `WorkspaceContext::is_platform_admin` — still used by the `WsContext` extractor (admin sees a `platform_admin` display role) and by `list_my_workspaces` (admin sees all workspaces — a display filter, not a gate). Keep.
- **`Authorizer::{new, workspace_roles_for, replace_platform_roles}`** — `workspace_roles_for` is called by `WsContext` (`context.rs:57`); `replace_platform_roles` is the write logic behind `set_platform_roles` (`routes.rs:400`). Keep the struct and these three; remove only the dead gate/permission methods (§3).
- The legacy tables, triggers, and `access_grants` read-model — all stay (flip is B2).

## 3. Removals (all verified dead — no handler references them)

- `egret_auth::authz::require_permission` + `require_platform_admin` (and their `#[cfg(test)]` tests). `egret-auth/src/authz.rs` contains only these two — the file is deleted, and `pub mod authz;` + the `pub use authz::{...}` re-exports are removed from `egret-auth/src/lib.rs`. (`require_permission`'s body reads `principal.permissions`, but that field stays for `/me`; only the function goes.)
- `egret_workspace::rbac::Authorizer::{require_workspace, require_platform, workspace_permissions, platform_permissions}` — `require_workspace`/`require_platform` have no callers outside `rbac.rs`; `workspace_permissions`/`platform_permissions` were called only by those two. Remove all four; keep `new`/`workspace_roles_for`/`replace_platform_roles`.
- `egret_controlplane::auth::require_platform_admin` wrapper + its tests. (`require_permission` wrapper was already removed in B1b-2.) If `auth.rs` is left with nothing meaningful, delete it and drop its `mod` declaration.

## 4. `set_platform_roles` → admin-graph `forbid`

`set_platform_roles` (`egret-workspace/src/routes.rs:384`) assigns platform roles (including `platform_admin` itself) — the most sensitive authz-graph mutation. Today it gates on an inline `if !p.platform_roles.contains("platform_admin") { Forbidden }`. Fold it into the unified model:
- Add `"user:assign_platform_role"` to `egret_core::rbac::ADMIN_GRAPH_ACTIONS` (the `forbid` policy is generated from that list, so it is automatically covered — allowed only for `platform_admin`, non-delegatable by construction). It is NOT added to the grantable catalog (consistent with the other admin-graph actions).
- Replace the inline check with `egret_core::authz::require(&*st.pdp, &p, "user:assign_platform_role").await?` (`WorkspaceState` already carries `pdp` from B1b-3).
- **Behavior-preserving:** the `forbid`-unless-`platform_admin` yields exactly the same allow/deny as the inline check. Add a test: a `platform_admin` sets platform roles (2xx); a non-admin (even one holding every catalog permission) is denied (403).
- Leave `list_my_workspaces`'s `is_admin` filter untouched — it is a display decision, not a gate.

## 5. Testing

- `set_platform_roles` forbid test (§4).
- The admin-graph forbid test (from B1b-2) still passes; extend it to include `user:assign_platform_role` if convenient.
- Removals: `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` prove nothing referenced the deleted items.
- Full local gate: fmt, clippy, `cargo test --workspace`, frontend `lint/typecheck/test/build` (the `/me` `permissions` field is unchanged, so no FE impact expected — verify).

## 6. Open questions (for the plan)

- Confirm `egret-auth/src/authz.rs` contains ONLY the two functions (so the file can be deleted) — if it holds anything else, remove just the two functions.
- Confirm no test elsewhere imports `egret_auth::require_permission`/`require_platform_admin` (update or delete such tests).
- The exact action string for `set_platform_roles` — `user:assign_platform_role` (proposed); ensure it is added to `ADMIN_GRAPH_ACTIONS` so the `forbid` covers it, and that no route uses it as a bare permission gate.

---

*Design v1.0 (2026-07-10). Completes the B1b enforcement unification: deletes the dead legacy gate primitives and folds `set_platform_roles` into the admin-graph `forbid`. The data-model flip (drop the legacy grant tables) is deferred to B2. `Principal.permissions` stays — it feeds the frontend, not enforcement.*
