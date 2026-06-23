# Metabase-faithful dashboard style (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-23 · **Branch:** `feat/metabase-dashboard-style` (off main)

## Goal

Make egret's **dashboard surfaces** look like Metabase — the defining "white cards floating on a light-gray canvas, defined by a thin border (not a shadow)" look — plus matching header/tab/filter/edit chrome, typography, and spacing. **Keep egret's teal brand** (`--primary`) for buttons/links/active/ring; copy only Metabase's neutral structure. Adjust egret's own OKLCH tokens (one source of truth, light + dark), do NOT introduce a parallel `--mb-color-*` system. Scope is the dashboard only (not chart-builder / SQL Lab). One comprehensive spec.

## Reference — Metabase dashboard style (verified from `metabase` repo + live)

- **Canvas**: page background `--mb-color-bg-dashboard` = `hsla(240,4%,95%)` (light cool gray). Cards are white tiles on this gray.
- **Card**: bg white; **1px solid border** `--mb-color-border` `hsla(195,6%,87%)`; radius `6px` (`--mantine-radius-sm`); **no resting shadow** (drag shadow only `3px 3px 8px shadow`); card title ~14px.
- **Header**: white bg, bottom 1px border; title `1.25rem`/`700`/`1.5rem` line-height; padding `1rem` top / `0.75rem` bottom; action icons gap `0.5rem`.
- **Tabs**: 2px baseline border `--mb-color-border`; active tab indicator.
- **Filter/parameter bar**: padding `0.5rem 0`; sticky bg `--mb-color-bg-dashboard` + bottom border when stuck; pills radius `6px`.
- **Edit bar**: brand @ 85% opacity, white bold text.
- **Card edit toolbar**: white pill, radius `8px`, shadow `0 1px 3px rgb(0 0 0/13%)`, hover-revealed (`opacity 0→1`).
- **Typography**: base `14px`; size scale xs11/sm12/md14/lg17/xl21; family Roboto-ish.
- **Spacing**: `4/8/16/24/32`; radius `4/6/8`; grid gap `6px`.

## Existing egret state (verified — `frontend/src/index.css` + dashboard components)

