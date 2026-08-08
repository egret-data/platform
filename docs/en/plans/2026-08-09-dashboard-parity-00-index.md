# Dashboard Parity — Plan Index

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to
> implement each workstream plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close every divergence between egret's dashboard and Metabase's, except the four
explicitly declined in §5 of the design.

**Design:** `docs/en/specs/2026-08-09-dashboard-parity-design.md` (Vietnamese twin in
`docs/vi/specs/`). Read §2 (measured defects) and §9 (findings that did not survive inspection)
before starting any workstream — §9 exists so nobody re-fixes a non-problem.

**Tech Stack:** React 18 + TypeScript + Vite, Tailwind v4 (OKLCH tokens), shadcn/Radix,
react-grid-layout, TanStack Query, vitest + Testing Library. Backend: Rust (axum + sqlx),
Postgres.

---

## One plan per workstream

| Plan | Workstream | Depends on | Backend? |
|---|---|---|---|
| `…-01-responsive-grid.md` | W1 responsive grid + app-shell breakpoint | — | no |
| `…-02-edit-safety.md` | W2 discard confirm, undo toasts, grid stroke, mapping control | W1 | no |
| `…-03-width-control.md` | W3 fixed/full width | — | yes (column default) |
| `…-04-view-header.md` | W4 refresh popover, manual refresh, "…" menu, shortcuts | W3 | no |
| `…-05-card-level.md` | W5 download formats, edit-question link, description, background | — | no |
| `…-06-filters.md` | W6 single/multi, reorder, auto-connect, auto-apply, linked panel, `location` fix | — | no |
| `…-07-inline-parameters.md` | W7 move a filter into a heading/question card | W6 | no |
| `…-08-info-and-revisions.md` | W8 owner name, folder link, revision history | — | yes (**two** migrations + endpoints — all three items need backend, not just revisions) |
| `…-09-iframe-card.md` | W9 iframe card type | — | no |

