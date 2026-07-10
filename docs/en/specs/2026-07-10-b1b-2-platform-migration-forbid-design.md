# B1b-2 — Migrate the platform seams + admin-graph `forbid` model

> **Status:** Design (approved to plan) · **Date:** 2026-07-10 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) · **Predecessor:** [B1b-1 PDP wiring + require()](./2026-07-10-b1b-1-pdp-wiring-require-design.md).
>
> Third increment of B1b. Migrates every remaining **platform** seam off the two legacy static gates (`require_permission` and `require_platform_admin`) onto the PDP-backed `require()`, and models the admin-only guardrail as a Cedar `forbid` policy — turning a fragile "never-grantable string" invariant into a structural guarantee.

---

## 1. Why & scope

B1b-1 wired the PDP and migrated the `secrets` seam. B1b-2 finishes the platform layer: ~80 `require_permission` sites across access, storage, engine-mgmt, warehouse-admin, and policy-admin, plus the 11 `require_platform_admin` sites on the RBAC-graph-mutation routes. The mechanical part is the established B1b-1 pattern (inject `Arc<dyn Pdp>` into each seam state; swap the call). The non-mechanical part — and the reason this is a design, not just a chore — is **modelling admin-only through the PDP correctly** (§2).

**In scope:** the admin-graph `forbid` model; migrating all platform `require_permission` + `require_platform_admin` call sites onto `require()`; injecting the shared PDP into `EngineMgmtState`, `CatalogAdminState`, `PolicyAdminState` (access/storage already have it via `CpState`).
**Out of scope:** workspace seams (B1b-3); removing the now-dead legacy primitives (`require_permission`, `require_platform_admin`) and `Principal.permissions` (B1b-final, once no caller remains); `Principal.platform_roles` + `is_platform_admin` (kept — still used by the workspace-context extractor).

## 2. Admin-graph `forbid` model (security-critical — review this section closely)

### 2.1 The problem with the naive approach
Authorization has two distinct axes that today's code conflates:
1. **Permission (RBAC):** "can subject do action X?" — *delegatable* via roles.
2. **Admin-graph guardrail:** "only `platform_admin` may mutate the authorization graph itself (create/delete roles, assign permissions/roles, change group membership)." — deliberately **non-delegatable**; this is the PR #174 privilege-escalation fix.

Today axis 2 is handled by removing those actions from the catalog and gating the routes on `require_platform_admin` — an invisible third authorizer. Routing them through the PDP as ordinary actions would make them *delegatable* (grant the action to a role → a non-admin gains it), which defeats the guardrail. Guarding that with a "this string must never enter `role_permissions`" runtime check is a **negative invariant** — fragile exactly where it matters most.

### 2.2 The model: a static `forbid` policy
Cedar's `forbid` **overrides** any `permit`. So the guardrail becomes structural:
```cedar
// Admin-graph actions are allowed ONLY for platform_admin. forbid wins over any
// permit, so no grant — accidental or malicious — can ever override this.
forbid(
  principal,
  action in [Egret::Action::"role:create", Egret::Action::"role:update",
             Egret::Action::"role:delete", Egret::Action::"role:assign_permission",
             Egret::Action::"user:assign_role", Egret::Action::"group:manage_members"],
  resource
)
unless { context has is_platform_admin && context.is_platform_admin };
```
- **Non-escalatable by construction:** even if `role:create` were somehow granted to a non-admin role, the `forbid` still denies. The escalation protection no longer depends on catalog hygiene.
- **`unless { context has is_platform_admin && context.is_platform_admin }`** is **fail-safe**: if the flag is missing from the request context, `has` is false → the `unless` is false → the `forbid` **applies** (deny). A malformed request can only fail closed.
- The exact action list is a Rust const `ADMIN_GRAPH_ACTIONS: &[&str]` in `egret-core::rbac`, and the Cedar `forbid` string is generated from it (single source of truth). The list is the set of actions currently gated by `require_platform_admin` (enumerated per-route during the plan by reading `access/mod.rs`).

### 2.3 Supplying `is_platform_admin`
The PDP's `authorize()` already loads the subject's grants (`grants_for`) and the `role_id → name` map. It computes:
```rust
let is_admin = grants.iter().any(|g| g.scope_type == "platform"
    && inner.role_names.get(&g.role_id).map(String::as_str) == Some("platform_admin"));
```
and passes `Context::from_json_value(json!({ "is_platform_admin": is_admin }))` into the Cedar `Request` (replacing `Context::empty()`). This reuses the exact admin signal the platform_admin permit template already keys on — no new source of truth, no entity restructuring.
(Alternative considered: add `User in Role::"platform_admin"` entity edges in `build_entities` and use `unless { principal in Role::"platform_admin" }`. More Cedar-idiomatic but more invasive to the B1a entity code; the context flag is simpler to review for a security gate. Either is correct; this design uses the context flag.)

