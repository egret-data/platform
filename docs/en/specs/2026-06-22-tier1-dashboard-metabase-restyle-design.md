# Tier 1 — Dashboard Metabase-style restyle + filter pills (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-22 · **Branch:** `fix/dashboard-parity-gaps`
**Part of:** the dashboard Metabase-parity program ([audit backlog](./2026-06-22-dashboard-metabase-parity-audit.md))

## Goal

Make egret's dashboard **look and feel like Metabase** and close the highest-value UX gaps:
a Metabase-style canvas + cards, **filter pills with popovers** (replacing the raw native
`<select>` widgets), brand-coloured tabs, required-parameter enforcement, and an empty-tab
state. Frontend-only; no backend change. Clean-room (match look & behaviour, not Metabase code).

## Metabase grounding

Reference: the running Metabase dashboard `E-commerce Insights` (localhost:3001/dashboard/1):
light-gray canvas; white cards with a soft shadow, rounded corners, generous padding, an
inline title top-left; a row of rounded **filter pills** (`[icon] Name` → `[icon] Name:
4 selections`) that open a popover with a search box + checkbox list (category), date/number
controls, etc.; the active tab underlined in the brand colour. egret today: a bordered box of
native `<select>`/`<select multiple>` widgets, boxy cards with a divider header, dark-underline
tabs, white canvas.

## Scope (this tier)

**In:** (A) canvas + card visual restyle; (B) filter pills + per-type popover widgets replacing
`DashboardFilterBar`'s native controls; (C) brand-coloured active tab; (D) required-param
enforcement (gate card fetch + empty state); (E) empty-tab state.

**Out (later tiers / deferred):** night/theme mode, PDF export, value sources, linked filters,
map viz, sharing/embedding, action buttons, per-column click, hover `⋯` card menu (deferred to
keep this tier tight), Metabase's "Apply filters" button + auto-apply toggle (egret **auto-applies**
on change — kept as-is).

## Components

### A. Canvas + card shell (`DashboardView.tsx`, `DashboardCardView.tsx`)

- Dashboard content wrapper gets a muted canvas: add `bg-muted/30` to the page container (the
  fullscreen container too), with the existing `p-4`.
- Each grid card `<div>`: from `rounded-md border bg-background` → `rounded-lg border bg-card
  shadow-sm`. Headings stay chrome-less. Add a little inner padding so content isn't flush.
- `ChartCardTile`'s header: drop the `border-b` divider; render the title inline
  (`px-3 pt-2 pb-1 text-sm font-medium`), content below with `px-2 pb-2`.
- Grid spacing: pass an explicit `margin={[12, 12]}` to the RGL `<Grid>` (Metabase-like gap).

### B. Filter pills + popovers (`DashboardFilterBar.tsx` → pill row; new `FilterPill.tsx`)

`DashboardFilterBar` keeps its props (`parameters`, `values`, `onChange`, `optionsBySlug`) but
renders a flex-wrap row of pills (no bordering box). Each pill is a `FilterPill`:

```ts
interface FilterPillProps {
  param: DashboardParameter;
  value: unknown;
  options: string[];           // category/id distinct values (from optionsBySlug)
  onChange: (v: unknown) => void;
}
```

- **Pill button** (a `Popover` trigger): a type icon (lucide: `Tag`/`List` for category, `Hash`
  for id, `Type` for text, `Hash` for number, `Calendar` for date) + the param name; when a value
  is set, append `: <summary>` and show a clear (×) that stops propagation and resets the value.
  `summarize(param, value)`: category/multi → `"N selections"` for >1 else the single value;
  scalar → the value; range/between → `"a – b"`; relative → the option label; blank → none.
- **Popover content** branches by `param.type` (one `FilterPill`, internal switch):
  - **category** → a search `Input` + a scrollable checkbox list (`Checkbox` per option, filtered
    by the search text), **multi-select** into a string[] (→ `in` filter); a "Clear" button.
  - **id** → a search `Input` + a scrollable **single-select** list (one value → `eq` filter),
    with a "Clear"; falls back to a single text input when no options loaded. (id is single-value,
    unlike category — keep them distinct.)
  - **text** → an `Input` bound to the string value (operator already lives on the param).
  - **number** → an `Input type=number`; for operator `between`, two inputs (min/max → `[min,max]`).
  - **date** → `single` one date input; `range` two date inputs (`[start,end]`); `relative` a
    select of Last 7/30/90/365 days. (Same value encodings the current `paramToFilters` consumes.)
