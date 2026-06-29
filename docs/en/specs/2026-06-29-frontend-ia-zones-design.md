# Frontend IA — workspace zones + Control Plane scaffold — Design

**Date:** 2026-06-29 · **Status:** Design — approved · **Branch:** `feat/frontend-ia-zones` (off main)
**Program:** Frontend information architecture. Reshapes the sidebar into purpose zones and scaffolds (placeholder) the future Data-Engineering / Data-Governance / Control-Plane surfaces, per the three-layer model ([[egret-three-layer-management-model]]).

## Goal
Restructure the navigation into clear zones and stand up the **frames** (mostly placeholder pages) for the next wave of features, without implementing those features. Rename the platform "Admin" area to **Control Plane**.

## Revision (shipped, 2026-06-29) — trimmed + collapsible
After live review the sidebar was too long, so the workspace zones were trimmed and a new **Automate** zone added; each group heading is now a **collapsible** chevron button (default expanded; collapsed headings persisted in `localStorage` under `egret:sidebar-collapsed-groups`). Final workspace zones:
- **Explore**: SQL Lab · Datasets · Charts · Dashboards · Notebooks
- **Data Engineering**: Data API · Pipelines
- **Automate** *(new — mainly for analysts)*: Workflows · Alerts
- **Data Governance**: Policies · Data Lineage
- **Manage**: Catalog · Storage · Databases · Secrets

**Dropped** (not built): Code IDE, Query Audit, Data Quality (+ their routes). **Alerts** moved Explore→Automate; **Workflows** moved Data Engineering→Automate. Control Plane is unchanged from §2 below. The §1/§Routes/§Icons sections below describe the first cut; this revision supersedes their item lists.

## Background (current state)
- `components/layout/Sidebar.tsx`: two `NavGroup[]` — `EXPLORE_GROUPS` (workspace: **Explore** = SQL Lab/Datasets/Charts/Dashboards/Notebooks; **Manage** = Catalog/Storage/Databases/Secrets) and `ADMIN_GROUPS` (heading "Administration": Catalog/Policies/Users/Workspaces/Monitoring). `isAdmin = pathname.startsWith("/admin")` picks the group set + base (`/admin` vs `/explore/{ws}`). Items route to `${base}/${slug(label)}`.
- `App.tsx`: `ExploreRoutes` (`/explore/:workspace/*`) and `AdminRoutes` (`/admin/*`); admin pages live in `routes/admin/*` and are `<Placeholder title=… />`.
- `routes/Placeholder.tsx`: a "This feature arrives in a later phase" frame (`{ title }`).
- `components/layout/Header.tsx`: portal toggle — `isAdmin = pathname.startsWith("/admin")`, a `<Link to="/admin">` labelled "Admin". `__tests__/PortalToggle.test.tsx` drives it with `/admin` paths + an "admin" link name.

## Decisions

### 1. Workspace sidebar (`/explore/:ws/*`) — four zones
- **Explore**: SQL Lab · Datasets · Charts · Dashboards · Notebooks · **Alerts** *(new, placeholder)*.
- **Data Engineering** *(new zone)*: **Data API** · **Workflows** (n8n) · **Pipelines** (Airflow) · **Code IDE** (git) — all placeholders.
- **Data Governance** *(new zone)*: **Policies** · **Data Lineage** · **Query Audit** · **Data Quality** — all placeholders.
- **Manage** *(unchanged)*: Catalog · Storage · Databases · Secrets.

### 2. Control Plane (`/control-plane/*`, renamed from `/admin`)
Heading **"Control Plane"**. Items: **Workspaces** *(existing)* · **Resources** *(new, structured frame)* · **Monitoring** *(existing)* · **Users** *(existing)* · **Platform Policies** *(existing "Policies", renamed)* · **Catalog** *(existing overview)*.

