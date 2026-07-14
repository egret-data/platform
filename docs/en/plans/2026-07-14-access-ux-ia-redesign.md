# Control Plane + Access UX — IA Redesign Roadmap

**Goal:** Make egret's Control Plane + access-control area modern, simple, and easy to
use by restructuring the information architecture and consolidating overlapping tools —
without changing egret's visual system (OKLCH tokens, teal Metabase palette, shadcn).

**Direction approved** from an interactive prototype (see the session artifact). The core
problem: the access/governance area exposed egret's **four authz layers** (RBAC
permissions · roles · Cedar object grants · Ranger data-plane tags/policies) as ~15 flat
sibling menu items, plus **three overlapping lookup tools** (Overview · Access explorer ·
Who-can-access), with no conceptual scaffolding.

## Design decisions

1. **Group the IA by a mental model, not a feature list:** Overview → Identity (who) →
   Access (what they can do/see) → Data governance (data-plane rules) → Infrastructure.
2. **Merge the 3 lookup tools into one bidirectional Access Explorer** (principal→objects
   ↔ object→principals) that shows the access *path* (direct / role / group), effect
   (allow/deny, deny-wins), and expiry.
3. **Overview becomes a posture hub** that visualizes the 4-layer model + KPIs +
   "needs attention" + recent changes.
4. **Consistent list scaffold** across every list page: header + KPI strip + facet
   filters + table + detail side-sheet (reuse `FilterableTable`, `FacetSelect`,
   `SummaryStrip`).

## Increments (one PR each, FE-only, plan-first)

- [x] **I1 — Sidebar IA restructure** (`components/layout/Sidebar.tsx`). Regroup
  `CONTROL_PLANE_GROUPS` into Overview / Identity / Access / Data governance /
  Infrastructure. Routes unchanged; only grouping + ordering moved. Sidebar test updated
  (13/13). Branch `feat/access-ux-ia-redesign`.
- [x] **I2 — Bidirectional Access Explorer.** (commit e78d036) Merge `access/AccessExplorer.tsx` +
  `access/WhoCanAccess.tsx` into one page with a direction toggle. Keep both routes
  (`access-explorer`, `who-can-access`) resolving to the merged page (deep-link preserved)
  or redirect one to the other. Collapse the sidebar's two Access items into one
  "Access Explorer" once merged.
- [x] **I3 — Overview posture hub.** (commit ee3a77e) Rework `access/AccessOverview.tsx`: add the 4-layer
  model card (Identity → RBAC → Object grants → Data plane, each with count + deep link),
  keep/refine KPI strip + warnings + recent-changes. Hide KPIs the viewer lacks perms for
  (existing follow-up nit).
- [ ] **I4 — List scaffold consistency.** Standardize Users / Roles / Groups / Policies /
  Warehouses / Engines list pages on the shared scaffold (header + KPI strip + facets +
  table + side-sheet). Increment page-by-page.

## Notes

- Visual system is **unchanged** — this is IA/interaction work. Do not introduce new
  colors/fonts; use existing tokens in `frontend/src/index.css`.
- The Explore (workspace) sidebar is out of scope; only `CONTROL_PLANE_GROUPS` changed.
- Per project convention: subagent-driven where a task is sizeable; verify FE
  lint/typecheck/test locally before each squash-merge (GitHub CI is quota-limited).
