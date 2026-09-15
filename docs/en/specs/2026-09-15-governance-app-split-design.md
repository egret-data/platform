# SP2 — Governance app split

**Status:** design, awaiting review
**Programme:** `docs/{en,vi}/specs/2026-09-15-control-plane-dg-review-programme.md`
**Delivery:** one frontend-only PR

## 1. Problem

Data Governance lives inside Control Plane, so a data steward has to enter the system-admin console
to manage policies, and the DG group also holds the audit log, which is a system log (API-layer
authorization decisions and admin mutations), not a data access log. Two copies of the Control
Plane permission list exist and both miss `tag:read`, so a user holding only tag permissions cannot
reach Classifications at all.

## 2. Decisions

- Data Governance becomes a top-level app at `/governance/*`, next to Control Plane in the header.
- The audit log stays in Control Plane, in a new **System** group, platform_admin only (unchanged).
- Backend API paths are unchanged (`/api/control-plane/policies*`, `/tags*`, `/audit`): no
  regeneration of the API client, no Cedar or permission change.

## 3. Design

### 3.1 Permission lists — one module

New `frontend/src/auth/appAccess.ts`:

```ts
export const CONTROL_PLANE_PERMS = [
  "engine:read", "warehouse:read", "storage:read", "secret:read",
  "user:read", "group:read", "role:read", "workspace:read",
] as const;
export const GOVERNANCE_PERMS = ["policy:read", "tag:read"] as const;
export function canEnterControlPlane(me): boolean; // platform_admin || any CONTROL_PLANE_PERMS
export function canEnterGovernance(me): boolean;   // platform_admin || any GOVERNANCE_PERMS
```

`Header.tsx` and `guards.tsx` import from it; their local copies are deleted.

### 3.2 Header

`Explorer | Governance | Control Plane`. Governance is shown when `canEnterGovernance`, Control
Plane when `canEnterControlPlane`. Active state follows the URL prefix.

### 3.3 Sidebar

`Sidebar.tsx` gains a third mode selected by `pathname.startsWith("/governance")`, with
`GOVERNANCE_GROUPS`:

| Group | Item | Route | perm |
|---|---|---|---|
| Policies | Policies | `/governance/policies` | `policy:read` |
| Classification | Classifications | `/governance/classifications` | `tag:read` |
| Lineage | Data lineage | `/governance/data-lineage` | platform_admin (placeholder, as today) |

Filtering is the existing rule (platform_admin, or holds `perm`). `CONTROL_PLANE_GROUPS` loses the
"Data governance" group and gains:

| Group | Item | Route | perm |
|---|---|---|---|
| System | Audit log | `/control-plane/audit-log` | platform_admin (no perm, as today) |

Future items (not rendered in SP2): Governance → *Data permissions* (SP4), *Data access log* (SP4/SP5).

### 3.4 Routes

`App.tsx`:

- New `GovernanceRoutes` under `/governance/*`, wrapped in `RequireGovernanceAccess`
  (`canEnterGovernance`, else the existing forbidden page):
  - `policies`, `policies/:service`, `policies/:service/:kind/new`, `policies/:service/:kind/:id` — `RequirePermission perm="policy:read"`
  - `classifications`, `classifications/:id` — `tag:read`
  - `data-lineage` — `RequireGlobalAdmin` (placeholder)
  - index → first section the user may see (`policy:read` → policies, `tag:read` → classifications)
- Removed from `ControlPlaneRoutes`: the same seven routes. `audit-log` stays.
- **Redirects** (inside `ControlPlaneRoutes`): `policies/*`, `classifications/*`, `data-lineage` →
  the same subpath under `/governance`, preserving `search` and `hash` (e.g. `?scope=tag`), with
  `replace`.
- `FIRST_SECTION_BY_PERM` (Control Plane landing) drops `policy:read`; a user with only governance
  permissions who opens `/control-plane` is redirected to `/governance`.

### 3.5 Pages and links

- Move files: `routes/control-plane/{Policies,ProfileDetailPage,PolicyEditorPage}.tsx` and
  `routes/control-plane/governance/*` → `routes/governance/`, with their tests.
- Update every in-app link to the new paths. Known call sites: `Policies.tsx`,
  `ProfileDetailPage.tsx`, `PolicyEditorPage.tsx`, `governance/Classifications.tsx`,
  `governance/TagDetail.tsx`, `access/AccessOverview.tsx`, `components/layout/__tests__/Sidebar.test.tsx`,
  and any link from `WarehouseDetail` / `EngineDetail` to a profile or tag. The planner greps
  `"/control-plane/(policies|classifications|data-lineage)"` excluding `api/` and generated code
  (`frontend/src/api/policies.ts` uses the unchanged backend path and must not be touched).
- `AuditLog.tsx` docstring: "Control Plane → System → Audit log".

## 4. Testing

- Unit: `appAccess` (admin, steward-only, infra-only, tag-only users); Header shows the right
  entries; Sidebar governance mode filtering; each redirect preserves subpath and query;
  `RequireGovernanceAccess` allow/deny; Control Plane landing for a governance-only user.
- Existing policy/classification page tests pass from their new location.
- Frontend gate: `npm run lint`, typecheck, `npm test`, `npm run build`.
- **Live in the browser** with two accounts: platform_admin sees all three header entries and the
  System → Audit log item; a user whose only platform role is `data_steward` reaches Governance
  (policies and classifications), does not see Audit log, and in Control Plane sees only the
  read-only sections that role already holds. An old bookmark
  `/control-plane/policies/<svc>/<kind>/new?scope=tag` lands on the governance editor.

## 5. Out of scope

- Recording the missing audit events (logins; engine/warehouse/storage/secret/policy config
  changes) — recommended follow-up so the audit log is a complete system log.
- Moving backend routes to `/api/governance/*`.
- Any change to how policies or tags work (SP4).
