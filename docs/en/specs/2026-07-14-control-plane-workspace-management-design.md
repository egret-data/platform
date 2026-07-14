# Control-Plane Workspace Management — Design

- **Date:** 2026-07-14
- **Status:** Approved (design), pending implementation plan
- **Scope:** Frontend-primary feature with four supporting backend additions
- **Layer:** Control plane (`/control-plane/*`), gated `RequireGlobalAdmin` (platform_admin)

## Problem

The control-plane sidebar lists **Workspaces**, but the route is the sole remaining
placeholder among real CP pages:

```tsx
<Route path="workspaces" element={<RequireGlobalAdmin><Placeholder title="Workspaces" /></RequireGlobalAdmin>} />
```

Every other CP surface (engines, warehouses, storage, secrets, users, groups, roles,
permissions, access-overview, audit-log) is a built page. A platform admin has no UI to
create, browse, rename, archive, or inspect workspaces platform-wide, even though the
backend lifecycle API already exists.

## Current state

### Backend — already present

`admin_workspace_router` (`crates/egret-workspace/src/routes.rs`) exposes the full
lifecycle:

| Method | Path | Handler | Authz |
|---|---|---|---|
| POST | `/api/admin/workspaces` | `create_ws` | `workspace:create` |
| GET | `/api/admin/workspaces` | `list_ws` (paginated) | `workspace:read` |
| GET | `/api/admin/workspaces/{id}` | `get_ws` | `workspace:read` |
| PUT | `/api/admin/workspaces/{id}` | `update_ws` (name only) | `workspace:update` |
| DELETE | `/api/admin/workspaces/{id}` | `archive_ws` (soft) | `workspace:archive` |

Supporting APIs reusable by the cockpit (all `/ws/{ws}/…`; `platform_admin` bypasses the
per-workspace `workspace:manage_members` gate):

- Members: `GET/POST /ws/{ws}/members`, `DELETE /ws/{ws}/members/{ptype}/{pid}`.
  `add_member` already performs an **upsert / role-replace with last-admin protection**,
  so add / change-role / remove need no new backend.
- Access requests: `GET /ws/{ws}/access-requests` + `.../{id}/approve` + `.../{id}/deny`.
- Audit: the control-plane audit API accepts a `workspace: Option<Uuid>` filter, so
  per-workspace activity is already queryable.
- Resource lists: `/ws/{ws}/catalog/warehouses`, `/ws/{ws}/storages`, `/ws/{ws}/databases`
  (engines), `/ws/{ws}/secrets`.

The `workspaces` table (`migrations/0003_workspace_rbac.sql`) has columns the DTO does not
yet surface:

```sql
CREATE TABLE workspaces (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slug text NOT NULL UNIQUE,
    name text NOT NULL,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
    settings jsonb NOT NULL DEFAULT '{}',
    created_by uuid REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now()
);
```

`WorkspaceDto` currently exposes only `{ id, slug, name, status }`. The repo
(`crates/egret-workspace/src/workspaces.rs`) has `create`, `update_name`, `archive` — but
**no `set_status`/`restore`**.

### Frontend — the gap

- CP route is a `<Placeholder>`.
- Generated `admin-workspaces` client has only `useAdminWorkspacesListWs` +
  `useAdminWorkspacesGetWs` (no create/update/delete hooks).
- `routes/explore/settings/Members.tsx` is a full members-management page keyed on
  `useParams().workspace` — the reuse target for the detail page's Members tab.
- CP list pages follow a `SummaryStrip` + `FilterableTable` convention
  (`routes/control-plane/access/UsersPage.tsx`, `ControlPlaneEngines.tsx`).

## Design

### Information architecture

Two surfaces, both under `RequireGlobalAdmin`.

**List — `/control-plane/workspaces`**

- `SummaryStrip`: total / active / archived counts.
- `FilterableTable`: columns Slug · Name · Owner · Created · Status · ⋮.
  Text filter + status facet.
- Toolbar: **+ New workspace** (dialog: slug + name; slug validated
  lowercase/digits/hyphen per `model::validate` rules).
- Row menu (`⋮`): Open · Rename · Archive — on archived rows, **Restore** replaces Archive.

**Detail — `/control-plane/workspaces/:id`**

Header: name, `slug` mono chip, status badge, action buttons
(Rename · Archive/Restore · **Transfer owner**). Five tabs:

1. **Overview** — owner name, created date, status, and a stat row sourced from the usage
   endpoint: members · warehouses · storage connections · engines · queries · storage GB.
2. **Members** — renders the extracted `<MembersPanel workspaceId={id} />`. Full add /
   change-role / remove; server enforces the last-admin guard.
3. **Resources** — read-only counts with deep-links into the existing CP Warehouses /
   Storage / Engines / Secrets pages scoped to this workspace. No CRUD here (those pages
   own it).
4. **Access requests** — pending join requests, Approve / Deny via the existing API.
   Empty state when none pending.
5. **Activity** — audit log filtered by `?workspace={id}`, reusing `AuditLog.tsx`'s table
   rendering.

### Backend changes