- Tokens (`:root`): `--background: oklch(1 0 0)` (white), `--card: oklch(1 0 0)` (white), `--primary: oklch(0.59 0.15 185)` (teal — KEEP), `--secondary: oklch(0.95 0.01 258.36)` (light cool gray — already ≈ Metabase canvas), `--muted: oklch(0.22 0.11 263.03 / 3%)` (near-transparent — why `bg-muted/30` is too faint), `--border: oklch(0.92 0 286.61)` (≈ Metabase border), `--radius: 0.625rem` (**10px** — why cards look rounder than Metabase's 6px). `.dark` has `--card: 0.21`, `--background: 0.14`, `--secondary/--muted: 0.27`.
- `@theme` block (line ~8) registers Tailwind color/utility tokens from these vars.
- Dashboard page wrapper (`DashboardView.tsx:277`): `min-h-full bg-muted/30 p-4` (canvas too faint).
- Card chrome (`DashboardView.tsx:426`): `relative h-full overflow-hidden rounded-lg border bg-card shadow-sm` (+ `ring-2 ring-primary` when its settings panel is open). **Has a shadow; 10px radius.**
- Edit bar (`DashboardView.tsx:279`): `bg-primary … text-primary-foreground` (teal — KEEP).
- Header title (`DashboardView.tsx:291`): `text-lg font-semibold` (18px/600).
- Card title (`DashboardCardView.tsx:36`): `px-3 pt-2 pb-1 text-sm font-medium`.
- Card edit toolbar (`CardEditToolbar.tsx`): already a white pill — `rounded-md border bg-card p-0.5 shadow-sm` (already ≈ Metabase; minor radius tweak only).
- Tab bar (`DashboardTabBar.tsx`): container `border-b`; active tab `border-b-2 border-primary text-primary` (teal — KEEP).

## Design — token changes (light + dark)

Add a dedicated **dashboard canvas** token rather than overloading `--secondary`, and register it in `@theme` so `bg-dashboard` works:
- `:root`: `--dashboard: oklch(0.955 0.004 270)` (light cool gray ≈ Metabase `hsla(240,4%,95%)`).
- `.dark`: `--dashboard: oklch(0.16 0 0)` (darker than `--card` 0.21 so cards still pop).
- `@theme`: add `--color-dashboard: var(--dashboard)`.
- `--border` (0.92) and `--card` (white / dark 0.21) already match Metabase — unchanged.
- Card radius: keep the global `--radius` (10px) for the rest of the app; cards get an explicit **6px** via `rounded-md` (Tailwind `md` = 6px; if egret's `@theme` remaps `rounded-md`, use `rounded-[6px]` — confirm at build time). Brand/teal tokens unchanged.

## Design — component changes (dashboard surfaces only)

1. **Canvas** (`DashboardView.tsx:277`): `bg-muted/30` → `bg-dashboard`. (Both the normal and `isFullscreen` wrappers.)
2. **Card chrome** (`DashboardView.tsx:426`): `rounded-lg border bg-card shadow-sm` → `rounded-md border bg-card` (drop `shadow-sm`, 10px→6px). Keep `overflow-hidden`, the heading-card no-border branch, and the `ring-2 ring-primary` selected state.
3. **Header title** (`DashboardView.tsx:291`): `text-lg font-semibold` → `text-xl font-bold`. Add a 1px bottom border + consistent padding to the header row to match Metabase (`border-b pb-3`).
4. **Card title** (`DashboardCardView.tsx:36`): `px-3 pt-2 pb-1 text-sm font-medium` → `px-4 pt-3 pb-2 text-sm font-medium text-foreground` (Metabase padding; keep 14px medium).
5. **Tab bar** (`DashboardTabBar.tsx`): keep teal active indicator; ensure the baseline is a 2px border (`border-b-2 border-border` on the container, active tab `-mb-[2px] border-b-2 border-primary`) so the active tab sits on the baseline like Metabase.
6. **Filter bar** (the parameter pills — `DashboardFilterBar.tsx` / `EditFilterChips.tsx`): align pill chrome to Metabase — `h-8 rounded-md border bg-background px-2 text-sm` with the leading tag icon + trailing chevron; filled/active pill uses `bg-secondary`. (Restyle only; no behavior change.)
7. **Edit bar** (`DashboardView.tsx:279`): keep teal; soften to `bg-primary/90` to match Metabase's brand-@85% (optional, low-risk).
8. **Card edit toolbar** (`CardEditToolbar.tsx`): already a white pill — bump radius to `rounded-lg` (8px) to match Metabase; otherwise unchanged.
9. **Heading card** (`HeadingCard.tsx`): ensure heading text is ~`text-xl font-bold text-foreground` (Metabase section heading) on a transparent (no-border) card.

## Data flow / structure

No data or behavior changes — this is a visual restyle of existing components. Token additions flow through Tailwind's `@theme`; component edits are class-only. Dark mode is covered by defining `--dashboard` in both `:root` and `.dark`.

## Error handling / edge cases

- **Dark mode**: every token change is defined in both `:root` and `.dark`; cards (`--card` 0.21) must stay lighter than the canvas (`--dashboard` 0.16) so they pop — verified by the chosen values.
- **Heading cards** keep their no-border/transparent treatment (the card-chrome change must not add a border/bg to heading cards — preserve the existing `card_type === "heading" ? "" : "…"` branch).
- **Radius source**: if `rounded-md` doesn't resolve to ~6px under egret's `@theme`, fall back to `rounded-[6px]` (decided at implementation by inspecting the computed radius).
- **Contrast**: the gray canvas must keep AA contrast for text on cards (cards stay white) and the canvas-vs-card edge stays visible via the 1px border.

## Testing

- **Vitest**: this is class/token styling; no meaningful unit assertions (styling isn't logic). Keep the existing dashboard tests green (the class changes must not break role/text queries). Optionally a tiny snapshot/class assertion that the card wrapper no longer contains `shadow-sm` and the canvas uses `bg-dashboard` — only if cheap.
- **Live (Playwright MCP, 0 console errors, throwaway dashboard, NOT a1)** — open egret and Metabase (localhost:3001, alice@test.com) side by side:
  - Canvas is a clear light gray; cards are white tiles with a 1px border and **no drop shadow**; card radius ≈ 6px.
  - Header title is 20px bold with a bottom border; tab active indicator is teal on a 2px baseline.
  - Filter pills match Metabase height/radius/border.
  - Edit mode: teal edit bar, white card toolbar pill, hover-revealed SE handle, grid overlay.
  - **Dark mode**: toggle theme — canvas darker than cards, cards still readable, brand teal intact.
  - Compare the overall feel against the Metabase x-ray dashboard; confirm 0 console errors. Verify an existing dashboard (PARITY, read-only) looks coherent.
- **Full sweep** before each commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Conventions

Metabase fidelity (structure/neutrals only; egret keeps its teal brand) · clean-room (reimplement values, don't copy proprietary CSS) · adjust egret's OKLCH tokens (one source of truth, light + dark) · full sweep before each commit · configured git identity + `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage explicit paths · docs bilingual · squash-merge · branch `feat/metabase-dashboard-style`.

## Task order (for the plan)

1. Tokens: add `--dashboard` (`:root` + `.dark`) + `@theme` `--color-dashboard`.
2. Canvas + card chrome (`DashboardView.tsx`): `bg-dashboard`, card `rounded-md border bg-card` (no shadow).
3. Header + card title + heading card typography.
4. Tab bar baseline + filter-pill chrome.
5. Edit chrome polish (edit bar `/90`, toolbar radius).
6. Live verify (egret vs Metabase, light + dark) + PR.
