# Tier 2c — Edit-mode visual fidelity to Metabase (design)

**Status:** approved · **Date:** 2026-06-22 · **Branch:** `feat/dashboards-tier2b-edit-ux` (continues PR #33)
**Part of:** the dashboard Metabase-parity program ([audit](./2026-06-22-dashboard-metabase-parity-audit.md))

## Goal

Close the remaining *visual* edit-mode gaps found by walking through Metabase's dashboard-create
flow step by step, so egret's edit mode looks and feels like Metabase. Frontend-only; behaviour and
data unchanged. Built on Tier 2b (right sidebar + clean cards).

## Scope (5 items, from the Metabase walkthrough)

1. **"You're editing this dashboard" banner.** A full-width coloured strip at the very top while
   editing (Metabase shows this as the clear edit-mode indicator). egret keeps its `+ Chart` /
   `+ Filter` / Width / Cancel / Save controls in the dashboard sub-header; the banner is a thin
   `bg-primary text-primary-foreground` bar above everything with a pencil icon + the text.
2. **Floating centred card toolbar.** Today the toolbar (⚙⧉↺✕) sits top-right and overlaps the
   title. Move it to a **centred floating pill** at the top of the card (rounded, bordered, subtle
   shadow, `bg-card`), revealed on hover — like Metabase's pill. It does not overlap the
   left-aligned title (it's centred). The full-width top strip remains the RGL **drag handle**
   (`dash-draghandle`, `cursor-move`); the pill is `dash-nodrag`.
3. **Viz-type icons in the Add panel.** Each chart row in `AddPanel` shows a small lucide icon for
   its `viz_type` (looked up in `CHART_TYPES`), like Metabase's coloured viz icons — far more
   scannable than plain text.
4. **Friendly edit empty state.** When editing and the active area has no cards, show a centred
   icon + "Add your first chart" message + a prominent button that opens/focuses the Add panel
   (Metabase shows a friendly empty state with a centred CTA), instead of a bare grid.
5. **Visible grid cells while editing (best-effort).** Give the grid a faint cell background in
   edit mode so drop targets are visible (Metabase outlines every grid cell). Approximate with a
   CSS background grid sized to the column width / row height; if it can't be aligned cleanly it
   is dropped rather than shipped misaligned.

## Components touched

- `DashboardView.tsx` — the banner; the card wrapper (floating pill toolbar + drag handle); the
  edit empty state; the grid-cell background (best-effort).
- `CardEditToolbar.tsx` — restyle as a centred pill (rounded/bordered/shadow); keep `dash-nodrag`.
- `AddPanel.tsx` — a viz icon per chart row (needs the chart's `viz_type`; map via `CHART_TYPES`).

No prop/data-model change; all existing draft mutators and view mode unchanged.

## Testing

- **Vitest:** `AddPanel` renders a viz icon per chart row (assert an `svg`/`[data-viz]` per row);
  `CardEditToolbar` keeps `dash-nodrag`; `DashboardView` shows the editing banner in edit mode and
  not in view mode; the edit empty-state message renders when editing with no cards.
- **Live (Playwright MCP, side-by-side with Metabase `localhost:3001`):** banner shows in edit;
  card toolbar is a centred pill on hover, no title overlap; AddPanel rows have viz icons; empty
  edit state shows the CTA; grid cells visible (if shipped). View mode unchanged. **0 console
  errors.**
- **Full sweep** before each commit (from `frontend/`): `npm run typecheck && npm run lint &&
  npm run build && npx vitest run`.

## Out of scope (later tiers)

Filter UX overhaul (in-card column mapping, widget type, default, linked filters, more param
types) = Tier 2d. Per-card viz-settings override; link-to-dashboard/question; sharing; etc.

## Conventions

Metabase as the visual standard · clean-room · TDD where unit-testable / live-verify for visual ·
full sweep before each commit · configured git identity + `Co-Authored-By: Claude Opus 4.8
(1M context) <noreply@anthropic.com>` · stage explicit paths · branch `feat/dashboards-tier2b-edit-ux`.
