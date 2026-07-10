# B2-1a — Object-level authorization (backend) for BI content

> **Status:** Design (approved to plan) · **Date:** 2026-07-10 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) §2 (B2). First increment of **B2 (object-level scoping)**.
>
> **Prerequisite:** B1b complete — all enforcement flows through the Cedar PDP; `access_grants` has the reserved `scope_type='object'` / `object_type` / `scope_id` columns.

---

## 1. Goal & scope

Deliver **object-level authorization** for BI content — Metabase-style collection sharing where a user/group can be granted a role on a **folder** (collection) or an **individual item** (dashboard/chart/dataset), and permission resolves along the hierarchy **Item → Folder → Workspace → Platform**. This increment is the **backend**: the PDP object-scope model, the grant store + share API, and the BI per-object handler migration. The sharing **UI** is B2-1b.

**In scope:** `Resource::Object`; the Cedar entity hierarchy + the per-role-scope template operator fix; `AccessGrantStore` object-grant reads; object-grant write/revoke/list APIs; migrating the BI per-object handlers (dashboard/chart/dataset) from workspace-scope to object-scope checks; tests.
**Out of scope:** the sharing UI (B2-1b); object-scoping other resource types — notebooks, saved-queries, warehouses (B2-2); role hierarchy + explicit deny / allow-exceptions (B2-3); the deferred data-model flip (drop legacy grant tables). Folders are **flat** today (no nesting) — folder→folder nesting is out of scope.

**Sharing roles:** reuse the existing workspace roles — `ws_viewer` (→ `dashboard:read`/`chart:read`/`dataset:read`) and `ws_editor` (→ `*:manage`). The share API exposes them as "Viewer"/"Editor". No new role types.

## 2. The core model fix: template operator by role scope

B1b-2 made every non-admin role template use `resource == ?resource` (exact match) to stop a platform-scoped grant from cascading into workspaces. But object inheritance (a **folder** grant must cover **items in the folder**) needs `resource in ?resource`. Resolve this by making the operator depend on the **role's scope** (which `template_for_role(role_name, scope, perms)` already receives):

| Role scope | Template body | Effect |
|---|---|---|
| `platform` (non-admin) | `permit(principal in ?principal, action in [perms], resource == ?resource)` | **no cascade** — a platform grant matches only `Platform` (unchanged from B1b-2; anti-leak preserved) |
| `workspace` (non-admin) | `permit(principal in ?principal, action in [perms], resource in ?resource)` | **cascade down** — a workspace grant covers objects in the workspace; a folder grant covers items in the folder; an object grant covers that object (reflexive `in`) |
| `platform_admin` | `permit(principal in ?principal, action, resource in ?resource)` | unrestricted (unchanged) |

Why this is safe:
- **No cross-workspace leak:** a workspace grant links `?resource = Workspace::"W1"`; `resource in Workspace::"W1"` is true only for resources whose ancestor chain contains W1 (objects/folders in W1). A resource in W2 is not `in` W1. ✓
- **No platform→object leak:** platform roles keep `==`, so a platform grant never matches a workspace/object resource. ✓ (Independently, platform-scope and workspace-scope permission namespaces are disjoint — B1b-2 review.)
- **Object grants use workspace roles** (`ws_viewer`/`ws_editor`), so they get `in` and cascade correctly (folder→items; item→itself).
- **The 60-cell regression matrix is unaffected:** it issues only `Platform` and `Workspace(id)` requests; for a direct `Workspace(W1)` request, `resource in Workspace::"W1"` (reflexive) equals `resource == Workspace::"W1"`. The `in`/`==` difference manifests only for object requests, which the matrix does not contain. Re-run it — it must stay 60/60.

## 3. Cedar entity model

Add a single generic **`Object`** entity type (avoids declaring one Cedar type per resource kind). Ids are namespaced by kind: `Egret::Object::"dashboard:<uuid>"`, `Egret::Object::"folder:<uuid>"`, etc. The request resource's ancestor chain is built in `build_entities`:

```
Egret::Object::"dashboard:<id>"   parent → Egret::Object::"folder:<fid>"   (if folder_id set)
                                  parent → Egret::Workspace::"<wsid>"      (if folder_id NULL)
Egret::Object::"folder:<fid>"     parent → Egret::Workspace::"<wsid>"
Egret::Workspace::"<wsid>"        parent → Egret::Platform::"root"
```
So `resource in Egret::Object::"folder:<fid>"` is true for the folder and its items; `resource in Egret::Workspace::"<wsid>"` is true for everything in the workspace; `resource in Egret::Platform::"root"` matches all (admin path). Grants link `?resource` to the grant's scope entity: `Object::"<kind>:<id>"` for object grants, `Workspace::"<id>"` for workspace grants, `Platform::"root"` for platform grants.

## 4. `Resource::Object` + the `require_object` PEP

`egret_core::authz::Resource` gains a variant carrying the ancestry the PDP needs (so the PDP never queries object stores):
```rust
pub enum Resource {
    Platform,
    Workspace(WorkspaceId),
    Object(ObjectRef),
}
pub struct ObjectRef {
    pub kind: String,                 // "dashboard" | "chart" | "dataset" | "folder"
    pub id: Uuid,
    pub folder_id: Option<Uuid>,      // the item's folder (None = Uncategorized)
    pub workspace_id: WorkspaceId,
}
```
A new PEP alongside `require`:
```rust
pub async fn require_object(
    pdp: &dyn Pdp,
    ctx: &WorkspaceContext,           // subject + the request's workspace (sanity: must equal object.workspace_id)
    action: &str,
    object: ObjectRef,
) -> Result<()>;
```
The handler resolves `ObjectRef` from the object it is operating on (it already loads the row, or does a cheap `SELECT folder_id, workspace_id FROM <table> WHERE id=$1` ancestry lookup before authorizing). `build_entities` uses the `ObjectRef` fields to construct the chain in §3; `authorize` passes `Resource::Object` through. `AuthTarget` gains no new impl — object requests go through `require_object`, not the generic `require` (which stays for platform/workspace routes).