- Selecting values calls `onChange` (same contract as today → `paramsState.setValue` → cards
  refetch). The popover stays open for multi-select; closes on outside-click.

The value→filter translation (`paramToFilters`) and option loading (`useColumnValues`) are
unchanged — this is purely the widget layer.

### C. Tabs (`DashboardTabBar.tsx`)

Active tab: replace `border-foreground font-medium` with `border-primary text-primary
font-medium`; inactive stays muted. No behaviour change.

### D. Required-param enforcement (`DashboardView.tsx`, `useChartData.ts`, `DashboardCardView.tsx`)

- Compute `requiredUnset = parameters.filter(p => p.required && isBlank(values[p.slug]))`
  (reuse the `isBlank` helper from `params.ts`; export it).
- When `requiredUnset.length > 0`: the filter row shows a small notice
  (`Set the required filter(s): Region, …`), and **chart cards do not fetch** — they render a
  muted "Set the required filters to load this card" placeholder.
- Plumbing: add an optional `enabled` arg to `useChartData(ws, chart, nonce, enabled = true)`
  that gates the build/submit effect (when `false`, no request, `isLoading` false). `ChartCardTile`
  takes a `blocked` prop, passes `enabled={!blocked}`, and renders the placeholder when blocked.
  `DashboardCardView` threads `blocked` down; `DashboardView` sets it from `requiredUnset`.
  Required pills render with a subtle "required" affordance (e.g. an asterisk, already present).

### E. Empty-tab state (`DashboardView.tsx`)

When a dashboard has tabs and the active tab has no cards (`tabs.length > 0 &&
visibleCards.length === 0`), render a centered muted message "No cards on this tab." instead of
the empty grid. (The existing whole-dashboard "No cards yet" message stays for tabless empties.)

## Data flow

```
parameters + values + optionsBySlug
  → DashboardFilterBar → FilterPill per param
       click → Popover (per-type control) → onChange(value) → paramsState.setValue → URL + refetch
  requiredUnset = required params with blank value
       → notice in the pill row
       → DashboardCardView(blocked=true) → ChartCardTile(enabled=false) → placeholder (no fetch)
visibleCards (active tab) empty + has tabs → "No cards on this tab."
```

## Error handling

- A category/id param whose options haven't loaded yet → popover shows "Loading…"; id falls back
  to a free text input so the filter still works.
- `onChange` with an empty multi-select → value becomes `[]` (treated as blank → no filter), pill
  reverts to the unset label.
- Required gating never deadlocks: setting the required value clears `blocked` and cards fetch.
- A blank/loading column-values response leaves the checkbox list empty with a "No values" note;
  the search input still renders.
- Restyle is purely class changes — no behavioural regressions to tabs, click behaviour (D1),
  cross-filter, fullscreen, or auto-refresh.

## Testing

- **Vitest:**
  - `FilterPill`: renders the type icon + name; `summarize` text for category multi ("2
    selections"), single value, range, blank; category popover lists options, typing filters
    them, checking an option calls `onChange` with the array, "Clear" empties it; text/number/date
    controls call `onChange` with the right encoding; the clear (×) resets.
  - `DashboardFilterBar`: renders one pill per parameter; shows the required notice when a required
    param is blank.
  - `useChartData`: `enabled=false` fires no request and is not loading.
  - `DashboardView`: a required-but-unset param blocks chart cards (placeholder shown, no card
    content); setting it unblocks. Empty active tab shows "No cards on this tab." Existing tabless
    + click-behaviour tests stay green.
- **Live (Playwright MCP, side-by-side with Metabase):** the restyled canvas/cards/tabs match
  Metabase's look; filter pills open popovers, category search+checkbox multi-select filters the
  cards + updates the pill summary + URL; required enforcement gates then unblocks; empty tab
  message shows. **0 console errors.**
- **Full sweep** before each commit (from `frontend/`): `npm run typecheck && npm run lint &&
  npm run build && npx vitest run`.

## Conventions

Metabase as the visual standard · clean-room (no AGPL) · TDD · full sweep before each commit ·
reuse existing Radix UI primitives (`popover`, `checkbox`, `input`, `button`, `scroll-area`) — no
new deps · configured git identity (never `--author`), footer `Co-Authored-By: Claude Opus 4.8
(1M context) <noreply@anthropic.com>` · stage explicit paths · squash-merge · branch
`fix/dashboard-parity-gaps`.
