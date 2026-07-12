# AX-1 — Workspace Members Page — Design

**Status:** Approved (design via live review + published mockup)
**Date:** 2026-07-12
**Initiative:** Access-Control UX uplift (increment 1 of 5) — see
`docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`
**Mockup:** `scratchpad/workspace-members-mockup.html`

## Goal

Give each workspace its own **Members** admin surface so a workspace admin
(`ws_admin`) can manage who belongs to their workspace and at what role —
**without** needing a platform admin. This closes the structural gap in egret's
two-plane authorization model: the platform plane (Control Plane → Access control)
has a full UI, but the workspace plane has none, so today every workspace-role
assignment is forced through the platform-admin-only control plane.

## Motivation / current state

- The backend **already** exposes the member API — `GET/POST/DELETE
  /api/ws/{ws}/members`, all gated `workspace:manage_members` (so `ws_admin` and
  `platform_admin` can use them). There is **no frontend** for it.
- `MemberRepo::add` is a **replace-then-insert in one transaction** ("one workspace
  role per member: replace any existing workspace grant, then insert the new
  role"). So **changing a member's role = POST the member again with the new role**;
  no new endpoint is needed.
- `MemberRepo::list` returns only `{principal_type, principal_id, role}` — **no
  display name, no provenance**. The UI would show raw UUIDs.
- `MemberRepo::add` validates the role against a hard-coded const
  `WORKSPACE_ROLES = ["ws_admin","ws_editor","ws_viewer"]`, which **rejects custom
  workspace roles** (e.g. "Report Builder") even though the INSERT's `SELECT`
  already scopes to `r.scope = 'workspace'`.
- There is **no last-admin guard**: `remove_member` (or a demote via `add`) can
  strip a workspace of its last `ws_admin`, leaving it unmanageable without platform
  intervention.
- The frontend has **no workspace-settings shell** — only a standalone
  `notebook-settings` page. There is nowhere to host a workspace-admin surface.

## Design

### 1. Backend — `crates/egret-workspace`

**1a. Enrich the member DTO (`src/routes.rs` + `src/members.rs`).**
Extend `MemberRow` / `WorkspaceMember` / `MemberDto` and the `list` query to carry:

- `principal_name: Option<String>` — a user's email or a group's name, via a
  `LEFT JOIN` + `COALESCE(u.email::text, g.name)` (the exact pattern in
  `egret-bi/src/shares.rs::grant_list`).
- `granted_by_name: Option<String>` — who added them (resolve `access_grants.granted_by`
  → user email via LEFT JOIN).
- `added_at: DateTime<Utc>` — from `access_grants.created_at`.
- `is_platform_admin: bool` — true if this principal (or, for a user, none of the
  group path — keep it simple: the principal directly) holds the `platform_admin`
  role at platform scope. Computed with an `EXISTS` sub-select against
  `access_grants` + `roles`. Surfaces the "this person has superuser access
  regardless of membership" badge.

New `list` query (shape):

```sql
SELECT ag.principal_type, ag.principal_id, r.name AS role,
       COALESCE(u.email::text, g.name) AS principal_name,
       gb.email::text AS granted_by_name,
       ag.created_at AS added_at,
       EXISTS (SELECT 1 FROM access_grants pa JOIN roles pr ON pr.id = pa.role_id
               WHERE pa.principal_type = ag.principal_type AND pa.principal_id = ag.principal_id
                 AND pa.scope_type = 'platform' AND pr.name = 'platform_admin') AS is_platform_admin
FROM access_grants ag
JOIN roles r ON r.id = ag.role_id
LEFT JOIN users  u  ON ag.principal_type = 'user'  AND u.id  = ag.principal_id
LEFT JOIN groups g  ON ag.principal_type = 'group' AND g.id  = ag.principal_id
LEFT JOIN users  gb ON gb.id = ag.granted_by
WHERE ag.scope_type = 'workspace' AND ag.scope_id = $1
ORDER BY ag.created_at
```

**1b. Allow custom workspace roles (`src/members.rs::add`).**
Replace the `WORKSPACE_ROLES.contains(&role)` guard with a check that the role
exists at `scope = 'workspace'` (query the `roles` table). The INSERT's `SELECT …
WHERE r.scope = 'workspace' AND r.name = $5` already enforces scope; the guard just
needs to accept any workspace-scoped role name (seeded **or** custom) and reject
unknown / platform-scoped names with `InvalidInput`. Keep `WORKSPACE_ROLES` only
where a fixed default set is genuinely needed (if nowhere else, remove it).

**1c. Last-admin guard (`src/routes.rs` handlers + `src/members.rs`).**
Add `MemberRepo::count_ws_admins(ws) -> i64` (count members holding `ws_admin` in
the workspace). Before a **remove** or a **role change that would drop `ws_admin`**,
reject with `Error::Conflict("the workspace must keep at least one admin")` if this
is the last `ws_admin`. Wire it into both `remove_member` and `add_member` (the
latter only when the target already holds `ws_admin` and the new role isn't
`ws_admin`). This mirrors the existing "last platform admin" guard in
`egret-auth/src/roles.rs`.

**1d. No new endpoints.** Role change reuses `POST /members` (replace-then-insert).
`list_members` stays gated `workspace:manage_members` (admin-only view) — a possible
future relaxation to let any member view the roster is **out of scope** here.

### 2. Frontend

**2a. Workspace-settings shell (new).**
Add a workspace-scoped settings area under `/explore/:workspace/settings` with a
left sub-nav, hosting **Members** as the first tab (built now) and leaving room for
future tabs (Connections / Catalog / Storage). Route + shell gated on
`workspace:manage_members` (via the existing permission guard). Entry point: a
"Workspace settings" gear affordance in the workspace/explore nav, shown only to
holders of `workspace:manage_members`. If a general workspace nav/permission-gating
hook already exists, reuse it; otherwise add a minimal guard mirroring
`RequirePermission`.

**2b. Members page (`frontend/src/routes/explore/settings/Members.tsx`).**
Mirror the published mockup:

- **Header:** title "Members", subtitle naming the workspace, primary **Add member**
  action.
- **Scope clarifier callout:** "Workspace roles apply only inside {workspace}.
  Platform-wide access is granted separately in Control Plane → Access control."
- **Add-member picker:** reuse the `ShareDialog` `AddGrantRow` pattern (User/Group
  toggle + principal search via `listPrincipals` + role select + Add). Roles offered
  = workspace-scoped roles from `GET …/roles` filtered `scope = 'workspace'` (seeded
  + custom).
- **Toolbar:** a search box + Role filter + Type (user/group) filter.
- **Summary strip:** a compact one-line count ("N members · N admins · N people · N
  groups"), **not** oversized KPI cards.
- **Table:** columns Member (name + a `platform admin` badge when
  `is_platform_admin`), Type, **Role (inline `Select`)**, Added by, Added, and a `⋯`
  actions menu (Remove). Group rows get an expandable "N people via this group"
  (best-effort: if group membership isn't readily available, show the group name +
  type only and defer the expansion — note it).
- **Inline role change:** the per-row `Select` calls `POST /members` with the new
  role (replace-then-insert). Optimistic update + toast; revert on error.
- **Guardrails:** the sole `ws_admin`'s role `Select` is disabled with a
  "Last admin — role locked" hint; `Remove` is disabled for the last admin;
  removing shows a confirm dialog; changing/removing yourself shows a warning.

**2c. `FilterableTable` v1 (lean).**
Introduce a small reusable filterable-table primitive (search + sortable headers,
reusing the existing `SortableHeader`) that the Members table consumes. Keep it
minimal; AX-2 generalizes it across the control-plane pages.

**2d. API client (`frontend/src/api/…`).**
`useWorkspaceMembers(ws)` (list), `useAddMember(ws)` (POST — used for both add and
role change), `useRemoveMember(ws)`. Types match the enriched `MemberDto`.

### 3. Semantic colour

Introduce a `--success` token (a green distinct from the teal brand accent) for the
"admin" role swatch / state, so role/state colour is separate from the primary CTA
colour. (This is the small P3 token the roadmap's AX-5 also relies on; introducing
it here is fine.)

## Testing

**Backend**

- `list` returns enriched rows: a user member resolves to email, a group member to
  its name; `granted_by_name` + `added_at` populated; `is_platform_admin` true for a
  member who also holds `platform_admin`, false otherwise.
- `add` accepts a **custom** workspace role (create a workspace role "Report
  Builder", assign it — succeeds); rejects an unknown or platform-scoped role name.
- Role change: adding an existing member with a new role **replaces** (still one
  workspace grant; new role reflected) — no duplicate grant.
- **Last-admin guard:** removing the sole `ws_admin` → `Conflict`; demoting the sole
  `ws_admin` → `Conflict`; with two admins, removing one → OK.
- Cross-workspace isolation: `list`/`add`/`remove` only ever touch the path
  workspace's grants.

**Frontend (Vitest)**

- Members table renders enriched names (not UUIDs) + a `platform admin` badge for a
  platform-admin member.
- Inline role `Select` change calls the add mutation with the new role.
- The Add picker opens and submits a grant.
- The sole-admin row's role `Select` is disabled with the last-admin hint.
- The scope-clarifier callout renders.

**Security/UX review** — adversarial review before merge: `manage_members` gating on
every path; last-admin guard can't be bypassed via role change; custom-role
validation happens server-side (never trust the client's role string); cross-tenant
isolation; the settings shell/route is permission-gated.

## Out of scope

- The other workspace-settings tabs (Connections / Catalog / Storage) — only the
  shell + Members ship now.
- Effective-access viewer (**AX-3**).
- Generalizing `FilterableTable` to Users/Roles/Permissions (**AX-2**).
- Bulk member operations + undo (**AX-5**).
- Letting non-admins view the member roster (possible future relaxation of the
  `list_members` gate).
- Group-membership expansion if the data isn't already available cheaply (show group
  name/type; defer the "N people via this group" drill-in).

## Migration / rollout notes

- No schema migration — all changes are query/handler/DTO + frontend. (`--success`
  token is a CSS/token addition.)
- Behaviourally additive: a new page + enriched read; the only behaviour change is
  the last-admin guard (a new, safe refusal) and accepting custom workspace roles
  (a widening, not a break).
