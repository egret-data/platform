# B1b-3 — Workspace seam migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate every workspace seam off `Authorizer::require_workspace` (and the workspace seam's residual `require_platform`) onto the PDP `require()`, using the `&WorkspaceContext` overload. Behavior-preserving, mechanical.

**Architecture:** Each workspace seam state gains an injected `Arc<dyn Pdp>`; every `st.authz.require_workspace(&ctx, PERM).await?` becomes `egret_core::authz::require(&*st.pdp, &ctx, PERM).await?`. The `authz: Authorizer` field stays (the `WsContext` extractor still uses `workspace_roles_for`).

**Tech Stack:** Rust, axum, `sqlx`/Postgres, `cedar-policy` (via the already-wired PDP).

**Reference:** [B1b-3 design](../specs/2026-07-10-b1b-3-workspace-migration-design.md). **This is the SAME pattern as the merged B1b-2 seams** — read `crates/egret-engine-mgmt/` (state pdp field + `engines.rs` swaps + `crates/egret-bin/src/main.rs` injection + `tests/routes.rs` real-PDP/`AllowAllPdp` wiring) as the concrete template. The only difference: workspace seams call `require(&*st.pdp, &ctx, PERM)` (a `&WorkspaceContext`) instead of `&p`.

---

## File Structure

- `crates/egret-bi/src/routes.rs` (state + 46 swaps), `crates/egret-notebook/src/{routes.rs,runtime.rs}` (state + 17), `crates/egret-query/src/{execution,registry}/routes.rs` (2 states + 13), `crates/egret-kernel/src/routes.rs` (state + 7), `crates/egret-workspace/src/routes.rs` (state + 3 workspace + 7 platform).
- `crates/egret-bin/src/main.rs` — inject `pdp: pdp.clone()` into each seam-state construction (the `pdp` var exists from B1b-1).
- Per-seam test files under each crate's `tests/`.

Shared migration recipe per seam:
1. Add `pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>` to the seam's state struct (confirm the crate has a direct `egret-core` dep; add `egret-core = { workspace = true }` if only transitive).
2. In `egret-bin/src/main.rs`, add `pdp: pdp.clone()` at that state's construction (grep `<StateName> {` to find it).
3. Swap every `require_workspace`/`require_platform` call in the seam (grep the file). `&ctx` workspace calls → `require(&*st.pdp, &ctx, PERM)`; `&p` platform calls → `require(&*st.pdp, &p, PERM)`. Keep the exact PERM constant/literal; use the handler's real state var.
4. Fix the seam's tests: real `CedarPdp` (`Arc::new(egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new()).await.unwrap())`, roles seeded BEFORE build) for any test asserting a 403; inline `AllowAllPdp` stub for incidental-authz tests (never for a 403 assertion). Add `egret-auth`/`egret-events` dev-deps if missing. Add one real-PDP assertion per seam.
5. `cargo test -p <crate>` green + `cargo build --workspace` green; commit.

---

## Task 1: Migrate `egret-bi` (BiState, 46 sites)

**Files:** `crates/egret-bi/src/routes.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-bi/tests/*`