## 5. Grant store: object grants

- **Writes** — object grants are new rows in `access_grants`: `principal_type/principal_id, role_id, scope_type='object', object_type=<kind>, scope_id=<object id>`. (No legacy table, no trigger — these are net-new writes directly to `access_grants`, independent of the deferred data-model flip.) The `access_grants` `UNIQUE (principal_type, principal_id, role_id, scope_type, scope_id)` already accommodates them.
- **Reads** — `AccessGrantStore::grants_for` is extended to also return the caller's **object grants whose scope is an ancestor of the request resource**. New shape:
  ```rust
  pub async fn grants_for(&self, user_id, ws: Option<WorkspaceId>, object_scope_ids: &[Uuid]) -> Result<Vec<Grant>>;
  ```
  where `object_scope_ids = [object.id, folder_id?]` (the object itself + its folder). SQL adds `OR (ag.scope_type = 'object' AND ag.scope_id = ANY($3))`. `Grant` gains `object_type`. `link_grants` binds `?resource` to `Object::"<object_type>:<scope_id>"` for object grants (Workspace/Platform unchanged).

## 6. Share API (write/revoke/list)

New workspace-scoped endpoints (in the BI seam) to manage object grants — the backend the B2-1b UI drives:
- `PUT /ws/{ws}/objects/{kind}/{id}/grants` — grant `(principal, role)` on the object (folder or item). Gated by `require_object(pdp, ctx, "<kind>:manage", object)` — i.e. only someone who can manage the object can share it (Metabase "curate" semantics).
- `DELETE /ws/{ws}/objects/{kind}/{id}/grants/{grant_id}` — revoke.
- `GET /ws/{ws}/objects/{kind}/{id}/grants` — list who has access (direct grants on the object; the UI separately shows inherited).
Writing/removing an object grant publishes `RolesChanged`? No — `RolesChanged` is for role/permission *definitions* (template rebuild). Object grants are *assignments*, read live per request by `grants_for` (like workspace_members), so **no event needed** — the next request sees them immediately.

## 7. BI per-object handler migration

Today the BI handlers check `require(&ctx, "dashboard:read")` at **workspace** scope. Migrate the handlers that operate on a **specific** object to object scope:
- Handlers with a `{id}` path param for a dashboard/chart/dataset → `require_object(&*st.pdp, &ctx, "dashboard:read"|"dashboard:manage"|…, object_ref).await?`, resolving `object_ref` (folder_id + workspace_id) for that id first.
- Handlers that are **workspace-level** (list all dashboards, create a new one with no parent object) stay `require(&ctx, PERM)` at workspace scope — a workspace grant still authorizes them (and, via §2, also covers object reads for members).
- **Behavior preservation:** a workspace member with `ws_editor` still manages every dashboard in the workspace (their workspace grant cascades to objects via `in`). Object grants *add* access for principals who are **not** workspace members (or have a lower workspace role) — the new capability. Existing workspace-member behavior is unchanged.

## 8. Testing

- **Regression matrix:** `pdp_regression` stays 60/60 (§2).
- **Object-scope unit/integration tests (new):**
  - A folder grant of `ws_viewer` to user U (not a workspace member) → U can `dashboard:read` an item **in that folder** (inherited), and **cannot** read an item in a different folder / Uncategorized.
  - An item grant of `ws_editor` to U → U can `dashboard:manage` **that** dashboard only, not a sibling.
  - A workspace grant still covers all objects (member behavior unchanged).
  - A platform (non-admin) grant does **not** authorize any object action (anti-leak).
  - `platform_admin` manages any object (admin path).
  - Share API: manage-capable user grants/revokes; a viewer cannot grant (403 on the share endpoint).
- Full local gate: fmt, clippy, `cargo test --workspace`, frontend `lint/typecheck/test/build` (the share API changes the OpenAPI → regenerate the FE client; no FE UI yet).

## 9. Open questions (for the plan)

- Exact set of BI per-object handlers to migrate (enumerate the `{id}`-bearing dashboard/chart/dataset routes in `egret-bi/src/routes.rs`); confirm which read vs manage action each uses.
- `ObjectRef` resolution: reuse the object row the handler already loads, vs a dedicated ancestry-lookup query — pick per handler (avoid a double fetch where the row is already loaded).
- Whether "Uncategorized" (folder_id NULL) items are covered only by workspace grants (yes — no folder to grant on); confirm the UI/spec messaging for B2-1b.
- Whether the share endpoints live in the BI seam (`egret-bi`) or a new small `egret-sharing` surface — default: BI seam, since the objects are BI content.
- Role list for sharing: `ws_viewer`/`ws_editor` only, or also `ws_admin`? Default Viewer/Editor; revisit for a "Manager" tier in B2-3.

---

*Design v1.0 (2026-07-10). Backend object-level authorization for BI content (folders + items, per-item + inherited), reusing workspace roles at object scope. The `in`/`==` tension from B1b-2 is resolved by keying the template operator on the role's scope. Sharing UI is B2-1b; other object types B2-2; role-hierarchy/deny B2-3; the legacy-table flip remains deferred.*