### 2.4 The static policy in `load_inner`
`CedarPdp::load_inner` (B1b-0) builds a `PolicySet` of per-role linked templates. It now **also** adds the static `forbid` policy (generated from `ADMIN_GRAPH_ACTIONS`) once. Being constant, it is re-added on every hot-reload rebuild — no staleness. It is a plain `Policy` (not a template), parsed and `add`ed to the set.

### 2.5 Catalog treatment (defense in depth)
`ADMIN_GRAPH_ACTIONS` are recognised by the PDP but are **NOT** added to the grantable catalog surfaced by the access console (`list_permissions`), and `set_role_permissions`/`RoleRepo::set_permissions` continue to reject them (they are not `is_known` grantable keys). So there are **two** independent layers: they can't be offered/inserted as role permissions (hygiene), and even if inserted, the `forbid` denies (structural). The `forbid` is the guarantee; the catalog exclusion is the "don't even offer it."

### 2.6 Migration of the 11 sites & tests
Each `require_platform_admin(&p)?` site becomes `require(&*st.pdp, &p, "<that route's admin-graph action>").await?` — using the specific action for that route (e.g. the create-role route → `"role:create"`), which is more auditable than a blanket admin check and uniform with every other gate. No `require_admin` helper is introduced.

**Behavior-preservation tests (must):**
- `platform_admin` is allowed every admin-graph action.
- A non-admin holding **every** grantable catalog permission (and, adversarially, even a role into which `role:create` was force-inserted via raw SQL) is **denied** every admin-graph action.
- A non-admin `is_platform_admin=false` context path returns `Error::Forbidden` → 403 (same as legacy).
- Extend the 60-cell PDP regression matrix (or add a focused matrix) to include the admin-graph actions × {admin, non-admin} and assert the `forbid` outcome.

## 3. Per-seam migration (mechanical, established pattern)

For each seam: add `pub pdp: Arc<dyn egret_core::authz::Pdp>` to its state (where absent), inject the shared PDP from `egret-bin`, replace each `require_permission(&p, "key")?` with `egret_core::authz::require(&*st.pdp, &p, "key").await?` (adapt the state/principal variable names per handler), and fix that seam's tests to use a real `CedarPdp` where they assert authz outcomes (seed roles **before** building the PDP, since raw-SQL seeding emits no `RolesChanged`). Behavior is preserved because the PDP's Platform decision equals the legacy `require_permission` decision (B1a regression matrix).

| Seam | File(s) | ~sites | State | Injection |
|---|---|---|---|---|
| access | `egret-controlplane/src/access/mod.rs` | 13 `require_permission` + 11 `require_platform_admin` | `CpState` | already has `pdp` |
| storage | `egret-controlplane/src/storage.rs` | 8 | `CpState` | already has `pdp` |
| engine-mgmt | `egret-engine-mgmt/src/{engines,monitor}.rs` | 19 | `EngineMgmtState` | **add + wire** |
| warehouse-admin | `egret-catalog-engine/src/warehouse_admin/mod.rs` | 28 | `CatalogAdminState` | **add + wire** |
| policy-admin | `egret-policy/src/admin/mod.rs` | 12 | `PolicyAdminState` | **add + wire** |

## 4. Scope boundary

- The legacy `egret_auth::{require_permission, require_platform_admin}` primitives and `egret_controlplane::auth` wrappers become **dead** after this increment (no platform caller left; workspace seams still use the workspace `Authorizer`). Do NOT delete them here — that is B1b-final, together with `Principal.permissions` removal and the grant-writer flip. Leaving them dead-but-present keeps this PR a pure migration.
- `Principal.platform_roles` + `WorkspaceContext::is_platform_admin` stay (used by the workspace extractor).
- `catalog_drift` lint already recognises `authz::require(` (B1b-1). Confirm the admin-graph actions are seen as enforced (they now appear as `require(..., "role:create")` sites) and not flagged as unenforced catalog entries — since they are NOT in the grantable catalog, the lint should not expect them there; keep the lint honest.

## 5. Testing

- Admin-graph forbid unit/integration tests (§2.6).
- Each migrated seam: existing route tests green (same allow/deny) + a real-PDP authz assertion.
- Full local gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, frontend `lint/typecheck/test/build`.

## 6. Open questions (for the plan)

- Exact per-route action names for the 11 `require_platform_admin` sites — enumerate by reading `access/mod.rs` (create/update/delete role, set-permissions, assign/unassign user role, add/remove group member, replace-platform-roles). Some may share an action.
- Whether the admin-graph actions should also appear (read-only, greyed) in the access console for transparency, or stay entirely hidden — default hidden (matches today); revisit in a UX pass.
- `Context::from_json_value` shape + confirming Cedar evaluates `context has is_platform_admin` as intended in 4.11.2 (the B1a spike confirmed the core API; verify the `has` operator on context in a quick test).

---

*Design v1.0 (2026-07-10). Migrates the platform layer onto the PDP `require()` and replaces the `require_platform_admin` guardrail with a Cedar `forbid` policy — a structural, non-escalatable admin-graph guarantee that also lays the deny-override groundwork B2 needs. Legacy-primitive removal is deferred to B1b-final.*