- [ ] **Step 1: Inject pdp.** Add `pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>` to `BiState` (`routes.rs:36`). In `egret-bin/src/main.rs`, grep `BiState {` and add `pdp: pdp.clone()`.
- [ ] **Step 2: Swap the 46 calls.** `rtk proxy grep -n "require_workspace(" crates/egret-bi/src/routes.rs` — each `<st>.authz.require_workspace(&ctx, PERM).await?` → `egret_core::authz::require(&*<st>.pdp, &ctx, PERM).await?` (some sites use `ctx` vs `&ctx` — match; keep the PERM constant, e.g. `DS_READ`, `CH_MANAGE`, `DB_MANAGE`, `PERM`). Confirm zero remain after.
- [ ] **Step 3: Fix tests.** Grep `BiState {` in `crates/egret-bi/tests`. Real `CedarPdp` for 403-asserting tests (seed first); `AllowAllPdp` stub otherwise. Add: a workspace member with `dashboard:read` reads a dashboard (200); a principal lacking it is denied (403).
- [ ] **Step 4: Run.** `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-bi -- --nocapture` → PASS (authz assertion RAN). `rtk proxy cargo build --workspace` → SUCCESS.
- [ ] **Step 5: Commit.**
```bash
git add crates/egret-bi/ crates/egret-bin/src/main.rs Cargo.lock
git commit -m "feat(authz): migrate bi seam onto require()

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Migrate `egret-notebook` (NotebookState, 17 sites)

**Files:** `crates/egret-notebook/src/routes.rs` (16), `crates/egret-notebook/src/runtime.rs` (1), `crates/egret-bin/src/main.rs`, `crates/egret-notebook/tests/*`

- [ ] **Step 1: Inject pdp** into `NotebookState` (`routes.rs:38`) + `main.rs` (grep `NotebookState {`).
- [ ] **Step 2: Swap the 17 calls** in `routes.rs` and `runtime.rs` (grep both). `require_workspace(&ctx, READ|WRITE|MANAGE)` → `require(&*st.pdp, &ctx, ...)`. NOTE `runtime.rs` has a `require_workspace` call whose surrounding function may not be an axum handler — confirm it has `st`/`ctx` in scope and adapt (the doc comment there says the HTTP handler is responsible for calling require_workspace — read it).
- [ ] **Step 3: Fix tests** (grep `NotebookState {` in `crates/egret-notebook/tests`): real PDP for 403 tests, stub otherwise. Assert: a member with `notebook:read` reads (200), lacking `notebook:write` denied on edit (403).
- [ ] **Step 4: Run** `... cargo test -p egret-notebook ...` → PASS. `cargo build --workspace` → SUCCESS.
- [ ] **Step 5: Commit** `git add crates/egret-notebook/ crates/egret-bin/src/main.rs Cargo.lock` → `feat(authz): migrate notebook seam onto require()` (+ trailer).

---

## Task 3: Migrate `egret-query` (EngineState + DatabaseState, 13 sites)

**Files:** `crates/egret-query/src/execution/routes.rs` (EngineState, 6), `crates/egret-query/src/registry/routes.rs` (DatabaseState, 7), `crates/egret-bin/src/main.rs`, `crates/egret-query/tests/*`

- [ ] **Step 1: Inject pdp** into BOTH `EngineState` (`execution/routes.rs:30`) and `DatabaseState` (`registry/routes.rs:29`) + their `main.rs` constructions (grep `EngineState {` and `DatabaseState {`).
- [ ] **Step 2: Swap the 13 calls** across both files. execution uses `PERM`; registry uses `PERM_MANAGE`. `require_workspace(&ctx, PERM)` → `require(&*st.pdp, &ctx, PERM)`.
- [ ] **Step 3: Fix tests** (grep both state names in `crates/egret-query/tests`): real PDP for 403 tests. Assert: a member with `query:run` runs a query (200); a principal with `workspace:manage_members`-less role denied on a registry mutation (403).
- [ ] **Step 4: Run** `... cargo test -p egret-query ...` → PASS. `cargo build --workspace` → SUCCESS.
- [ ] **Step 5: Commit** `git add crates/egret-query/ crates/egret-bin/src/main.rs Cargo.lock` → `feat(authz): migrate query seam onto require()` (+ trailer).

---

## Task 4: Migrate `egret-kernel` (KernelState, 7 sites)

**Files:** `crates/egret-kernel/src/routes.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-kernel/tests/*`

- [ ] **Step 1: Inject pdp** into `KernelState` (`routes.rs:45`) + `main.rs` (grep `KernelState {`).
- [ ] **Step 2: Swap the 7 calls** in `routes.rs` (all `require_workspace(&ctx, "notebook:read")` per B1a). → `require(&*st.pdp, &ctx, "notebook:read")`.
- [ ] **Step 3: Fix tests** (grep `KernelState {`): real PDP for 403 tests. Assert: a member with `notebook:read` reaches a kernel route (200); a non-member denied (403).
- [ ] **Step 4: Run** `... cargo test -p egret-kernel ...` → PASS. `cargo build --workspace` → SUCCESS.
- [ ] **Step 5: Commit** `git add crates/egret-kernel/ crates/egret-bin/src/main.rs Cargo.lock` → `feat(authz): migrate kernel seam onto require()` (+ trailer).

---

## Task 5: Migrate `egret-workspace` (WorkspaceState, 3 workspace + 7 platform sites)

**Files:** `crates/egret-workspace/src/routes.rs`, `crates/egret-bin/src/main.rs`, `crates/egret-workspace/tests/*`

- [ ] **Step 1: Inject pdp** into `WorkspaceState` (`routes.rs:20`) + `main.rs` (grep `WorkspaceState {`).
- [ ] **Step 2: Swap all gate calls** in `routes.rs`:
  - 3 `require_workspace(&ctx, "workspace:manage_members")` → `require(&*st.pdp, &ctx, "workspace:manage_members")`.
  - 7 `require_platform(&p, PERM)` (`workspace:create` @104, `workspace:read` @141/@171, `workspace:update` @203, `workspace:archive` @231, `user:read` @314/@359) → `require(&*st.pdp, &p, PERM)` (`&Principal` → Platform scope).
  Grep `require_workspace(\|require_platform(` in the file to confirm all 10 migrated; none may remain.
- [ ] **Step 3: Fix tests** (grep `WorkspaceState {` in `crates/egret-workspace/tests`, e.g. `workspace_db.rs`): real PDP for 403 tests (seed first). Assert: a `platform_admin` creates a workspace (200/201); a non-admin without `workspace:create` denied (403); a workspace member manages members with the role, a non-member denied.
- [ ] **Step 4: Run** `... cargo test -p egret-workspace ...` → PASS. `cargo build --workspace` → SUCCESS.
- [ ] **Step 5: Commit** `git add crates/egret-workspace/ crates/egret-bin/src/main.rs Cargo.lock` → `feat(authz): migrate workspace seam onto require() (workspace + platform gates)` (+ trailer).

---

## Task 6: Verify complete + full gate

- [ ] **Step 1: Verify no handler uses the workspace authorizer's gate methods.** `rtk proxy grep -rn "require_workspace(\|\.require_platform(" crates --include=*.rs | grep "/src/"` — the ONLY remaining hits should be the method DEFINITIONS in `crates/egret-workspace/src/rbac.rs` (and possibly a doc-comment). No seam handler may still call them. If any remains, migrate it. Also confirm `workspace_roles_for` is still called by `crates/egret-workspace/src/context.rs` (the `authz` field must stay live for it).
- [ ] **Step 2: `catalog_drift` lint.** `export DATABASE_URL=... && rtk proxy cargo test -p egret-controlplane --test catalog_drift` → green (it scans `authz::require(`; the workspace-feature keys are now enforced via that form).
- [ ] **Step 3: Full local CI gate (sequential — never two workspace test runs at once):**
  - `rtk proxy cargo fmt --all --check` (fmt + restage if needed)
  - `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` → clean
  - `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test --workspace` → 0 failures
  - Frontend gate: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` → green (no FE surface expected)
- [ ] **Step 4: Commit** (only if Steps 1/2 required changes)
```bash
git add -u
git commit -m "test(authz): verify workspace migration complete + gate green (B1b-3)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Spec coverage:** §2 migration → Tasks 1–5; §3 `authz` field stays → respected (only gate calls swapped, field untouched) and re-verified in Task 6 Step 1; §4 tests → per-task real-PDP assertions.
- **Behavior preservation:** workspace `require_workspace` → `require()` and workspace-seam `require_platform` → `require()` are proven-equal by the B1a 60-cell matrix (crosses both scopes). Deny → `Error::Forbidden` → 403 unchanged.
- **No new model:** no forbid/catalog changes; the B1b-2 admin-graph forbid does not intersect workspace-feature actions.
- **Type consistency:** `require(&*st.pdp, &ctx, PERM)` (workspace) / `require(&*st.pdp, &p, PERM)` (platform); `pdp: Arc<dyn egret_core::authz::Pdp>` on every seam state.

---

## Next: B1b-final (separate plan)

Delete the two legacy authorizers (`egret_workspace::rbac::Authorizer` gate methods, `egret_auth::{require_permission, require_platform_admin}`) + `egret_controlplane::auth` wrappers; rework `WsContext` to derive `workspace_role` without the `Authorizer` gate methods (keep `workspace_roles_for` or fold it in); remove `Principal.permissions`; fold `egret-workspace::set_platform_roles` into the admin-graph `forbid` model (review follow-up); flip grant writers to `access_grants` and drop the sync triggers + the two legacy tables (`platform_role_assignments`, `workspace_members`).