### 3. Rename `/admin` → `/control-plane` (everywhere)
- `App.tsx`: route `/admin/*` → `/control-plane/*`; `AdminRoutes` → `ControlPlaneRoutes`; index/`*` redirect target stays the first item (Workspaces).
- Move `routes/admin/` → `routes/control-plane/` (Catalog, Monitoring, Users, Workspaces; `Policies.tsx` → label "Platform Policies"); add `ControlPlaneResources.tsx`.
- `Header.tsx`: `isAdmin` → `pathname.startsWith("/control-plane")`; `<Link to="/control-plane">` labelled **"Control Plane"**.
- `Sidebar.tsx`: `isAdmin` + `base` use `/control-plane`; rename the heading.
- `PortalToggle.test.tsx`: update the `/admin` paths + link-name expectations to `/control-plane` / "Control Plane".

### 4. Page depth
- Every new workspace item + the renamed/kept control-plane items are `<Placeholder title=… />`.
- **Resources** is a **structured frame** (no backend): a page with a `Card`, **type tabs** (Trino / StarRocks / Spark Connect), a table (`Name · Type · Endpoint · Status · Workspaces · Actions`) showing an **empty state** per tab, and a header **"Add resource"** button that is a stub (disabled or opens a "coming soon" note). Clearly a frame — no API calls.

### 5. Policies in both layers (scope-separated)
- Workspace **Data Governance → "Policies"** (`/explore/:ws/policies`) = the workspace's data-access policies (placeholder).
- Control Plane **"Platform Policies"** (`/control-plane/platform-policies`) = global Ranger admin (the renamed existing Policies placeholder).

### Routes added/renamed
- Workspace (new placeholders): `alerts`, `data-api`, `workflows`, `pipelines`, `code-ide`, `policies`, `data-lineage`, `query-audit`, `data-quality`.
- Control Plane: `workspaces`, `resources` *(frame)*, `monitoring`, `users`, `platform-policies`, `catalog`. Index → `workspaces`.
- Slugs derive from labels via the existing `slug()` (e.g. "Data API" → `data-api`, "Code IDE" → `code-ide`, "Query Audit" → `query-audit`, "Data Lineage" → `data-lineage`, "Data Quality" → `data-quality`, "Platform Policies" → `platform-policies`).

### Icons (lucide)
Alerts→`Bell`, Data API→`Plug`, Workflows→`Workflow`, Pipelines→`Waypoints`, Code IDE→`Code`, Policies/Platform Policies→`ShieldCheck`, Data Lineage→`Share2`, Query Audit→`ScrollText`, Data Quality→`BadgeCheck`, Resources→`Server`. (Existing icons unchanged.)

## Components touched
- `components/layout/Sidebar.tsx` — new `NavGroup[]` for the four workspace zones + the Control Plane group; base/isAdmin/heading rename.
- `components/layout/Header.tsx` — `/control-plane` link + "Control Plane" label + isAdmin.
- `App.tsx` — new placeholder routes; `ControlPlaneRoutes` + `/control-plane/*`; imports from `routes/control-plane/*`.
- `routes/control-plane/*` (moved from `routes/admin/`) + new placeholder pages under `routes/explore/*` (or inline `<Placeholder>`), + `routes/control-plane/ControlPlaneResources.tsx` (the frame).
- `components/layout/__tests__/PortalToggle.test.tsx` + any sidebar test — updated for the rename.

## Testing (Vitest)
- Sidebar renders the four workspace zones with their items (incl. Alerts, the Data-Engineering + Data-Governance items) at the right `/explore/:ws/...` links; renders the Control Plane group at `/control-plane/...`.
- Navigating to each new route renders its placeholder (title present) / the Resources frame.
- `/control-plane` index redirects to `workspaces`; the Header toggle links to `/control-plane` labelled "Control Plane"; `isAdmin` detection uses `/control-plane`.
- The Resources frame renders the three type tabs + an empty-state table + the (stub) Add button.
- Existing suites stay green (PortalToggle + any route/sidebar tests updated for the rename).

## Out of scope (later epics, each its own spec→plan→build)
The actual features behind every placeholder: Alerts; Data API; Workflows (n8n); Pipelines (Airflow); Code IDE (git); workspace Policies + Platform Policies (Ranger UI); Data Lineage; Query Audit log; Data Quality; and Control-Plane Resources CRUD + assignment + live monitoring (Trino/StarRocks/Spark Connect provisioning, internal-only monitoring). No backend changes in this scaffold.
