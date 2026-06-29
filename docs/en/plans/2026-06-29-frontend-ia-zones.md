# Frontend IA Zones + Control Plane Scaffold — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Frontend-only scaffolding. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Reshape the sidebar into workspace zones (Explore / Data Engineering / Data Governance / Manage) + rename the Admin area to **Control Plane** at `/control-plane`, with placeholder pages for all new items and a structured (no-backend) **Resources** frame.

**Tech Stack:** React 18 + react-router v6 + Vitest + shadcn (`Card`, `Tabs`, `Button`, `Table`) + lucide.

**Spec:** `docs/en/specs/2026-06-29-frontend-ia-zones-design.md`.

---

## Background facts (verified)
- `components/layout/Sidebar.tsx`: `NavItem = {label, icon}`, `NavGroup = {heading, items}`; `EXPLORE_GROUPS` / `ADMIN_GROUPS`; `slug = s.toLowerCase().replace(/\s+/g,"-")`; `isAdmin = useLocation().pathname.startsWith("/admin")`; `base = isAdmin ? "/admin" : "/explore/{workspace}"`; items link to `${base}/${slug(label)}`.
- `App.tsx`: `ExploreRoutes` (`/explore/:workspace/*`), `AdminRoutes` (`/admin/*`); admin pages imported from `routes/admin/*` and are all `<Placeholder title=… />`; `<Route path="/admin/*" element={<RequireAuth><AdminRoutes/></RequireAuth>} />`.
- `routes/Placeholder.tsx`: `Placeholder({title})` — a "later phase" frame.
- `components/layout/Header.tsx`: `isAdmin = pathname.startsWith("/admin")`; in Explorer, when `canAdmin`, renders `<Button asChild><Link to="/admin"><Settings/>Admin</Link></Button>`; in Admin, an "Explorer" button. `routes/admin/{Catalog,Policies,Users,Workspaces,Monitoring}.tsx` are one-line `<Placeholder>`.
- `components/layout/__tests__/PortalToggle.test.tsx`: drives `initialPath: "/admin"` and asserts link name `/admin/i` / button `/explorer/i`.

---

## Task 1: Rename `/admin` → `/control-plane`

**Files:** `App.tsx`, `components/layout/Header.tsx`, `components/layout/Sidebar.tsx`, `components/layout/__tests__/PortalToggle.test.tsx`; delete `routes/admin/*.tsx`.

### Step 1: Update the PortalToggle test (drives the rename)
In `PortalToggle.test.tsx`: replace every `initialPath: "/admin"` with `"/control-plane"`; the Explorer-portal test's `getByRole("link", { name: /admin/i })` → `{ name: /control plane/i }`; the not-present assertions likewise. (The "Explorer" button name is unchanged.) Run it → fails (Header still says "Admin"/`/admin`).

### Step 2: Header.tsx
- `isAdmin = location.pathname.startsWith("/control-plane")`.
- The Explorer→admin button: `<Link to="/control-plane">` with label **"Control Plane"** (icon `Settings` kept).

### Step 3: Sidebar.tsx
- `isAdmin = useLocation().pathname.startsWith("/control-plane")`.
- `base = isAdmin ? "/control-plane" : \`/explore/${workspace ?? ""}\``.
(The group set + heading are replaced in Task 2.)

### Step 4: App.tsx — route + group component
- Rename `AdminRoutes` → `ControlPlaneRoutes`; mount at `<Route path="/control-plane/*" element={<RequireAuth><ControlPlaneRoutes/></RequireAuth>} />`.
- Remove the `routes/admin/*` imports; the control-plane pages become inline `<Placeholder>` / `<ControlPlaneResources/>` (Task 3).
- Delete the now-unused `routes/admin/{Catalog,Policies,Users,Workspaces,Monitoring}.tsx`.

### Step 5: Run the test → PASS; commit
`cd frontend && npx vitest run src/components/layout/__tests__/PortalToggle.test.tsx`
```bash
git commit -m "refactor(frontend): rename Admin area to Control Plane (/control-plane)"
```

---

## Task 2: Sidebar zones

**Files:** `components/layout/Sidebar.tsx`.

