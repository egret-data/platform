# EB-5: warehouse + catalog-scoped RBAC UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-egret-embedded-catalog-design.md` (Component 5). **EB-5.** Depends on EB-4 (mgmt routes). VI: `docs/vi/plans/2026-07-02-eb5-warehouse-rbac-ui.md`.

**Goal:** Control-plane UI for the embedded catalog: a Warehouses list + a Warehouse detail page (roles by consumer-type + grants + Catalog Clients). Remove the Polaris-era pages (connection card, Polaris catalogs table, flat RBAC page).

**Architecture:** React/shadcn under `/control-plane`, on EB-4's routes. Reuse the `ControlPlaneEngines`/`ControlPlaneStorage` frame + the once-shown-credential dialog pattern.

**Tech Stack:** TypeScript, React 18, shadcn, TanStack Query, Vitest.

---

## Background the implementer needs

- Merged UI (#115): `ControlPlaneCatalog.tsx` (Polaris connection + catalogs), `ControlPlaneRbac.tsx` (flat RBAC), `CatalogDetail.tsx` (if EB gateway landed — it did not; ignore). Remove/replace the Polaris ones.
- `frontend/src/api/controlPlane.ts` has Polaris catalog/rbac fns (remove) + storage/secrets (keep). Add the EB-4 warehouse/role/grant/client fns.
- `useMyWorkspaces` hook lists workspaces (for warehouse/client create).

---

## Task 1: API client (warehouses / roles / grants / clients)

**Files:** Modify `frontend/src/api/controlPlane.ts`

- [ ] **Step 1:** Add types + fns: `Warehouse`, `listWarehouses`/`createWarehouse({name,workspaceId,storageConnectionId})`/`deleteWarehouse`; `Role`, `listRoles(whId)`/`createRole(whId,{name,consumerType})`; `Grant`, `listGrants(roleId)`/`addGrant(roleId,GrantInput)`/`revokeGrant(grantId)`; `Client`/`ClientCreated`, `listClients(whId)`/`createClient(whId,{name,consumerType,workspaceId,catalogRoleId})`/`revokeClient(id)`. Remove the Polaris `polarisConnection`/`catalogs`/`principals`/`assignments` fns. `npm run typecheck`.
- [ ] **Step 2: Commit** — `feat(ui): embedded-catalog API client (warehouses/roles/grants/clients)`.

---

## Task 2: Warehouses list page

**Files:** Create `frontend/src/routes/control-plane/Warehouses.tsx`

- [ ] **Step 1: Failing test** — `listWarehouses` → `[{id,name}]` renders; empty state otherwise.
- [ ] **Step 2:** `Table` (Name / Workspace / Storage / Actions) from `listWarehouses`; "Add warehouse" dialog (name + workspace Select + storage-connection Select from `listStorage`) → `createWarehouse`; row → link to `/control-plane/warehouses/{id}` + Delete. Mirror `ControlPlaneStorage`.
- [ ] **Step 3: Commit** — `feat(ui): warehouses list page`.

---

## Task 3: Warehouse detail (roles-by-consumer-type + grants + clients)

**Files:** Create `frontend/src/routes/control-plane/WarehouseDetail.tsx`

- [ ] **Step 1: Failing test** — renders the warehouse name + tabs Roles / Clients; `listRoles` row shows a role + its `consumerType`.
- [ ] **Step 2:** `useParams` warehouse id; `Tabs`:
  - **Roles & grants:** `listRoles(id)` table (Name / Consumer type); "Add role" (name + consumerType Select engine/notebook/external); per role "Add grant" (securable kind + identifier + privilege → `addGrant`) + `listGrants` table with revoke.
  - **Clients:** `listClients(id)` table (Name / Consumer / Role / Client ID / Last used); "Add client" (name + consumerType + workspace + role Select from `listRoles`) → `createClient` → show `clientId`/`clientSecret` **once** (copyable + warning); row Revoke.
- [ ] **Step 3: Commit** — `feat(ui): warehouse detail — roles/grants + clients`.

---

## Task 4: Nav + routes; remove Polaris-era pages

**Files:** `frontend/src/App.tsx`, `Sidebar.tsx`; delete Polaris pages

- [ ] **Step 1:** In `ControlPlaneRoutes`: add `warehouses` + `warehouses/:id` routes; remove `catalog`/`rbac` routes + imports. `git rm ControlPlaneCatalog.tsx ControlPlaneRbac.tsx` (+ tests). In `Sidebar.tsx`: replace "Catalog"/"RBAC" nav items with a single "Warehouses" (`/control-plane/warehouses`); update `Sidebar.test.tsx`.
- [ ] **Step 2:** `npm run typecheck && npm run lint && npm run test` — fix the Sidebar test's Control-Plane items list.
- [ ] **Step 3: Commit** — `refactor(ui): warehouses/RBAC pages replace Polaris catalog/RBAC pages`.

---

## Task 5: Verify

- [ ] `cd frontend && npm run typecheck && npm run lint && npm run test && npm run build`. Manual (stack up): create a warehouse → add a role + grant → add a client (secret once) → revoke.

---

## Self-review notes (spec coverage for EB-5)

- Spec **Component 5**: warehouses list (Task 2) + warehouse detail with roles-by-consumer-type + grants + clients (Task 3); Polaris-era pages removed (Task 4); once-shown client secret (Task 3).
- **Out of EB-5:** e2e + engine wiring (EB-6).