| # | Change | Detail |
|---|---|---|
| B1 | **Restore (un-archive)** | Add `workspaces.set_status(id, status)`. Extend `PUT /api/admin/workspaces/{id}` (`UpdateWorkspace`) with an optional `status` field; when present and `= "active"`, reactivate. Reuse the `workspace:archive` permission (state transition), or introduce `workspace:restore` if finer control is wanted — **decision: reuse `workspace:archive`** to avoid a permission-catalog migration. |
| B2 | **Owner + created** | Extend `model::Workspace` and `WorkspaceDto` with `created_at`, `created_by`, `owner_name`. Update the `list` and `get_by_id` queries to `LEFT JOIN users ON workspaces.created_by = users.id` and select the display name. Null-tolerant (`created_by` is nullable). |
| B3 | **Usage stats** | New `GET /api/admin/workspaces/{id}/stats`, gated `workspace:read`, returning `{ member_count, warehouse_count, storage_conn_count, engine_count, query_count, storage_bytes }`. `member_count`/`warehouse_count`/`storage_conn_count`/`engine_count` are COUNT(*) over the respective workspace-scoped tables. `query_count` = COUNT over `queries` for the workspace. `storage_bytes` = sum of object-store size across the workspace's storage connections (best-effort; degrade to `null` per-connection on probe failure rather than failing the whole call). |
| B4 | **Ownership transfer** | New `POST /api/admin/workspaces/{id}/transfer { new_owner_id }`, gated `workspace:update`. If the new owner is not already a member, add them as `ws_admin`; set `workspaces.created_by = new_owner_id`; write an audit event (`workspace.transfer_owner`). Returns the updated `WorkspaceDto`. |

All new/changed routes carry `utoipa::path` annotations so the orval client regenerates
cleanly.

### Frontend build

1. **Regenerate** the `admin-workspaces` client from the updated OpenAPI — adds
   create / update / archive / restore / stats / transfer hooks.
2. **Refactor** `Members.tsx`: extract the body into a reusable
   `components/access/MembersPanel.tsx` taking an explicit `workspaceId` prop; the existing
   explore page becomes a thin wrapper passing `useParams().workspace`. The CP detail
   Members tab renders the same panel. Behaviour and tests preserved.
3. **New pages:**
   - `routes/control-plane/Workspaces.tsx` — list (SummaryStrip + FilterableTable +
     create/rename/archive/restore dialogs).
   - `routes/control-plane/WorkspaceDetail.tsx` — tabbed detail + transfer-owner dialog.
4. **Wire `App.tsx`:** replace the placeholder route; add `workspaces/:id`; both under
   `RequireGlobalAdmin`.
5. Follow OKLCH design tokens (`var(--token)` in inline styles), the SummaryStrip /
   FilterableTable pattern, and shadcn primitives.

### Data flow

```
List:   useAdminWorkspacesListWs ─▶ SummaryStrip + FilterableTable
        create/rename/archive/restore mutations ─▶ invalidate list query

Detail: useAdminWorkspacesGetWs(id)      ─▶ header + Overview
        useAdminWorkspacesStats(id)      ─▶ Overview stat row
        useWorkspaceMembersListMembers   ─▶ Members tab (via MembersPanel)
        list/approve/deny access-requests ─▶ Access requests tab
        audit(workspace=id)              ─▶ Activity tab
        transfer mutation ─▶ invalidate get + list
```

## Testing

**Backend**

- Repo test: `set_status` round-trip (archive → restore).
- Route tests: `PUT` with `status:"active"` restores; `WorkspaceDto` now carries
  `created_at`/`created_by`/`owner_name`; `stats` returns correct counts on a seeded
  workspace; `transfer` sets owner, auto-adds a non-member as `ws_admin`, and is audited.
- Edge: transfer to an existing member does not duplicate membership; stats tolerate a
  storage connection whose object store is unreachable.

**Frontend**

- Vitest for `Workspaces.tsx`: renders rows from a mocked list, create/rename/archive/
  restore flows fire the right mutations, status facet filters.
- Vitest for `WorkspaceDetail.tsx`: tab switching, overview stats render, transfer dialog
  submits.
- `MembersPanel` refactor: existing `Members` tests continue to pass unchanged.

**Manual**

- Live smoke against the dev stack as `admin@egret.local` (platform_admin): create a
  workspace, open detail, add/remove a member, view stats, transfer owner, archive,
  restore. This is manual Playwright-MCP driving, **not** an automated Playwright test.

## Rollout

Single PR (backend routes + DTO + orval regen + FE pages + tests), or split
backend-then-FE if smaller reviews are preferred. Local full CI (fmt + clippy + workspace
tests, and FE lint/typecheck/test/build) must pass before any admin squash-merge, per the
project's run-CI-locally rule.

## Out of scope / future

- Per-workspace CRUD of warehouses/storage/engines/secrets from the detail page (those CP
  pages already own it; the Resources tab only deep-links).
- Hard delete of a workspace (archive is the terminal state).
- Historical usage trends / charts (stats are point-in-time).
- Bulk workspace operations.
