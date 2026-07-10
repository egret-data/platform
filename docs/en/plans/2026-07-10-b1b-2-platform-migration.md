# B1b-2 — Platform seam migration + admin-graph `forbid` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate every platform seam off the legacy `require_permission`/`require_platform_admin` gates onto the PDP `require()`, and replace the admin-only guardrail with a Cedar `forbid` policy — a structural, non-escalatable guarantee. Behavior-preserving.

**Architecture:** A static Cedar `forbid(principal, action in ADMIN_GRAPH_ACTIONS, resource) unless { context.is_platform_admin }` is added to the PDP's policy set; `authorize()` supplies `is_platform_admin` (computed from the subject's grants) via the Cedar request context. The 3 remaining platform seam states get an injected `Arc<dyn Pdp>`; every gate call becomes `require(&*st.pdp, &p, action).await?`.

**Tech Stack:** Rust, `cedar-policy` 4.11.2, axum, `sqlx`/Postgres.

**Reference:** [B1b-2 design](../specs/2026-07-10-b1b-2-platform-migration-forbid-design.md).

---

## File Structure

- `crates/egret-core/src/rbac.rs` (modify) — `ADMIN_GRAPH_ACTIONS` const + `is_admin_graph()`.
- `crates/egret-auth/src/pdp/mod.rs` (modify) — `load_inner` adds the `forbid` policy; `authorize` supplies the `is_platform_admin` context.
- `crates/egret-controlplane/src/{access/mod.rs,storage.rs}` (modify) — swap gates.
- `crates/egret-engine-mgmt/src/state.rs` + `{engines,monitor}.rs` (modify) — inject pdp + swap.
- `crates/egret-catalog-engine/src/warehouse_admin/mod.rs` (modify) — inject pdp + swap.
- `crates/egret-policy/src/admin/mod.rs` (modify) — inject pdp + swap.
- `crates/egret-bin/src/main.rs` (modify) — set `pdp` on the 3 states (var already exists from B1b-1).
- Tests: `crates/egret-auth/tests/admin_graph_forbid.rs` (new) + per-seam test updates.

---

## Task 1: The admin-graph `forbid` model (security-critical)

**Files:**
- Modify: `crates/egret-core/src/rbac.rs`, `crates/egret-auth/src/pdp/mod.rs`
- Test: `crates/egret-auth/tests/admin_graph_forbid.rs` (new)

- [ ] **Step 1: Declare the admin-graph actions** in `crates/egret-core/src/rbac.rs`:
```rust
/// Actions that mutate the authorization graph itself (roles, role-permission
/// sets, role/group assignments). These are admin-only and NON-delegatable: the
/// PDP `forbid`s them for everyone but `platform_admin`. They are deliberately
/// NOT in `PERMISSIONS` (not grantable) — see the B1b-2 design.
pub static ADMIN_GRAPH_ACTIONS: &[&str] = &[
    "role:create",
    "role:update",
    "role:delete",
    "role:assign_permission",
    "user:assign_role",
    "group:manage_members",
];

/// True if `key` is an admin-graph (non-delegatable) action.
pub fn is_admin_graph(key: &str) -> bool {
    ADMIN_GRAPH_ACTIONS.contains(&key)
}
```
Add a test asserting `is_admin_graph("role:create")` and `!is_admin_graph("engine:read")`, and that NONE of `ADMIN_GRAPH_ACTIONS` appears in the grantable `PERMISSIONS` (so they stay non-grantable): `for a in ADMIN_GRAPH_ACTIONS { assert!(!is_known(a), "{a} must not be grantable"); }`.

- [ ] **Step 2: Add the `forbid` policy + admin context to the PDP.** In `crates/egret-auth/src/pdp/mod.rs`:

In `load_inner`, after building the per-role templates into `templates`, add the static forbid policy:
```rust
let actions = egret_core::rbac::ADMIN_GRAPH_ACTIONS
    .iter()
    .map(|a| format!("Egret::Action::\"{a}\""))
    .collect::<Vec<_>>()
    .join(", ");
let forbid_src = format!(
    "forbid(principal, action in [{actions}], resource) \
     unless {{ context has is_platform_admin && context.is_platform_admin }};"
);
let forbid: cedar_policy::Policy = forbid_src
    .parse()
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("forbid policy: {e:?}")))?;
templates
    .add(forbid)
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("add forbid: {e:?}")))?;
```
(Verify the 4.11.2 API: `Policy` implements `FromStr`; `PolicySet::add(&mut self, Policy)` adds a static policy. If the method name differs, adapt — a quick `cargo test` compile will tell you. `PolicySet` holds both static policies and templates+links together.)

