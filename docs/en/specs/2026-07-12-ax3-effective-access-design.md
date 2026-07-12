# AX-3 — Effective-Access Viewer — Design

**Status:** Approved
**Date:** 2026-07-12
**Initiative:** Access-Control UX uplift (increment 2 of 5, pulled up from #3) — see
`docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`

## Goal

Turn the Access-Control console from a set of lists into an **answer tool**: pick any
user or group and see, in one place, **what they can actually do, where, and why** —
the roles they hold (direct or via a group), their effective permissions (with the
role/parent each one comes from), their per-workspace memberships, the objects shared
with them, and any explicit denies. Today an admin has to mentally join roles +
groups + role-hierarchy + object grants + deny to answer "why can Alice see this?";
this feature computes and explains it.

## Why this shape

Cedar's `authorize` is a *per-decision* API (allow/deny for one action on one
resource); it cannot enumerate "everything a principal can do". So effective access is
**computed directly from the data** (`access_grants` + `roles` + `role_permissions` +
`user_groups` + the object tables) — deterministic and enumerable, with provenance.
This extends exactly what `egret-auth/src/session.rs::permissions()` already does (a
recursive role-hierarchy closure) by additionally tracking *where each grant comes
from*. A live "can X do action A on resource R?" **simulator** (which would use Cedar
`Decision.reasons`) is deliberately **out of scope** here.

## Design

### 1. Backend — `AccessResolver` (in `egret-auth`)

A new read-only resolver that, given a principal `(type ∈ {user,group}, id)`, returns
a structured `EffectiveAccess`:

```
EffectiveAccess {
  principal:      { type, id, name, is_platform_admin },
  platform_roles: [ { name, source } ],                 // source: "direct" | "group:{groupName}"
  permissions:    [ { key, group, via: [ { role, inherited_from } ] } ],  // inherited_from: parent role name | null
  workspaces:     [ { workspace: {id, slug, name}, role, source } ],
  objects:        [ { kind, id, name, workspace_slug, role, effect, source } ],  // effect: "allow" | "deny"
  denies:         [ { scope, target, role, source } ],  // explicit effect='deny' grants, surfaced separately
}
```

Resolution rules:

- **Principals feeding a user's access:** the user's own grants **plus** grants held
  by every group in `user_groups`. Each resolved role/membership/object carries a
  `source`: `"direct"` or `"group:{groupName}"`.
- **Group principal:** egret groups don't nest (only `user_groups`: user→group), so a
  group resolves to its **own** direct grants only — no "via group" layer.
- **platform_roles:** `access_grants` at `scope_type='platform'` for the principal (+
  its groups), joined to `roles`.
- **permissions (the provenance-bearing part):** for each granted platform role, walk
  its **parent chain** (role-hierarchy closure, same as `session.rs`) and collect
  `role_permissions`. Each effective permission records every path that grants it:
  `{ role: <the granted role>, inherited_from: <parent role name if the permission came
  from a parent, else null> }`. Group permissions by the catalog group (from the
  permission catalog) for display.
- **workspaces:** `access_grants` at `scope_type='workspace'` (+ groups) → per
  workspace: the role name + source. Resolve `scope_id` → `{id, slug, name}`.
- **objects:** `access_grants` at `scope_type='object'` (+ groups) → `object_type`
  (kind), `scope_id` (id) resolved to the object's `name` + owning workspace (via the
  kind→table map already in `egret-bi/src/shares.rs`), the role, and `effect`.
- **denies:** any resolved grant with `effect='deny'` is also collected into a separate
  `denies` list so a "Blocked" state reads at a glance (deny-wins is already how the
  PDP behaves; this just surfaces it).
