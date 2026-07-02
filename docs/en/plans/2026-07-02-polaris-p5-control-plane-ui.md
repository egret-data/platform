# Control-Plane Polaris — Plan 5: control-plane UI (catalog + RBAC + assignment) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P5 of the epic.** Consumes the P4 routes (`/api/control-plane/polaris/*`, assignments) and the P2 Storage/Secrets pages. VI reference: `docs/vi/plans/2026-07-02-polaris-p5-control-plane-ui.md`.

**Goal:** Global-admin UI under `/control-plane` to register the Polaris connection, manage catalogs (create on a storage connection / list / delete), manage RBAC (principals / roles / grants), and assign catalogs to workspaces — following the existing `ControlPlaneEngines` frame + shadcn primitives.

**Architecture:** React 18 + shadcn (forwardRef primitives) + TanStack Query. New pages under `frontend/src/routes/control-plane/`, wired into `ControlPlaneRoutes` (`App.tsx`, `RequireGlobalAdmin`) + the `Sidebar` "Control Plane" section. All data via a `controlPlane.ts` API client (extended from P2). Verified with Vitest + typecheck/build.

**Tech Stack:** TypeScript, React 18, shadcn/ui, TanStack Query, Vitest + Testing Library.

---

## Background the implementer needs

- **Routing:** `App.tsx` `ControlPlaneRoutes()` (wrapped in `RequireGlobalAdmin` + `AppShell`) has `workspaces`/`engines`/`users` routes. Add `catalog`, `storage`, `secrets` (storage/secrets pages exist from P2). `Sidebar.tsx` "Control Plane" section (heading ~line 65) lists nav items — add Catalog/Storage/Secrets.
- **Frame pattern:** `frontend/src/routes/control-plane/ControlPlaneEngines.tsx` (type tabs, table with header cols, empty state, "Add" button) + its `__tests__/`. shadcn: `Table`, `Card`, `Tabs`, `Button`, `Dialog`, `Select`, `Input`, `Badge` under `@/components/ui/*`. Primitives are forwardRef (React 18, Radix `asChild`).
- **API base:** `apiJson`/`apiFetch` in `@/api/client` (adds CSRF for mutations, `credentials: include`). P2 created `@/api/controlPlane.ts` with storage/secrets calls — extend it.
- **P4 endpoints:** `GET/PUT /api/control-plane/polaris/connection` (+ `/health`); `GET/POST /polaris/catalogs`, `DELETE /polaris/catalogs/{name}`; `GET/POST /polaris/principals` (create shows `clientSecret` once), `DELETE /{name}`; `GET/POST /polaris/principal-roles`; `GET/POST /polaris/catalogs/{c}/catalog-roles`; `PUT /polaris/principals/{p}/principal-roles`, `PUT /polaris/principal-roles/{pr}/catalog-roles/{c}`; `PUT /polaris/catalogs/{c}/catalog-roles/{cr}/grants`; `POST/DELETE /api/control-plane/workspaces/{ws}/catalog-assignments`.
- Frontend checks: `npm run typecheck`, `npm run lint`, `npm run build`, `npm run test` (Vitest).

---

## Task 1: API client + types

**Files:** Modify `frontend/src/api/controlPlane.ts`

- [ ] **Step 1: Add types + calls**

```ts
// --- Polaris connection ---
export type PolarisConnection = { baseUrl: string; kcTokenUrl: string; kcClientId: string; kcScope?: string };
export const getPolarisConnection = () => apiJson<PolarisConnection | null>("/control-plane/polaris/connection");
export const putPolarisConnection = (b: PolarisConnection & { kcClientSecret: string }) =>
  apiJson<PolarisConnection>("/control-plane/polaris/connection", { method: "PUT", body: JSON.stringify(b) });
export const polarisHealth = () => apiJson<{ ok: boolean }>("/control-plane/polaris/connection/health");

// --- Catalogs ---
export type Catalog = { name: string; type: "INTERNAL" | "EXTERNAL"; storage?: string };
export const listCatalogs = () => apiJson<Catalog[]>("/control-plane/polaris/catalogs");
export const createCatalog = (b: { name: string; storageConnection: string }) =>
  apiJson<Catalog>("/control-plane/polaris/catalogs", { method: "POST", body: JSON.stringify(b) });
export const deleteCatalog = (name: string) => apiFetch(`/control-plane/polaris/catalogs/${name}`, { method: "DELETE" });

// --- RBAC ---
export type Principal = { name: string };
export type PrincipalWithCreds = { principal: Principal; credentials: { clientId: string; clientSecret: string } };
export const listPrincipals = () => apiJson<Principal[]>("/control-plane/polaris/principals");
export const createPrincipal = (name: string) => apiJson<PrincipalWithCreds>("/control-plane/polaris/principals", { method: "POST", body: JSON.stringify({ name }) });
export const listPrincipalRoles = () => apiJson<{ name: string }[]>("/control-plane/polaris/principal-roles");
export const createPrincipalRole = (name: string) => apiJson<{ name: string }>("/control-plane/polaris/principal-roles", { method: "POST", body: JSON.stringify({ name }) });
export const listCatalogRoles = (c: string) => apiJson<{ name: string }[]>(`/control-plane/polaris/catalogs/${c}/catalog-roles`);
export const createCatalogRole = (c: string, name: string) => apiJson<{ name: string }>(`/control-plane/polaris/catalogs/${c}/catalog-roles`, { method: "POST", body: JSON.stringify({ name }) });
export const addGrant = (c: string, cr: string, grant: Record<string, unknown>) => apiFetch(`/control-plane/polaris/catalogs/${c}/catalog-roles/${cr}/grants`, { method: "PUT", body: JSON.stringify(grant) });

// --- Assignment ---
export const listAssignments = (ws: string) => apiJson<{ catalogName: string }[]>(`/control-plane/workspaces/${ws}/catalog-assignments`);
export const assignCatalog = (ws: string, catalog: string) => apiJson(`/control-plane/workspaces/${ws}/catalog-assignments`, { method: "POST", body: JSON.stringify({ catalog }) });
export const unassignCatalog = (ws: string, catalog: string) => apiFetch(`/control-plane/workspaces/${ws}/catalog-assignments/${catalog}`, { method: "DELETE" });
```