**Ordering constraints, and only these.** W1 before W2 (both edit `DashboardGridLayout.tsx`).
W3 before W4 (W4's "…" menu is where W3's width switch lives — W3 creates the menu component).
W6 before W7 (W7 reuses W6's parameter-ordering model). Everything else is independent and may
run in parallel worktrees.

**Cut line.** W7, W8 and W9 can each be dropped without leaving anything half-built. W8 is the
only workstream requiring backend work; if the programme is trimmed, trim from the bottom.

---

## File structure — what each file becomes responsible for

Locking this in before task decomposition, so two workstreams do not both grow the same file.

### New files

| Path | Responsibility |
|---|---|
| `frontend/src/components/chart/dashboard/grid-breakpoints.ts` | Breakpoint constants and the desktop→mobile layout derivation. Pure functions, no React. (W1) |
| `frontend/src/lib/useIsNarrowViewport.ts` | A `matchMedia` hook for the app shell's sidebar rule. Deliberately **separate** from the grid's breakpoint: the grid keys off its own measured *container* width (which it already tracks with a `ResizeObserver`), the shell keys off *viewport* width. They measure different things and must not be collapsed into one number. (W1) |
| `frontend/src/components/chart/dashboard/DashboardActionsMenu.view.tsx` | The dashboard-page "…" menu (Duplicate / Move / Trash / Edit settings / Reset filters). Distinct from the existing list-page `DashboardActionsMenu.tsx`. (W3 creates, W4 fills) |
| `frontend/src/components/chart/dashboard/RefreshMenu.tsx` | Auto-refresh popover + countdown trigger, replacing the native `<select>`. (W4) |
| `frontend/src/lib/useDashboardShortcuts.ts` | Keyboard shortcut registration for the dashboard route. (W4) |
| `frontend/src/components/chart/export-xlsx.ts` | Lazy `.xlsx` writer, kept out of the main bundle. (W5) |
| `frontend/src/components/chart/dashboard/autoConnect.ts` | Pure "which other cards have a column of this name" resolution + the patch it produces. (W6) |
| `frontend/src/components/chart/dashboard/DefaultValueField.tsx` | Typed default-value widget dispatching on parameter type. (W6) |
| `migrations/0106_dashboard_attribution.sql` | `created_at` + `updated_by` on `analytics.dashboards`. (W8 — needed by items 1 and 2, not only by revisions.) |
| `migrations/0107_dashboard_revisions.sql` | Revisions table. (W8 — **0105 is taken by W3**; 0104 is the highest on `main`. If W3 is dropped, W8 renumbers down.) |
| `frontend/src/components/chart/dashboard/IFrameCard.tsx` | iframe card + URL allow-list. (W9) |

### Modified files, and by whom

| Path | W1 | W2 | W3 | W4 | W5 | W6 | W9 |
|---|---|---|---|---|---|---|---|
| `components/chart/dashboard-grid.ts` | ✎ | | | | | | ✎ |
| `components/chart/dashboard/DashboardGridLayout.tsx` | ✎ | ✎ | | | | | |
| `components/layout/AppShell.tsx` | ✎ | | | | | | |
| `routes/explore/DashboardView.tsx` | ✎ | ✎ | ✎ | ✎ | ✎ | ✎ | ✎ |
| `components/chart/dashboard/CardFilterMapOverlay.tsx` | | ✎ | | | | | |
| `components/chart/dashboard/DashboardEditToolbar.tsx` | | | ✎ | | | | ✎ |
| `components/chart/dashboard/FilterSettingsPanel.tsx` | | | | | | ✎ | |
| `components/chart/dashboard/DashboardFilterBar.tsx` | | | | | | ✎ | |
| `components/chart/dashboard/DashboardCardView.tsx` | | | | | ✎ | | |
| `components/chart/ChartExportMenu.tsx` | | | | | ✎ | | |
| `api/useDashboardParams.ts` | | | | | | ✎ | |
| `api/useDashboardViewControls.ts` | | | | ✎ | | | |

`DashboardView.tsx` is touched by seven of the nine workstreams. It is already 769 lines. **Do
not let it grow.** Every workstream that adds behaviour there must extract that behaviour into
one of the new files above and leave `DashboardView.tsx` holding wiring only. A workstream that
adds net lines to `DashboardView.tsx` should be reviewed for a missing extraction.

---

## Conventions every workstream follows

**Branch.** One branch per workstream, named `feat/dashboard-parity-NN-<slug>`, branched from
`main`. Where a workstream depends on another (W1→W2, W3→W4, W6→W7), branch from the dependency's
branch, not from `main`, and do not re-include its commits in the PR body.

**Commits.** Conventional Commits. Author is `hoangquocvietbk2.13@gmail.com` — never override it.

**Gates before opening a PR.** GitHub CI is out of quota, so local gates are the real gates. Run
each as its own command — never chain them through a pipe, and never pipe into `rtk`:

```bash
cd frontend && npm run lint
cd frontend && npx tsc --noEmit
cd frontend && npm run test
```

Record the actual test count in the PR body. A count that differs from the previous PR's by more
than the tests you added means a gate did not run.

**Tests must survive mutation.** For each new assertion, stub the implementation it covers and
watch the test go red before claiming it verifies anything. Score mutation runs by process **exit
code**, never by matching stdout text — the `PASS (n) FAIL (m)` line is the rtk wrapper's
reformatting and raw vitest never prints it.

**Live check.** Every workstream ends with a measurement on the running app, not a screenshot
opinion. `just deploy-kind` rebuilds and redeploys to `localhost:30080`
(admin@egret.local / Admin12345!). The harness dashboard is
`/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`; Metabase's twin is
`localhost:3033/dashboard/2`. Record the number the check produced in the PR body.

**Migrations.** The dashboard tables live in the `analytics` schema — `0067_analytics_kernel_schema.sql:14` moved them, so a migration written against bare `dashboards` fails outright; qualify it as `analytics.dashboards`. After adding a migration, touch `crates/egret-metastore/src/lib.rs` to force the migrations to be re-embedded (the convention is documented in that file's header).

**Do not disturb the harness.** 22 cards per side on byte-identical data. Fixing a harness chart
means opening it and pressing Save — never hand-writing its JSON. See
`docs/en/guides/chart-parity-harness.md`.

**Stop at the PR.** Open it and stop; the merge is the user's call.
