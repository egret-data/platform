# Gateway P-C: catalog-scoped RBAC UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-polaris-gateway-machine-idp-design.md` (Pillar 3). **P-C of the gateway epic.** Depends on P-B (Catalog Client routes) + the existing P4/P5 catalog-role/grant routes. VI reference: `docs/vi/plans/2026-07-02-gw-pc-catalog-scoped-rbac-ui.md`.

**Goal:** Move RBAC into the catalog it belongs to. A **catalog detail page** (`/control-plane/catalog/{name}`) with Roles + Grants + Clients; remove the flat `/control-plane/rbac` page.

**Architecture:** React/shadcn. A new detail route reached from the catalogs table. Three sections backed by existing endpoints: catalog-roles (`GET/POST /polaris/catalogs/{c}/catalog-roles`), grants (`PUT /polaris/catalogs/{c}/catalog-roles/{cr}/grants`), and Catalog Clients (`GET/POST /polaris/catalogs/{c}/clients`, `DELETE /polaris/clients/{id}` from P-B). The old assign dialog + flat RBAC page are removed.

**Tech Stack:** TypeScript, React 18, shadcn, TanStack Query, Vitest.

---

## Background the implementer needs

- `frontend/src/routes/control-plane/ControlPlaneCatalog.tsx` (catalogs table + connection card) and `ControlPlaneRbac.tsx` (flat RBAC — to be removed) exist. `frontend/src/api/controlPlane.ts` has: `listCatalogRoles(catalog)`, `createCatalogRole(catalog,name)`, `addGrant(catalog,catalogRole,GrantBody)`, `listCatalogs`, etc.
- P-B adds the Catalog Client endpoints — add their client fns to `controlPlane.ts` if not already: `listClients(catalog)`, `createClient(catalog, {name,consumerType,workspaceId,catalogRole})→{...,clientId,clientSecret}`, `revokeClient(id)`.
- Routing: `ControlPlaneRoutes` in `App.tsx`; catalog routes are under `/control-plane/catalog`. Add `catalog/:name`. Nav in `Sidebar.tsx`.

---

## Task 1: API client for Catalog Clients + a catalog detail route

**Files:** Modify `frontend/src/api/controlPlane.ts`, `frontend/src/App.tsx`; Create `frontend/src/routes/control-plane/CatalogDetail.tsx`

- [ ] **Step 1: Client fns** — add `Client` type (`{id,name,consumerType,catalogRole,clientId?,createdAt,lastUsedAt}`), `ClientCreated` (adds `clientId,clientSecret`), and `listClients`/`createClient`/`revokeClient` per the P-B endpoints. `npm run typecheck`.

- [ ] **Step 2: Detail route + shell**

Add `<Route path="catalog/:name" element={<CatalogDetail />} />` under `ControlPlaneRoutes` (import at top). `CatalogDetail.tsx`: read `:name` via `useParams`; header shows the catalog name + a back link to `/control-plane/catalog`; a `Tabs` with "Roles & grants" and "Clients" (empty bodies for now). In `ControlPlaneCatalog.tsx` catalogs table, make the Name cell (or a "Manage" action) link to `/control-plane/catalog/{name}`.

- [ ] **Step 3: Failing test + commit**

`__tests__/CatalogDetail.test.tsx`: render with a route param `name=lakehouse`; assert the heading "lakehouse" + both tabs render. `npm run test -- CatalogDetail` PASS.
```bash
git add frontend/src/api/controlPlane.ts frontend/src/App.tsx frontend/src/routes/control-plane/CatalogDetail.tsx frontend/src/routes/control-plane/__tests__/ && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(ui): catalog detail route + Catalog Client API client"
```
(Co-Authored-By trailer.)

---

## Task 2: Roles & Grants section

**Files:** Modify `frontend/src/routes/control-plane/CatalogDetail.tsx`

- [ ] **Step 1: Failing test — lists roles for the catalog**

```tsx
it("lists catalog roles", async () => {
  vi.mocked(listCatalogRoles).mockResolvedValue([{ name: "writer" }]);
  renderDetail("lakehouse");
  await userEvent.click(screen.getByRole("tab", { name: /roles/i }));
  expect(await screen.findByText("writer")).toBeInTheDocument();
});
```

- [ ] **Step 2: Implement**