### Step 1: Replace the group definitions
```tsx
import {
  Activity, BadgeCheck, BarChart3, Bell, Boxes, Code, Database, FolderKanban,
  HardDrive, KeyRound, LayoutDashboard, type LucideIcon, NotebookText,
  PanelLeftClose, PanelLeftOpen, Plug, ScrollText, Server, Share2, ShieldCheck,
  SquareTerminal, Table2, Users, Waypoints, Workflow,
} from "lucide-react";

const EXPLORE_GROUPS: NavGroup[] = [
  {
    heading: "Explore",
    items: [
      { label: "SQL Lab", icon: SquareTerminal },
      { label: "Datasets", icon: Table2 },
      { label: "Charts", icon: BarChart3 },
      { label: "Dashboards", icon: LayoutDashboard },
      { label: "Notebooks", icon: NotebookText },
      { label: "Alerts", icon: Bell },
    ],
  },
  {
    heading: "Data Engineering",
    items: [
      { label: "Data API", icon: Plug },
      { label: "Workflows", icon: Workflow },
      { label: "Pipelines", icon: Waypoints },
      { label: "Code IDE", icon: Code },
    ],
  },
  {
    heading: "Data Governance",
    items: [
      { label: "Policies", icon: ShieldCheck },
      { label: "Data Lineage", icon: Share2 },
      { label: "Query Audit", icon: ScrollText },
      { label: "Data Quality", icon: BadgeCheck },
    ],
  },
  {
    heading: "Manage",
    items: [
      { label: "Catalog", icon: Boxes },
      { label: "Storage", icon: HardDrive },
      { label: "Databases", icon: Database },
      { label: "Secrets", icon: KeyRound },
    ],
  },
];

const CONTROL_PLANE_GROUPS: NavGroup[] = [
  {
    heading: "Control Plane",
    items: [
      { label: "Workspaces", icon: FolderKanban },
      { label: "Resources", icon: Server },
      { label: "Monitoring", icon: Activity },
      { label: "Users", icon: Users },
      { label: "Platform Policies", icon: ShieldCheck },
      { label: "Catalog", icon: Boxes },
    ],
  },
];
```
- `const groups = isAdmin ? CONTROL_PLANE_GROUPS : EXPLORE_GROUPS;` (rename `ADMIN_GROUPS` → `CONTROL_PLANE_GROUPS`).
- Everything else (render, slug, collapse) unchanged. The slug fn already yields `data-api`, `code-ide`, `data-lineage`, `query-audit`, `data-quality`, `platform-policies`.

### Step 2: typecheck + commit
`cd frontend && npm run typecheck`
```bash
git commit -m "feat(frontend): workspace zones (Data Engineering, Data Governance, Alerts) + Control Plane group"
```

---

## Task 3: Routes + the Resources frame

**Files:** `App.tsx`; create `routes/control-plane/ControlPlaneResources.tsx`.

### Step 1: ExploreRoutes — add placeholder routes
In `ExploreRoutes`, after the existing routes, add (import `Placeholder` from `@/routes/Placeholder`):
```tsx
<Route path="alerts" element={<Placeholder title="Alerts" />} />
<Route path="data-api" element={<Placeholder title="Data API" />} />
<Route path="workflows" element={<Placeholder title="Workflows" />} />
<Route path="pipelines" element={<Placeholder title="Pipelines" />} />
<Route path="code-ide" element={<Placeholder title="Code IDE" />} />
<Route path="policies" element={<Placeholder title="Policies" />} />
<Route path="data-lineage" element={<Placeholder title="Data Lineage" />} />
<Route path="query-audit" element={<Placeholder title="Query Audit" />} />
<Route path="data-quality" element={<Placeholder title="Data Quality" />} />
```
(The existing `*` → `sql-lab` fallback stays last.)

### Step 2: ControlPlaneRoutes — inline placeholders + the Resources frame
```tsx
function ControlPlaneRoutes() {
  return (
    <AppShell>
      <Routes>
        <Route index element={<Navigate to="workspaces" replace />} />
        <Route path="workspaces" element={<Placeholder title="Workspaces" />} />
        <Route path="resources" element={<ControlPlaneResources />} />
        <Route path="monitoring" element={<Placeholder title="Monitoring" />} />
        <Route path="users" element={<Placeholder title="Users" />} />
        <Route path="platform-policies" element={<Placeholder title="Platform Policies" />} />
        <Route path="catalog" element={<Placeholder title="Catalog" />} />
        <Route path="*" element={<Navigate to="workspaces" replace />} />
      </Routes>
    </AppShell>
  );
}
```
(Match the existing `AdminRoutes` wrapper shape — reuse whatever layout wrapper it used.)

