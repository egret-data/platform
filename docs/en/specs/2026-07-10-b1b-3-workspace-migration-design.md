# B1b-3 — Migrate the workspace seams onto `require()`

> **Status:** Design (approved to plan) · **Date:** 2026-07-10 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) · **Predecessor:** [B1b-2 platform migration + forbid](./2026-07-10-b1b-2-platform-migration-forbid-design.md).
>
> Fourth increment of B1b. Migrates every **workspace** seam off the legacy `Authorizer::require_workspace` (and the workspace seam's residual `Authorizer::require_platform`) onto the PDP `require()`. Purely mechanical — no new authorization model. After this, no handler in the codebase gates on either legacy authorizer.

---

## 1. Why & scope

B1b-2 finished the platform layer. B1b-3 finishes the workspace layer: ~86 `Authorizer::require_workspace(&ctx, action)` sites across bi (46), notebook (17), query (13), kernel (7), and workspace (3), plus the 7 `Authorizer::require_platform(&p, action)` sites in `egret-workspace/src/routes.rs` (workspace CRUD + `user:read`). This applies the `&WorkspaceContext` `AuthTarget` overload implemented and unit-tested in B1b-1.

**In scope:** inject the shared `Arc<dyn Pdp>` into `BiState`, `NotebookState`, `KernelState`, `EngineState`, `DatabaseState`, `WorkspaceState`; swap every `require_workspace`/`require_platform` gate call to `require()`; fix each seam's tests.
**Out of scope:** removing the `authz: Authorizer` field (still needed — see §3); removing the legacy authorizers/primitives, `Principal.permissions`, or the grant-writer flip (B1b-final); the `set_platform_roles` forbid fold-in (B1b-final).

## 2. The migration (mechanical, established pattern)

Identical to B1b-2, using the workspace overload:
- **Workspace gates:** `st.authz.require_workspace(&ctx, PERM).await?` → `egret_core::authz::require(&*st.pdp, &ctx, PERM).await?` (`&WorkspaceContext` → `Resource::Workspace(ctx.workspace_id)`).
- **Platform gates (workspace seam only):** `st.authz.require_platform(&p, PERM).await?` → `egret_core::authz::require(&*st.pdp, &p, PERM).await?` (`&Principal` → `Resource::Platform`).
- Each seam state gains `pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>`, injected from the shared `pdp` var in `egret-bin` (built in B1b-1).

**Behavior-preserving:** the PDP's Workspace decision equals the legacy `require_workspace` decision, and its Platform decision equals `require_platform` (both proven by the B1a 60-cell regression matrix — which crosses platform *and* workspace scopes). Workspace-feature actions (`dashboard:read`, `notebook:write`, …) are not admin-graph, so the B1b-2 `forbid` does not touch them. Deny still maps to `Error::Forbidden` → 403.

## 3. Why `authz: Authorizer` stays

Each workspace seam state keeps its `authz: Authorizer` field even though its `require_workspace`/`require_platform` methods are no longer called: the `WsContext` extractor (`egret-workspace/src/context.rs`) uses `Authorizer::workspace_roles_for(user, ws)` to compute the request's `workspace_role` (the display/precedence role on `WorkspaceContext`), via `Authorizer: FromRef<S>`. That extractor is unchanged here. The `Authorizer` struct therefore remains live (for `workspace_roles_for`); only its gate methods go unused (a `pub` method → no dead-code warning). Removing the `Authorizer` entirely — and reworking how `workspace_role` is derived — is **B1b-final**.

## 4. Testing

- Per seam: existing route tests green (same allow/deny), with a real `CedarPdp` wired into any test that asserts a 403 (seed roles **before** building the PDP — it snapshots at build; raw-SQL seeding emits no `RolesChanged`), and an inline `AllowAllPdp` stub only where authz is incidental. Add one real-PDP assertion per seam: a workspace member with the read role can read (200) but a non-member / insufficient role is denied (403).
- `catalog_drift` lint stays green (it already recognises `authz::require(`).
- Full local gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, frontend `lint/typecheck/test/build`.

## 5. Open questions (for the plan)

- Confirm each seam's handler state variable name and that the `WorkspaceContext` is in scope as `ctx` at every `require_workspace` site (some use `&ctx`, some `ctx`).
- `egret-bi` has 46 sites — the largest single-file swap; ensure the grep-driven swap catches all (constants like `DS_READ`, `CH_MANAGE`, `PERM` resolve to the literal — the swap keeps the same constant argument, only the call changes).
- Whether the workspace seam states already depend on `egret-core` (for `Arc<dyn Pdp>`) — they do transitively; confirm direct dep for naming the trait.

---

*Design v1.0 (2026-07-10). Mechanical migration of the workspace layer onto the PDP `require()` via the `&WorkspaceContext` overload; the `Authorizer` field stays for the `WsContext` extractor until B1b-final. No new authorization model.*