- **platform_admin:** if the principal holds `platform_admin` at platform scope, set
  `is_platform_admin=true`; the UI shows "Superuser — bypasses all permission checks"
  instead of enumerating permissions (mirrors the role editor's treatment). The
  resolver still returns the roles/workspaces/objects, but `permissions` may be shown
  collapsed under the superuser note.

### 2. Backend — endpoint

`GET /api/control-plane/access/effective?principal_type={user|group}&principal_id={id}`
→ `200 EffectiveAccess`.

- **Gating:** `role:read` (the permission that already gates the Roles / Permissions
  pages; `platform_admin` bypasses). This endpoint reveals a principal's complete
  access graph, so it must not be broader than the console's authz-graph read.
- `principal_id` accepts a UUID or (like AX-1's member add) a user email / group name,
  resolved server-side — so the picker can pass what it has. Unknown principal → 404.
- Wired into the control-plane access router alongside the existing users/roles
  endpoints; exposed through the orval-generated client.

### 3. Frontend — "Access explorer" page

- **Nav:** a new "Access explorer" item under **Access control** (after Permissions),
  gated `role:read` (same as Roles/Permissions).
- **Route:** `/control-plane/access/effective`, reading `?user={id}` / `?group={id}`
  from the URL to preselect (deep-linkable, shareable).
- **Principal picker** at the top (reuse `listPrincipals` — the same picker used
  elsewhere): choose a user or group; changing it refetches.
- **Sections** (each rendered only when non-empty):
  1. **Header:** principal name + type badge + an `is_platform_admin` "Superuser"
     badge (with the bypass note) when applicable.
  2. **Platform roles:** chips, each annotated with its `source` (direct / via group).
  3. **Effective permissions:** grouped by catalog group; each permission shows its
     provenance — "via {role}", and "inherited from {parent}" when it came up the
     hierarchy. Use `FilterableTable` (from AX-1) so a large permission set is
     searchable/sortable.
  4. **Workspaces:** per-workspace role + source.
  5. **Shared objects:** kind + name + workspace + role + effect.
  6. **Denies / Blocked:** explicit deny grants, visually highlighted
     (`--destructive`), so a block reads at a glance.
- **Deep-link entry:** the Users and Groups pages gain a row action **"View access"**
  linking to `/control-plane/access/effective?user={id}` (or `?group=`), so the viewer
  is reachable in context.
- Read-only (no mutations). Loading / empty / error / forbidden states handled like the
  other access pages.

### 4. Data flow

Picker (or URL param) selects a principal → one `GET …/effective` call → render the
sections. No writes.

## Testing

**Backend (`AccessResolver`, real Postgres)**

- A user with a **direct** platform role + a role held **via a group** → both appear
  with the correct `source`; effective permissions include both roles' permissions.
- **Hierarchy provenance:** a role whose parent grants permission P → P appears with
  `inherited_from = <parent>`; a directly-granted permission has `inherited_from =
  null`.
- **Workspaces:** a user who is `ws_editor` of workspace A (direct) and `ws_viewer` of
  B via a group → both memberships with correct source.
- **Objects + deny:** an object grant surfaces in `objects`; an `effect='deny'` grant
  surfaces in `denies`.
- **Group principal:** resolves the group's own grants; no "via group" layer.
- **platform_admin:** `is_platform_admin=true` and the superuser path is taken.
- **Principal id resolution:** an email/group-name resolves; an unknown principal → 404.

**Frontend (Vitest)**

- The page renders each section from a mocked `EffectiveAccess`, including permission
  provenance ("inherited from …") and a highlighted deny.
- `?user={id}` preselects the principal (deep-link).
- The picker changing the principal refetches.
- The superuser badge renders for a platform-admin principal.

**Security/UX review** before merge: the endpoint is gated `role:read` and never
leaks more than the authz graph a role-reader may already see; no data-plane/Ranger
info; cross-principal enumeration is intended (it's an admin tool) but must respect the
gate; principal-id resolution is server-side.

## Out of scope

- **Access simulator** ("can principal X do action A on resource R?" → live PDP
  `Decision` + Cedar `reasons`) — a strong follow-up, deferred.
- **Data-plane / Ranger** effective data access (what tables/rows they can query) — a
  separate governance subsystem, not part of the egret authz graph.
- **Group nesting** — egret has none (`user_groups` is user→group only).
- Editing from this viewer — it is read-only; changes happen on the Users/Roles/Members
  pages.

## Notes

- No schema migration — pure read/resolver + a new endpoint + a new page.
- The resolver is the reusable core; keep it a focused unit in `egret-auth` with a
  clear `EffectiveAccess` return type, testable independently of the HTTP layer.
- Reuse: `FilterableTable` (AX-1), `listPrincipals` picker, the kind→table object map
  in `egret-bi/src/shares.rs`, and the role-hierarchy closure logic from
  `session.rs::permissions()`.