### Step 3: Create `routes/control-plane/ControlPlaneResources.tsx` (the frame)
A no-backend structured frame: page header with title + a stub **Add resource** button; **Tabs** for `Trino` / `StarRocks` / `Spark Connect`; each tab shows a table header (`Name · Type · Endpoint · Status · Workspaces · Actions`) and an **empty state** row ("No <type> resources yet — connect one in a later phase."). Use shadcn `Card`, `Tabs`/`TabsList`/`TabsTrigger`/`TabsContent`, `Button` (the Add button `disabled` with a tooltip/title "Coming soon"), and the lucide `Server`/`Plus` icons. No data fetching, no `@/api` calls. Keep it visually consistent with the existing MANAGE pages (e.g. `DatabasesPage`/`StoragePage` header + table styling). Add `data-testid="resources-frame"` on the root and accessible tab names.

### Step 4: typecheck + lint + commit
`cd frontend && npm run typecheck && npx eslint src/App.tsx src/routes/control-plane`
```bash
git commit -m "feat(frontend): placeholder routes for new zones + Control Plane Resources frame"
```

---

## Task 4: Tests

**Files:** create `components/layout/__tests__/Sidebar.test.tsx` (if absent) + `routes/control-plane/__tests__/ControlPlaneResources.test.tsx`.

### Step 1: Sidebar zone test
Render `<Sidebar collapsed={false} .../>` inside a `MemoryRouter`:
- at `/explore/eng/sql-lab`: assert the four zone headings (`Explore`, `Data Engineering`, `Data Governance`, `Manage`) and representative items (`Alerts`, `Data API`, `Workflows`, `Pipelines`, `Code IDE`, `Policies`, `Data Lineage`, `Query Audit`, `Data Quality`) are present, and a link's `href` is `/explore/eng/data-api` etc.
- at `/control-plane/workspaces`: assert the `Control Plane` heading + items (`Workspaces`, `Resources`, `Platform Policies`) with `/control-plane/...` hrefs.

### Step 2: Resources frame test
Render `<ControlPlaneResources/>`: assert the three tab names (`Trino`, `StarRocks`, `Spark Connect`), the empty state, and the (disabled) Add resource button.

### Step 3: Full FE verify + commit
`cd frontend`: `npm run lint` · `npm run typecheck` · `npx vitest run` · `npm run build` — all clean (PortalToggle + any sidebar/route tests green).
```bash
git commit -m "test(frontend): sidebar zones + Control Plane Resources frame"
```

---

## Final verification (before merge)
1. **Full FE CI**: `npm run lint` · `npm run typecheck` · `npx vitest run` · `npm run build` — clean.
2. **Live smoke** (FE hot-reloads): log in; the workspace sidebar shows Explore / Data Engineering / Data Governance / Manage with all items; clicking a new item shows its placeholder; the header "Control Plane" button → `/control-plane` shows the Control Plane group; Resources shows the tabbed frame; back to Explorer works. No console errors.
3. **Admin squash-merge** (identity `hoangquocvietbk2.13@gmail.com`).
4. **Memory**: note the new IA in `[[egret-three-layer-management-model]]` (zones + /control-plane rename + the placeholders pending).

---

## Self-review
- **Spec coverage:** workspace zones + Alerts (Task 2) ✓; /admin→/control-plane rename (Task 1) ✓; Control Plane group + Platform Policies (Task 2) ✓; placeholder routes (Task 3) ✓; Resources frame (Task 3) ✓; Policies in both layers (workspace `policies` + control-plane `platform-policies`) ✓; tests (Task 4) ✓. Out-of-scope features not built. ✓
- **Placeholders:** the *pages* are intentional placeholders (the deliverable); the plan code itself has none.
- **Type/route consistency:** slugs from `slug(label)` match the `<Route path>` strings (`data-api`, `code-ide`, `data-lineage`, `query-audit`, `data-quality`, `platform-policies`); Sidebar `base`/`isAdmin` use `/control-plane` matching App.tsx's route + Header's detection.