- [ ] **Step 2: Typecheck** — `cd frontend && npm run typecheck` (clean).

- [ ] **Step 3: Commit**
```bash
git add frontend/src/api/controlPlane.ts
git commit -m "feat(ui): control-plane Polaris API client (connection/catalogs/rbac/assignment)"
```

---

## Task 2: Polaris connection page

**Files:** Create `frontend/src/routes/control-plane/ControlPlaneCatalog.tsx` (connection card + catalogs table live here); reuse for Task 3.

- [ ] **Step 1: Failing component test**

```tsx
// __tests__/ControlPlaneCatalog.test.tsx — renders "Connect Polaris" when getPolarisConnection returns null
it("prompts to connect when no Polaris connection", async () => {
  vi.mocked(getPolarisConnection).mockResolvedValue(null);
  render(<ControlPlaneCatalog />, { wrapper: qcWrapper });
  expect(await screen.findByText(/connect polaris/i)).toBeInTheDocument();
});
```

- [ ] **Step 2: Implement the connection card**

A `Card` at the top of `ControlPlaneCatalog`: if `getPolarisConnection()` is `null`, show a "Connect Polaris" form (baseUrl, kcTokenUrl, kcClientId, kcClientSecret, kcScope) → `putPolarisConnection`; else show the connection (baseUrl) + a live "Health" badge (`polarisHealth()` → green/red) + an "Edit" button. TanStack Query for fetch + a mutation that invalidates the query.

- [ ] **Step 3: Run + commit** — `npm run test -- ControlPlaneCatalog` (PASS).
```bash
git add frontend/src/routes/control-plane/ControlPlaneCatalog.tsx frontend/src/routes/control-plane/__tests__/
git commit -m "feat(ui): control-plane Polaris connection card + health"
```

---

## Task 3: Catalogs table (list / create / delete / assign)

**Files:** Modify `frontend/src/routes/control-plane/ControlPlaneCatalog.tsx`

- [ ] **Step 1: Failing test — lists catalogs**

```tsx
it("renders catalogs with a storage column", async () => {
  vi.mocked(getPolarisConnection).mockResolvedValue({ baseUrl:"http://p", kcTokenUrl:"", kcClientId:"" });
  vi.mocked(listCatalogs).mockResolvedValue([{ name:"lakehouse", type:"INTERNAL", storage:"minio" }]);
  render(<ControlPlaneCatalog />, { wrapper: qcWrapper });
  expect(await screen.findByText("lakehouse")).toBeInTheDocument();
});
```

- [ ] **Step 2: Implement the table + dialogs**