In `authorize`, compute the admin flag from the already-loaded grants and pass it as the request context (replacing `Context::empty()`):
```rust
let inner = self.inner.load();
let grants = self.grants.grants_for(subject.user_id, ws).await?;
let is_admin = grants.iter().any(|g| {
    g.scope_type == "platform"
        && inner.role_names.get(&g.role_id).map(String::as_str) == Some("platform_admin")
});
let context = cedar_policy::Context::from_json_value(
    serde_json::json!({ "is_platform_admin": is_admin }),
    None,
)
.map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("context: {e:?}")))?;
// ... build p, a, r ...
let req = cedar_policy::Request::new(p, a, r, context, None)
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
```
(Adapt `Context::from_json_value`'s arity to 4.11.2 — it may take `Option<(&Schema, &EntityUid)>` as the 2nd arg; pass `None`. Ensure `serde_json` is a dep of egret-auth — it is.) Keep the rest of `authorize` identical.

- [ ] **Step 3: Write the security test** — `crates/egret-auth/tests/admin_graph_forbid.rs` (DB-gated). Seed and assert:
```
// Build CedarPdp (fresh bus). Seed:
//  - user ADMIN with platform_role 'platform_admin'
//  - user USER with a platform role holding EVERY grantable catalog permission
//    (iterate rbac::PERMISSIONS at Scope::Platform), assigned platform-scope
//  - adversarial: force-insert 'role:create' into USER's role via raw
//    `INSERT INTO role_permissions` (bypassing the handler validation) + publish
//    RolesChanged / rebuild, to prove the forbid — not catalog hygiene — is what denies.
// Assertions (Resource::Platform):
//  for a in ADMIN_GRAPH_ACTIONS:
//     assert!( pdp.authorize(&admin_subject, a, &Platform).allow );      // admin allowed
//     assert!( !pdp.authorize(&user_subject, a, &Platform).allow );      // non-admin denied even w/ all perms + force-inserted role:create
//  // sanity: a normal platform permission the USER holds is still allowed
//  assert!( pdp.authorize(&user_subject, "<a perm USER holds>", &Platform).allow );
```
Use the seed/harness idiom from `crates/egret-auth/tests/cedar_pdp.rs`. Because the PDP snapshots at build, either build the PDP AFTER seeding, or (for the adversarial force-insert) publish `RolesChanged` through a `RoleRepo` on the same bus and poll for reload (reuse `pdp_hot_reload.rs`'s `poll_until`).

- [ ] **Step 4: Run** — `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test admin_graph_forbid -- --nocapture` → PASS, confirm it RAN. Also `rtk proxy cargo test -p egret-auth --test pdp_regression` → still 60/60 (the forbid must not change any non-admin-graph decision — the matrix actions are all non-admin-graph, so unaffected).

- [ ] **Step 5: Commit**
```bash
git add crates/egret-core/src/rbac.rs crates/egret-auth/src/pdp/mod.rs crates/egret-auth/tests/admin_graph_forbid.rs
git commit -m "feat(authz): admin-graph forbid policy — non-escalatable platform_admin gate

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Migrate `access` + `storage` (CpState — no injection)

**Files:**
- Modify: `crates/egret-controlplane/src/access/mod.rs`, `crates/egret-controlplane/src/storage.rs`
- Test: existing controlplane route tests

`CpState` already has `pdp` (B1b-1). Only call-site swaps + test wiring.

- [ ] **Step 1: Swap `require_permission` sites** in both files. Each `require_permission(&p, "key")?` → `egret_core::authz::require(&*st.pdp, &p, "key").await?` (use each handler's actual state var — often `st`/`state` — and principal var). storage.rs has 8; access/mod.rs has 13 `require_permission` (the `user:*`/`group:*`/`role:read` reads).

- [ ] **Step 2: Migrate the `require_platform_admin` sites** in `access/mod.rs`. Each `require_platform_admin(&p)?` → `egret_core::authz::require(&*st.pdp, &p, "<admin-graph action>").await?`. Map each route to its action (these MUST all be members of `rbac::ADMIN_GRAPH_ACTIONS` or the forbid won't cover them — verify each against the Task 1 list):
  - `create_role` → `"role:create"`
  - `update_role` → `"role:update"`
  - `delete_role` → `"role:delete"`
  - `set_role_permissions` → `"role:assign_permission"`
  - `assign_user_role` / `unassign_user_role` → `"user:assign_role"`
  - `assign_group_role` / `unassign_group_role` → `"user:assign_role"` (role→principal assignment)
  - `add_group_member` / `remove_group_member` → `"group:manage_members"`
  - any other `require_platform_admin` site found by grep → map to the matching admin-graph action; if a route needs an action NOT in the list, ADD it to `ADMIN_GRAPH_ACTIONS` in Task 1's file (and re-run Task 1's test) rather than leaving it permission-gated.
  Grep first: `rtk proxy grep -n "require_platform_admin" crates/egret-controlplane/src/access/mod.rs` — migrate EVERY site; none may remain. Remove the now-unused `require_platform_admin`/`require_permission` imports from the file if fully unused.

- [ ] **Step 3: Fix the controlplane tests.** The access/role tests assert 403 for under-privileged principals and now flow through `state.pdp`. Any test builder using the `AllowAllPdp` stub that exercises an access/role/storage route with a 403 assertion MUST switch to a real `CedarPdp` (seed roles before build). Critically, the RBAC-graph tests (create role as non-admin → 403) now depend on the forbid policy — they MUST use a real PDP so the forbid actually runs. Add an assertion: a non-`platform_admin` who holds `role:read` (a grantable perm) can list roles (200) but cannot create a role (403 via forbid).

- [ ] **Step 4: Run** — `export DATABASE_URL=... && rtk proxy cargo test -p egret-controlplane -- --nocapture` → PASS; confirm the RBAC-graph 403 tests RAN under a real PDP.

- [ ] **Step 5: Commit**
```bash
git add crates/egret-controlplane/src/access/mod.rs crates/egret-controlplane/src/storage.rs
git add -u
git commit -m "feat(authz): migrate access + storage seams onto require() (+ admin-graph forbid)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Migrate `engine-mgmt` (inject pdp into `EngineMgmtState`)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/state.rs`, `crates/egret-engine-mgmt/src/engines.rs`, `crates/egret-engine-mgmt/src/monitor.rs`, `crates/egret-bin/src/main.rs`
- Test: engine-mgmt route tests

- [ ] **Step 1: Inject the pdp.** Add `pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>` to `EngineMgmtState` (`crates/egret-engine-mgmt/src/state.rs`; confirm `egret-core` is a dep — it is). In `crates/egret-bin/src/main.rs` at the `EngineMgmtState { ... }` construction (~line 1287), add `pdp: pdp.clone()` (the `pdp` var already exists from B1b-1).

- [ ] **Step 2: Swap the 19 gate calls** in `engines.rs` (15) and `monitor.rs` (4): each `require_permission(&p, "engine:...")?` → `egret_core::authz::require(&*st.pdp, &p, "engine:...").await?` (actual state/principal var names per handler). Grep to confirm none missed: `rtk proxy grep -rn "require_permission(" crates/egret-engine-mgmt/src`.

- [ ] **Step 3: Fix tests.** engine-mgmt route tests build `EngineMgmtState` (grep `EngineMgmtState {` in `crates/egret-engine-mgmt/tests`). Give each a pdp: real `CedarPdp` (seed roles first) for authz-asserting tests; an inline `AllowAllPdp` stub (as in B1b-1) only where authz is incidental. Add one real-PDP assertion: a principal with `engine:read` (via a platform role) can GET engines (200) but not create (403).

- [ ] **Step 4: Run** — `export DATABASE_URL=... && rtk proxy cargo test -p egret-engine-mgmt -- --nocapture` → PASS, authz assertion RAN. `rtk proxy cargo build --workspace` → SUCCESS.

- [ ] **Step 5: Commit**
```bash
git add crates/egret-engine-mgmt/ crates/egret-bin/src/main.rs
git commit -m "feat(authz): migrate engine-mgmt seam onto require()

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Migrate `warehouse-admin` (inject pdp into `CatalogAdminState`)

**Files:**
- Modify: `crates/egret-catalog-engine/src/warehouse_admin/mod.rs`, `crates/egret-bin/src/main.rs`
- Test: catalog-engine warehouse-admin route tests

- [ ] **Step 1: Inject the pdp.** Add `pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>` to `CatalogAdminState` (`warehouse_admin/mod.rs:44`; confirm `egret-core` dep). In `egret-bin/src/main.rs` at `CatalogAdminState { ... }` (~line 1263), add `pdp: pdp.clone()`.

- [ ] **Step 2: Swap the 28 gate calls** in `warehouse_admin/mod.rs`: each `require_permission(&p, "warehouse:...")?` → `egret_core::authz::require(&*st.pdp, &p, "warehouse:...").await?`. Grep to confirm none missed.

- [ ] **Step 3: Fix tests** (grep `CatalogAdminState {` in `crates/egret-catalog-engine/tests`): real `CedarPdp` for authz-asserting tests (seed first), stub otherwise. Add one real-PDP assertion (a `warehouse:read` holder reads but can't create).

- [ ] **Step 4: Run** — `export DATABASE_URL=... && rtk proxy cargo test -p egret-catalog-engine -- --nocapture` → PASS. `rtk proxy cargo build --workspace` → SUCCESS.

- [ ] **Step 5: Commit**
```bash
git add crates/egret-catalog-engine/ crates/egret-bin/src/main.rs
git commit -m "feat(authz): migrate warehouse-admin seam onto require()

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Migrate `policy-admin` (inject pdp into `PolicyAdminState`)

**Files:**
- Modify: `crates/egret-policy/src/admin/mod.rs`, `crates/egret-bin/src/main.rs`
- Test: policy admin route tests

- [ ] **Step 1: Inject the pdp.** Add `pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>` to `PolicyAdminState` (`admin/mod.rs:37`; confirm `egret-core` dep). In `egret-bin/src/main.rs` at `PolicyAdminState { ... }` (~line 1275), add `pdp: pdp.clone()`.

- [ ] **Step 2: Swap the 12 gate calls** in `admin/mod.rs`: each `require_permission(&p, "policy:...")?` → `egret_core::authz::require(&*st.pdp, &p, "policy:...").await?`. Grep to confirm none missed. NOTE: `egret-policy` has its own local `Principal`? No — it uses `egret_core::Principal` here (the require_permission sites take `&p` where `p` is the auth principal). Confirm by reading the handler signatures.

- [ ] **Step 3: Fix tests** (grep `PolicyAdminState {` in `crates/egret-policy/tests`): real `CedarPdp` for authz-asserting tests (seed first), stub otherwise. Add one real-PDP assertion (a `policy:read` holder reads but can't create).

- [ ] **Step 4: Run** — `export DATABASE_URL=... && rtk proxy cargo test -p egret-policy -- --nocapture` → PASS. `rtk proxy cargo build --workspace` → SUCCESS.

- [ ] **Step 5: Commit**
```bash
git add crates/egret-policy/ crates/egret-bin/src/main.rs
git commit -m "feat(authz): migrate policy-admin seam onto require()

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Verify no platform `require_permission` remains + full gate

- [ ] **Step 1: Assert the platform migration is complete.** Grep: `rtk proxy grep -rn "require_permission(\|require_platform_admin(" crates --include=*.rs | grep "/src/" | grep -v "egret-auth/src/authz.rs" | grep -v "egret-controlplane/src/auth.rs"` — the ONLY remaining `/src/` hits should be the primitive definitions themselves (`egret-auth/src/authz.rs`, `egret-controlplane/src/auth.rs`), which are intentionally left dead until B1b-final. Every seam handler must be migrated. If any handler site remains, migrate it.

- [ ] **Step 2: `catalog_drift` lint.** Run `rtk proxy cargo test -p egret-controlplane --test catalog_drift`. The admin-graph actions are enforced now via `require(..., "role:create")`; ensure the lint (which scans `authz::require(`) sees them and that no grantable catalog key is unenforced. If the lint flags the admin-graph actions as catalog entries that must be enforced-and-grantable, reconcile: they are NOT in the grantable `PERMISSIONS`, so the lint should not expect them there — adjust the lint's expectation if needed (they're enforced, not grantable).

- [ ] **Step 3: Full local CI gate (sequential — never two workspace test runs at once):**
  - `rtk proxy cargo fmt --all --check` (fmt + restage if needed)
  - `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` → clean
  - `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test --workspace` → 0 failures
  - Frontend gate: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` → green (no FE surface expected)

- [ ] **Step 4: Commit** (only if Step 1/2 required changes; otherwise nothing to commit here)
```bash
git add -u
git commit -m "test(authz): verify platform migration complete + drift lint for admin-graph

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Spec coverage:** §2 forbid model → Task 1; §3 per-seam migration → Tasks 2–5; §4 scope (legacy primitives left dead) → Task 6 Step 1; §5 tests distributed.
- **Critical consistency:** every migrated `require_platform_admin` site uses an action that IS in `ADMIN_GRAPH_ACTIONS` (Task 2 Step 2 enforces this; if a new action is needed, it's added to the list in Task 1). Otherwise an admin-graph route would become a permission gate — a behavior change.
- **Behavior preservation:** platform `require_permission` → `require()` is proven-equal by the B1a matrix; `require_platform_admin` → `require()`+forbid is proven-equal by Task 1's adversarial test (non-admin with all perms + force-inserted role:create still denied).
- **No placeholders:** the only prose-sketched item is Task 1 Step 3's seed (concrete assertions listed) and the exact per-seam test files (found by grep in-task). Cedar API points to verify (`Policy::from_str`, `PolicySet::add`, `Context::from_json_value` arity) are flagged with adapt-to-compiler notes.

---

## Next: B1b-3 (separate plan)

Migrate the workspace seams (bi, notebook, query, kernel, workspace) off `Authorizer::require_workspace` onto `require(&*st.pdp, &ctx, action)` — applying the `&WorkspaceContext` overload (implemented + unit-tested in B1b-1). Then B1b-final: delete the two legacy authorizers + `egret_auth::{require_permission, require_platform_admin}` + `Principal.permissions`, flip grant writers to `access_grants`, drop the sync triggers + legacy tables.