"Roles & grants" tab: a `Table` of catalog-roles (`listCatalogRoles(name)`); "Add role" dialog (`createCatalogRole(name, roleName)`). Per role, an "Add grant" dialog: securable `kind` (catalog/namespace/table/view) + identifier fields + `privilege` input → `addGrant(name, role, grantBody)`. (Grants write-only — Polaris has no list endpoint; note it, as in the old RBAC page.) Invalidate `listCatalogRoles` on create.

- [ ] **Step 3: Run + commit** — PASS.
```bash
git add frontend/src/routes/control-plane/ && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(ui): catalog detail — roles & grants section"
```

---

## Task 3: Clients section

**Files:** Modify `frontend/src/routes/control-plane/CatalogDetail.tsx`

- [ ] **Step 1: Failing test — lists clients + create shows secret once**

```tsx
it("lists clients and shows a new client's secret once", async () => {
  vi.mocked(listClients).mockResolvedValue([{ id:"1", name:"trino-eng", consumerType:"trino", catalogRole:"writer" }]);
  vi.mocked(createClient).mockResolvedValue({ id:"2", name:"nb", consumerType:"notebook", catalogRole:"reader", clientId:"egret_ab", clientSecret:"S3CR3T" });
  renderDetail("lakehouse");
  await userEvent.click(screen.getByRole("tab", { name: /clients/i }));
  expect(await screen.findByText("trino-eng")).toBeInTheDocument();
  // open Add, submit -> the returned clientId/clientSecret are shown once
});
```

- [ ] **Step 2: Implement**

"Clients" tab: `Table` (Name / Consumer / Role / Client ID / Last used / Actions) from `listClients(name)`. "Add client" dialog: name + `consumerType` Select (trino/starrocks/notebook/external) + workspace Select (`useMyWorkspaces`) + `catalogRole` Select (`listCatalogRoles(name)`) → `createClient(...)`; on success show `clientId`/`clientSecret` **once** in a copyable dialog with the "shown only once" warning (reuse the pattern from the old RBAC principal-creds dialog). Row "Revoke" → `revokeClient(id)` (confirm). Invalidate `listClients`.

- [ ] **Step 3: Run + commit** — PASS.
```bash
git add frontend/src/routes/control-plane/ && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(ui): catalog detail — Catalog Clients section (create once, revoke)"
```

---

## Task 4: Remove the flat RBAC page + old assign dialog

**Files:** Modify `frontend/src/App.tsx`, `frontend/src/components/layout/Sidebar.tsx`, `frontend/src/routes/control-plane/ControlPlaneCatalog.tsx`; delete `ControlPlaneRbac.tsx` + its test

- [ ] **Step 1: Remove**

`git rm frontend/src/routes/control-plane/ControlPlaneRbac.tsx frontend/src/routes/control-plane/__tests__/ControlPlaneRbac.test.tsx`. In `App.tsx`: remove the `rbac` route + `ControlPlaneRbac` import. In `Sidebar.tsx`: remove the "RBAC" nav item (+ update `Sidebar.test.tsx` to drop the RBAC assertion + not expect it). In `ControlPlaneCatalog.tsx`: remove the old `AssignCatalogDialog` + the "Assign…" row button (assignment is superseded — access is granted by creating a Client in the catalog detail). Keep the connection card + catalogs table + create/delete.

- [ ] **Step 2: Typecheck + lint + test**

`cd frontend && npm run typecheck && npm run lint && npm run test` → all pass (fix the Sidebar test's Control-Plane-items list to drop "RBAC"; keep Catalog/Storage/Secrets).

- [ ] **Step 3: Commit**
```bash
git add frontend/src && git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "refactor(ui): remove flat RBAC page + assign dialog (RBAC now per-catalog)"
```

---

## Task 5: Verify

- [ ] **Step 1:** `cd frontend && npm run typecheck && npm run lint && npm run test && npm run build` → clean/pass.
- [ ] **Step 2: Manual (stack up):** as platform admin, open a catalog → add a role + grant → add a Client (see secret once) → revoke it. No `/control-plane/rbac` route; no "Assign…" on the catalogs table.
- [ ] **Step 3:** commit any fixups.

---

## Self-review notes (spec coverage for P-C)

- Spec **Pillar 3**: catalog detail page with Roles+Grants (Task 2) + Clients (Task 3); flat `/rbac` + assign dialog removed (Task 4); global RBAC editing gone (access = creating a Client in the catalog).
- **Credential once:** the new Client's secret is shown once (Task 3), matching P-B (routes return it once).
- **Out of P-C:** the proxy (P-A), the IdP (P-B), Polaris trust + e2e (P-D).
