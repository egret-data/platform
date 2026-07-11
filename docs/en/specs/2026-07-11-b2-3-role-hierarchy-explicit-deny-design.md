# B2-3 — Role hierarchy + explicit deny (deny-wins)

> **Status:** Design (approved to plan) · **Date:** 2026-07-11 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) §2 (B2) · **Predecessors:** [B2-1a object-scope backend](./2026-07-10-b2-1a-object-scope-design.md), [B2-1b sharing UI + data path](./2026-07-11-b2-1b-sharing-ui-datagap-design.md), [B2-2 notebooks + dataset preview](./2026-07-11-b2-2-notebooks-dataset-preview-design.md).
>
> Fourth B2 increment. Adds two independent authorization capabilities that share a single seam (Cedar template generation): **role hierarchy** (a role inherits a parent role's permissions) and **explicit deny** (a grant with `effect=deny` that overrides allows — the Ranger-parity carve-out).

---

## 1. Goal & scope

Two capabilities, chosen together:

1. **Role hierarchy** — a role may inherit another role's permissions via a single `parent_role_id`, constrained to the **same scope**. `ws_viewer ← ws_editor ← ws_admin` becomes a declared chain instead of hand-listed permission sets. DRYs role authoring and makes custom roles composable.
2. **Explicit deny (deny-wins)** — an `access_grants` row may carry `effect=deny`; a deny always beats an allow. This is the carve-out the current allow-only cascade cannot express: share a whole folder as Viewer, then **block** one sensitive dashboard for one user.

Both land in one increment. They are independent in the data model and UI but converge on `CedarPdp::load_inner` (template generation), so implementing them together avoids touching that seam twice.

**In scope:** `roles.parent_role_id` + same-scope/acyclic enforcement + transitive-closure permission expansion in template generation; `access_grants.effect` + a `forbid` template per role + effect-aware grant linking; deny-aware object-guest admission; share-modal "Blocked" option; RBAC-console role-inheritance editor.

**Out of scope:** the reverse deny direction (**deny-broad + allow-narrow / deny-exceptions** — Cedar's `forbid` always beats `permit`, so a re-allow inside a deny is not expressible without conditional forbids; deferred, likely never needed); multi-parent role DAGs (single-parent only); cross-scope inheritance; the deferred data-model flip (B2-flip); per-user data-plane delegation (B3); audit_log (B4).

**Decisions locked in brainstorming:** both features; deny is **deny-wins only**; hierarchy is **single-parent, same-scope**; the share-modal deny is a single **"Blocked (No access)"** effect implemented as a deny grant keyed to the **`ws_editor`** role (Editor ⊇ Viewer via hierarchy → denies read+manage = full block).

## 2. Role hierarchy

### 2.1 Data model
`ALTER TABLE roles ADD COLUMN parent_role_id uuid REFERENCES roles(id) ON DELETE SET NULL;` (new migration; repo is past 0070 — use the next free number). `ON DELETE SET NULL` so deleting a parent role demotes children to no-parent rather than cascading a delete.

**Same-scope + acyclic** cannot be enforced by a plain `CHECK` (it can't reference another row). `RoleRepo` (the sole application writer of `roles`/`role_permissions`, verified in B1b-0) enforces both on set-parent:
- reject if `parent.scope != role.scope` (→ a validation error, HTTP 400);
- reject if walking the parent chain from `parent_role_id` reaches `role.id` (cycle) (→ 400).

A defense-in-depth trigger `roles_parent_same_scope_and_acyclic` (BEFORE INSERT/UPDATE) re-checks the scope match and walks the chain, raising an exception on violation — so a raw SQL write can't corrupt the graph either.

### 2.2 Transitive-closure expansion (the seam)
Today `load_inner` builds each role's template body from its own `role_permissions`. New behavior: a role's **effective** permissions are the union over its parent chain — `own ∪ parent's effective ∪ …`. Compute the closure in `load_inner`:
1. Load all roles (`id, name, scope, parent_role_id`) and all `role_permissions` in two queries (already batched per role today; switch to two bulk queries to compute closure without N+1).
2. For each role, walk `parent_role_id` upward (chain is acyclic by §2.1), unioning permission sets. Cap the walk defensively (e.g. 32 hops) and log if exceeded (should never happen given the acyclic guarantee — a belt-and-suspenders against a corrupted graph).
3. Emit the `permit` template with the **effective** action list.

Everything downstream is unchanged: `link_grants`, `build_entities`, the admin-graph `forbid`, the `platform_admin` unrestricted body. Hierarchy is invisible to the request path — it only fattens template bodies at build time.

**Hot-reload:** a parent change is a role change. `RoleRepo` must publish `RolesChanged` on set-parent (it already does on create/delete/set_permissions) so the PDP rebuilds and re-computes closures.

### 2.3 API + UI
- `RoleRepo` create/update accept an optional `parent_role_id`; the role DTO exposes `parent_role_id` + a computed `inherited_permissions` (effective minus own) for display.
- RBAC-console role editor: an **"Inherits from"** picker listing same-scope roles, excluding self and the role's own descendants (client filters using the roles list to avoid offering a cycle; the backend re-validates). The permission editor shows own permissions (editable) and inherited permissions (read-only, badged with the source role).

## 3. Explicit deny (deny-wins)

### 3.1 Data model
`ALTER TABLE access_grants ADD COLUMN effect text NOT NULL DEFAULT 'allow' CHECK (effect IN ('allow','deny'));` and extend the uniqueness to include effect: drop the old `UNIQUE (principal_type, principal_id, role_id, scope_type, scope_id)` and re-add it with `effect` appended, so an allow and a deny on the same (principal, role, scope) can coexist (deny wins; coexistence is harmless and keeps upserts per-effect). Default `allow` → all existing rows and the B1a backfill/sync triggers keep producing allow grants unchanged, so **PDP parity is untouched**.

### 3.2 Grant loading
`Grant` gains `effect: String`; `grants_for` adds `ag.effect` to the SELECT and populates it. No filter change — both allow and deny grants are returned; the effect drives which template links.

### 3.3 Cedar emission (the seam, part 2)
`load_inner` emits **two templates per role**:
- `role:{id}` — `permit(...)` with the effective action list (unchanged, plus §2.2 closure).
- `role-deny:{id}` — `forbid(...)` with the **same** effective action list and the **same** scope operator (`in` for workspace/object roles, `==` for platform roles). The `platform_admin` role's deny template mirrors its unrestricted permit as a deny-all (`forbid(principal in ?principal, action, resource in ?resource)`).

`link_grants` picks the template by effect: `allow → role:{id}`, `deny → role-deny:{id}`, binding the same `?principal`/`?resource` slots. Cedar's `forbid` overrides every `permit`, so a deny grant beats any allow **for the actions and scope-cascade it covers**. Deny cascades in the same direction as allow: a deny on a folder (`forbid resource in folder:{id}`) blocks every item in the folder; a deny on an item blocks just that item.

`is_platform_admin` computation is unchanged — it counts platform-scoped **allow** grants on `platform_admin`. (A deny on the platform_admin role is an exotic edge case; the Cedar deny-all `forbid` still denies that subject's actual requests, so authorization is correct even though the ADMIN_GRAPH exemption flag ignores the deny. Documented, not handled specially.)

### 3.4 Share API + object-guest admission
- `grant_put` (`egret-bi/src/shares.rs`) accepts `effect` (default `allow`). The share modal's **"Blocked"** sends `effect=deny` with `role=ws_editor` (the Editor role, whose effective actions after §2.2 are read+manage). Creating a deny still requires `require_object(manage_action(kind), obj)` — same gate as an allow grant.
- **`egret_workspace::rbac::Authorizer::has_object_grant_in_workspace` must filter `effect='allow'`.** It admits a non-member who holds an object grant; a lone **deny** grant must NOT admit anyone (admitting someone solely to deny them is pointless and, worse, would let a deny row act as an admission side-channel). Add `AND ag.effect = 'allow'` to every arm of that query.
- `grant_list` returns deny grants too (so a manager sees the blocks), tagged with `effect`.

### 3.5 UI (share modal)
The role dropdown gains a third option **"Blocked (No access)"** alongside Viewer/Editor. Selecting it PUTs `{role: "ws_editor", effect: "deny"}`. Blocked principals render with a distinct "No access" badge (not a role label). Inherited denies (a deny on the containing folder) appear in the read-only inherited section like inherited allows, so a manager understands why a principal is blocked. Grant removal (`DELETE`) works identically for deny grants.

## 4. Security

- **Deny-wins is fail-safe**: adding a deny only removes access; it cannot grant anything. A misconfigured deny over-blocks (safe), never over-permits.
- **Admission excludes deny** (§3.4): the object-guest admission joins only `effect='allow'` grants, so a deny never widens the `WsContext` admission set.
- **Cross-scope inheritance forbidden** (§2.1): a workspace role cannot inherit a platform role, preventing a ws role from hoovering platform permissions through a parent link.
- **Cycle prevention** (§2.1): the parent chain is acyclic by app + trigger enforcement; the closure walk is additionally hop-capped, so a corrupted graph degrades to a bounded computation, not an infinite loop / DoS.
- **Deny requires manage** (§3.4): only a principal who can already manage the object (role or object-Editor share, or admin) can plant a deny — a Viewer cannot block others.
- **Admin-graph forbid unaffected**: the per-role deny templates are orthogonal to the static ADMIN_GRAPH `forbid`; both are `forbid` policies and both simply deny — no interaction that could escalate.
- **Parity preserved**: `effect` defaults to `allow`; the B1a regression matrix (allow-only) must still pass unchanged.

## 5. Testing

- **Role hierarchy:** `ws_editor` inheriting `ws_viewer` yields the union (a request for `ws_viewer`'s read action is allowed via an `ws_editor` grant); a set-parent that would create a cycle is rejected (400); a cross-scope parent is rejected (400); changing a parent hot-reloads the PDP (an app-wiring test proving the effective closure changes without restart); the closure walk terminates on a (test-forced) deep chain.
- **Explicit deny:** allow on a folder + deny on one item inside it → that item denied, siblings still allowed (deny-wins carve-out); a deny on the folder → every item denied; a non-member holding ONLY a deny grant is NOT admitted by `has_object_grant_in_workspace` (regression for §3.4); a Viewer cannot create a deny (403 on the share PUT); an Editor/manager can; `grant_list` surfaces the deny.
- **PDP parity:** the B1a regression matrix (allow-only) passes unchanged; a new object-scope matrix covers allow+deny combinations.
- **Frontend:** the role editor renders the inherits-from picker (excluding self/descendants) and shows inherited perms read-only; the share modal offers Blocked and PUTs a deny grant; blocked principals show the No-access badge; inherited deny shown read-only. Vitest.
- **Full gate:** BE `cargo test --workspace`, FE `lint/typecheck/test/build`, openapi + client current, adversarial security review.

## 6. Open questions (for the plan)

- **Closure computation cost:** two bulk queries + in-memory closure at `load_inner` (rebuilt on every `RolesChanged`). Roles are few (single/low-double digits), so this is negligible — but confirm the plan uses two bulk queries (not the current per-role N+1) to avoid a query per role on every reload.
- **Deny template for platform roles:** platform (non-admin) roles use `==`; a deny on a platform role therefore does not cascade into workspaces (mirrors the allow anti-leak). Confirm this is desired (it is — a platform deny should be exact-match, same as a platform allow).
- **"Blocked" role choice:** keyed to `ws_editor` so it denies read+manage. If a future object kind has an action outside the ws_editor closure, the block would miss it. Acceptable now (object actions are read/manage per kind); revisit if a kind adds a third action tier.
- **Migration number:** repo is past 0070; the plan picks the next free number (0071/0072) and touches `egret-metastore/src/lib.rs` to force a re-embed (known sqlx staleness gotcha).

---

*Design v1.0 (2026-07-11). Adds role hierarchy (single-parent, same-scope, transitive-closure expansion at template build) and explicit deny (deny-wins, `forbid` template per role, deny-aware admission), both converging on `CedarPdp::load_inner`. Deny-exceptions (the reverse direction) and multi-parent DAGs are deliberately out of scope.*