Below the connection card: a `Table` (cols Name / Type / Storage / Workspaces / Actions) fed by `listCatalogs()`. "Add catalog" `Dialog`: Name + a `Select` of storage connections (`listStorage()` from P2's client) → `createCatalog({name, storageConnection})`. Per-row: "Delete" (`deleteCatalog`) + "Assign…" (opens the Task 4 assignment dialog). Mutations invalidate `listCatalogs`. Empty state mirrors `ControlPlaneEngines`.

- [ ] **Step 3: Run + commit** — test PASS.
```bash
git add frontend/src/routes/control-plane/
git commit -m "feat(ui): control-plane catalogs table (create on storage / list / delete)"
```

---

## Task 4: Workspace assignment dialog

**Files:** Create `frontend/src/routes/control-plane/AssignCatalogDialog.tsx`; use it from the catalogs table

- [ ] **Step 1: Failing test — assign posts + lists**

```tsx
it("assigns a catalog to a workspace", async () => {
  vi.mocked(useMyWorkspaces).mockReturnValue({ data: [{ slug:"eng", name:"Engineering", id:"w1" }] } as any);
  render(<AssignCatalogDialog catalog="lakehouse" open onOpenChange={()=>{}} />, { wrapper: qcWrapper });
  await userEvent.click(screen.getByRole("button", { name: /assign/i }));
  expect(vi.mocked(assignCatalog)).toHaveBeenCalledWith("w1", "lakehouse");
});
```

- [ ] **Step 2: Implement**

A `Dialog` listing workspaces (from a workspaces list endpoint / `useMyWorkspaces` or a control-plane workspaces list) with an assign/unassign toggle per workspace for the given catalog; calls `assignCatalog(ws, catalog)` / `unassignCatalog`. Shows currently-assigned workspaces (`listAssignments` per ws, or a reverse lookup). Invalidate on change.

- [ ] **Step 3: Run + commit** — PASS.
```bash
git add frontend/src/routes/control-plane/
git commit -m "feat(ui): assign catalog to workspace dialog"
```

---

## Task 5: RBAC page (principals / roles / grants)

**Files:** Create `frontend/src/routes/control-plane/ControlPlaneRbac.tsx`

- [ ] **Step 1: Failing test — lists principals**

```tsx
it("lists principals", async () => {
  vi.mocked(listPrincipals).mockResolvedValue([{ name: "egret-ws-w1" }]);
  render(<ControlPlaneRbac />, { wrapper: qcWrapper });
  expect(await screen.findByText("egret-ws-w1")).toBeInTheDocument();
});
```

- [ ] **Step 2: Implement (tabbed)**

`Tabs`: **Principals** (table + "Add" → `createPrincipal`; on create show the returned `clientId`/`clientSecret` **once** in a copyable dialog with a "shown only once" warning; "Delete"), **Principal Roles** (list/create + assign-to-principal), **Catalog Roles** (pick a catalog → list/create its roles + assign a principal-role), **Grants** (pick catalog + catalog-role → add a grant: securable type [catalog/namespace/table] + identifier + a privilege `Select`; → `addGrant`). Each tab is a table + a dialog, mirroring the catalogs table.

- [ ] **Step 3: Run + commit** — PASS.
```bash
git add frontend/src/routes/control-plane/
git commit -m "feat(ui): control-plane RBAC page (principals/roles/grants)"
```

---

## Task 6: Nav + routes + verify

**Files:** Modify `frontend/src/App.tsx`, `frontend/src/components/layout/Sidebar.tsx`

- [ ] **Step 1: Routes** — in `ControlPlaneRoutes` add:

```tsx
<Route path="catalog" element={<ControlPlaneCatalog />} />
<Route path="rbac" element={<ControlPlaneRbac />} />
<Route path="storage" element={<ControlPlaneStorage />} />   {/* from P2 */}
<Route path="secrets" element={<ControlPlaneSecrets />} />   {/* from P2 */}
```
(import the pages at top).

- [ ] **Step 2: Nav** — in `Sidebar.tsx` "Control Plane" section add items: Catalog (`/control-plane/catalog`), RBAC (`/control-plane/rbac`), Storage (`/control-plane/storage`), Secrets (`/control-plane/secrets`), with lucide icons.

- [ ] **Step 3: Verify**

Run: `cd frontend && npm run typecheck && npm run lint && npm run test && npm run build`
Expected: all clean/pass.

- [ ] **Step 4: Manual (with backend up)** — as a platform admin: connect Polaris, create a catalog on the `minio` storage, assign it to `eng`, add a principal + a table grant. As a non-admin, `/control-plane/*` is not reachable (`RequireGlobalAdmin`).

- [ ] **Step 5: Commit**
```bash
git add frontend/src/App.tsx frontend/src/components/layout/Sidebar.tsx
git commit -m "feat(ui): wire control-plane Catalog/RBAC/Storage/Secrets routes + nav"
```

---

## Self-review notes (spec coverage for P5)

- Spec **E. Frontend (control plane)**: connection (Task 2), catalogs table + create-on-storage + delete (Task 3), assignment (Task 4), RBAC principals/roles/grants (Task 5), Storage/Secrets pages wired (Task 6; pages built in P2), all under `RequireGlobalAdmin`.
- **Credential display:** a created principal's `clientSecret` is shown **once** (Task 5) — matches P4 (routes return it once, never stored by the RBAC routes).
- **Out of P5:** Polaris docker-compose + e2e + the Trino/StarRocks connector wiring that consumes the assignment credential (P6); the future K8s-provisioning epic.
