# Tier 2a — Dashboard view options: fixed/full width + dark-mode polish (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-22 · **Branch:** `feat/dashboards-tier2a` (off main)
**Part of:** the dashboard Metabase-parity program ([audit backlog](./2026-06-22-dashboard-metabase-parity-audit.md), Tier 2)

## Goal

Two small Metabase-parity view options: a persisted **fixed/full width** dashboard setting, and
**dark-mode polish** so the dashboard reads correctly under egret's existing global dark theme.
Clean-room (match Metabase look/behaviour, not code).

## Scope

**In:** (A) `dashboards.width` ('fixed'|'full') persisted setting + edit-mode toggle + view-mode
centered container when fixed; (B) verify + fix the dashboard chrome (canvas, cards, filter
pills, tabs, popovers) under the global dark theme.

**Out / deferred (named):** **PDF/print export** (its own feature — reliable ECharts→PDF needs a
client lib + per-viz `getDataURL()` handling); a *dashboard-scoped* night-mode toggle (egret's
global theme toggle already serves Metabase's "night mode" — no separate toggle); ECharts
chart-internal dark theming (axis/label colours) — a broader chart concern, noted but not in
this tier unless a found gap is trivial.

## A. Fixed/full width (persisted, Metabase-faithful)

Metabase stores `width: "fixed" | "full"` on the dashboard (an edit-mode setting that applies to
all viewers; "fixed" centres the grid at a max width, "full" is full-bleed). egret mirrors this.

### Backend (`egret-bi`)
- **Migration `0018_dashboard_width.sql`:**
  ```sql
  ALTER TABLE dashboards ADD COLUMN width text NOT NULL DEFAULT 'full'
    CHECK (width IN ('fixed', 'full'));
  ```
  (sqlx migration-staleness: `touch crates/egret-metastore/src/lib.rs` before building/testing.)
- `dashboard.rs`: `Dashboard` struct + `COLS` gain `width: String`. `update(...)` gains a `width`
  param and sets it (`UPDATE dashboards SET name=$2, description=$3, parameters=$4, width=$5, …`).
- `model.rs`: `DashboardDetailDto` gains `width: String`; `UpdateDashboard` gains
  `#[serde(default = "default_width")] width: String` (default `"full"`); `dashboard_detail_dto`
  passes it through. Add a `validate` so only 'fixed'/'full' is accepted (or rely on the DB CHECK
  → maps to a 500; prefer an explicit 400 in the handler: reject width not in {fixed, full}).
- `routes.rs` `db_update` passes `b.width` to `update`.

### Frontend
- `api/dashboards.ts`: `DashboardDetail` gains `width: "fixed" | "full"`; the `update` mutation
  body gains `width`.
- `DashboardView.tsx`:
  - **Edit mode:** a small "Width" toggle in the edit header (two buttons or a select: Fixed /
    Full), held in a `draftWidth` state (seeded from `dashboard.width` on `startEdit`), saved in
    the existing whole-dashboard PUT (`onSave` sends `width: draftWidth`).
  - **View mode:** when `width === "fixed"`, wrap the dashboard content (filter bar + tabs + grid)
    in a centred container `mx-auto w-full max-w-[1200px]`; when `"full"`, today's full-bleed. The
    outer muted canvas stays full-bleed; only the content column is constrained (like Metabase).

## B. Dark-mode polish

egret already has a global light/dark theme (the header moon toggle). The dashboard components use
theme tokens (`bg-card`, `bg-muted`, `text-foreground`, `border`, `text-muted-foreground`) — a grep
found **no** hardcoded colours in the dashboard components — so the chrome should already adapt.

- **Verify live in dark mode** (toggle the theme on the seeded PARITY dashboard): canvas, cards,
  filter pills (incl. the `bg-primary/5` selected state + popover), tabs, the edit-mode controls,
  the drill menu/records dialog, the empty/blocked placeholders.
- **Fix any gaps found** (e.g. a hardcoded colour, insufficient contrast, an invisible border) by
  swapping to the appropriate theme token. Keep fixes minimal and token-based.
- ECharts chart bodies render their own colours; if they're unreadable on a dark canvas, note it
  as a follow-up (chart-wide dark theme) rather than expanding this tier — unless a one-line
  container fix suffices.

## Data flow

```
edit mode: draftWidth (Fixed/Full toggle) → onSave PUT { …, width } → dashboards.width
view mode: dashboard.width === "fixed" → content wrapped in mx-auto max-w-[1200px]
dark mode: global theme (html.dark) → dashboard chrome uses tokens → already adapts (verify/fix)
```

## Error handling

- PUT `width` not in {'fixed','full'} → 400 (explicit handler check; the DB CHECK is a backstop).
- Existing dashboards default to `width = 'full'` (migration default) — unchanged behaviour.
- A tabless / empty dashboard is unaffected by the width wrapper.

## Testing

- **Backend (`egret-bi` repo test):** `update` round-trips `width`; `dashboard_detail_dto` /
  `get` return it; default is `'full'` for a freshly created dashboard.
- **Vitest:** the edit-mode Width toggle writes `draftWidth` and the save payload includes `width`;
  in view mode `width: "fixed"` renders the centred `max-w-[1200px]` wrapper (assert the class /
  a `data-width` attr), `"full"` does not.
- **Live (Playwright MCP):** toggle Width Fixed↔Full in edit, save, reload → persists + view
  layout changes; toggle the global theme → the dashboard chrome looks correct in dark mode (fix
  gaps); compare against Metabase `localhost:3001/dashboard/1` (its fixed-width centred layout).
  **0 console errors.**
- **Full sweep** before each commit: frontend `npm run typecheck && npm run lint && npm run build
  && npx vitest run`; backend `cargo fmt --all --check && cargo test -p egret-bi` (fmt is in CI —
  see the cargo-fmt lesson).

## Conventions

Metabase as the standard · clean-room (no AGPL) · TDD · full sweep (incl. `cargo fmt`) before each
commit · configured git identity (never `--author`), footer `Co-Authored-By: Claude Opus 4.8
(1M context) <noreply@anthropic.com>` · stage explicit paths · squash-merge · branch
`feat/dashboards-tier2a`.
