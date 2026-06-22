# Phase 3d-B — Dashboards: card variety + polish (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-21 · **Builds on:** Phase A (grid of saved-chart cards, merged PR #25, `4a71a1d`)

## Goal

Round out egret dashboards with the second Metabase parity slice: **text /
heading / link cards**, a **per-card title** override, **duplicate / replace**
card actions, and **auto-refresh + fullscreen** view controls — matching Metabase
behaviour, the same fidelity bar held for Phase A.

## Why / Metabase grounding

Studied the local Metabase source (`data-portal/metabase`). Findings that anchor
this design:

- **Text card** — a virtual dashcard with `display:"text"`; body in
  `visualization_settings.text`; rendered with **react-markdown** + remark/rehype
  GFM (`dashboard/visualizations/Text/Text.tsx`).
- **Heading card** — `display:"heading"`, single-line, `text` setting,
  `"dashcard.background": false` (`Heading/Heading.tsx`).
- **Link card** — `display:"link"`, `visualization_settings.link.{url|entity}`
  (`LinkViz/LinkViz.tsx`). egret implements **external URL only** (no internal
  entity refs).
- **Per-card title** — `visualization_settings["card.title"]`; default = the
  question name; `""` hides the header (`lib/settings/visualization.ts`).
- **Auto-refresh / fullscreen** — *not* persisted on the dashboard; driven by
  context state **synced to URL** (`?refresh=<seconds>`, `?fullscreen=true`) via
  `useDashboardLocationSync`; refresh options in seconds: off / 60 / 300 / 600 /
  900 / 1800 / 3600 (`RefreshWidget`). Refresh re-dispatches each card's query.
- **Duplicate** — clone the dashcard with a new temp id, appended below
  (`duplicateCard`). **Replace** — swap the card on an existing dashcard, keeping
  its grid position, and **clear its visualization_settings** (`replaceCard`).

Clean-room: behaviour copied, no AGPL source. egret keeps its own schema (`options`
jsonb keys, not Metabase's `visualization_settings` names).

Deliberate trims (confirmed with the user): **no night mode** (egret already has a
global light/dark theme toggle), **no `{{variable}}` interpolation** in text (it
depends on dashboard parameters → Phase C), **no internal-entity link targets**.

## Data model — no migration

Phase A's `dashboard_cards` already has `card_type text` and `options jsonb`, and
the backend already accepts cards with `chart_id = NULL` (the `require_cards_in_ws`
guard only validates cards that *have* a `chart_id`). So Phase B adds **zero**
backend/SQL change. `card_type` and the `options` shape per type:

| `card_type` | `chart_id` | `options` |
|---|---|---|
| `chart` | the chart's id | `{ title?: string }` — override card header; `""` hides it |
| `text` | `null` | `{ text: string }` — markdown body |
| `heading` | `null` | `{ text: string }` — single line |
| `link` | `null` | `{ url: string, label?: string }` |

The frontend `DashboardCard`/`CardInput` types already carry `card_type` and
`options: Record<string, unknown>`; we add typed accessors, not schema.

## Architecture (frontend only)

### Card render dispatch

Today `DashboardView` renders every card through the chart-only `CardTile`.
Extract a **`DashboardCardView`** dispatcher that switches on `card_type`:

- `chart` → the existing chart path (`useChartData` + `ChartRenderer`), header
  text = `options.title ?? chart.name`, header hidden when `options.title === ""`.
- `text` → `<TextCard text={options.text} />` — renders markdown.
- `heading` → `<HeadingCard text={options.text} />` — single line, no border/bg.
- `link` → `<LinkCard url label />` — a clickable tile opening the URL in a new tab.

Each card type is a small, independently testable component under
`frontend/src/components/chart/dashboard/`. The chart card keeps its own
`useChartData` hook call (so hooks stay one-per-card — `DashboardCardView` must
itself be the per-card component, mounted once per grid child, so the chart
branch's hook is unconditional within it).

### Markdown

Add `react-markdown` + `remark-gfm`. react-markdown does **not** render raw HTML
by default (no `rehype-raw`), so user markdown can't inject scripts — safe without
extra sanitization. GFM gives tables / lists / autolinks / strikethrough.

### Edit-mode card editing

In edit mode each card is editable in place:

- **Add toolbar** gains **+ Text**, **+ Heading**, **+ Link** buttons beside the
  saved-chart list. Each appends a new card of that type with default `options`
  (empty text / empty url) at the bottom (same `addCard` positioning as Phase A).
- **Inline editors** (only while editing):
  - text → a `<textarea>` bound to `options.text`
  - heading → an `<input>` bound to `options.text`
  - link → `url` + `label` `<input>`s
  - chart → a "Title" `<input>` bound to `options.title` in the card header
  Edits mutate the draft card's `options` (same draft-cards state as Phase A); a
  card's options are persisted by the existing whole-layout PUT (no new endpoint).
- **Per-card actions** (edit mode, alongside the existing ✕ remove):
  - **Duplicate** (⧉) — clone the card with a fresh temp id, appended below
    (reuse the maxY positioning), copying `options`.
  - **Replace** (chart cards only) — open the saved-chart picker; set the card's
    `chart_id` to the chosen chart, **reset `options` to `{}`** (Metabase clears
    viz settings on replace), keep grid position + size.

### View controls (view mode, header) — URL-synced

A small hook **`useDashboardViewControls`** reads/writes two URL search params via
react-router `useSearchParams`:

- `?refresh=<seconds>` → `refreshSeconds: number | null` (null = off). Options:
  Off / 1 / 5 / 10 / 15 / 30 / 60 min (stored as seconds, matching Metabase).
  A `setInterval(refreshSeconds*1000)` increments a `refreshNonce`; the nonce is
  threaded into `useChartData` (new optional arg) and added to its effect deps so
  every card re-runs build→submit→poll on each tick. Cleared/reset when the param
  changes or the component unmounts.
- `?fullscreen=true` → `isFullscreen: boolean`. Fullscreen is a CSS
  full-viewport mode (the dashboard container goes `fixed inset-0 z-50 bg-background`,
  covering the app chrome). The toggle also best-effort calls the native
  `requestFullscreen()` / `exitFullscreen()` on click (a real user gesture); on a
  reload with `?fullscreen=true` the CSS mode is restored without the native call
  (browsers block programmatic fullscreen without a gesture).

The header shows, in view mode: a **refresh** dropdown and a **fullscreen** toggle
(next to the existing Edit button). Both are hidden in edit mode.

`useChartData` change: add an optional `nonce?: number` (or `unknown`) arg; include
it in the `specKey`/effect deps so a changed nonce forces a refetch. Default
behaviour (no nonce) is unchanged — Phase A callers and tests keep working.

## Data flow

```
DashboardView (view)
  ├─ useDashboardViewControls()  ⇄  URL (?refresh, ?fullscreen)
  │     └─ refreshNonce (ticks)
  └─ per card → DashboardCardView(card, refreshNonce)
        ├─ chart → useChartData(ws, chart, refreshNonce) → ChartRenderer
        ├─ text/heading → markdown / line from options
        └─ link → anchor tile

DashboardView (edit)
  └─ draftCards (Phase A) now also edited via inline option editors,
     + addText/addHeading/addLink, duplicateCard, replaceCard
     → Save = same whole-layout PUT (options travels in each CardInput)
```

## Error handling

- Link card with an empty/whitespace URL renders a disabled/placeholder tile (no
  broken anchor). Link opens with `rel="noreferrer"` `target="_blank"`.
- Text/heading with empty body render nothing (or a faint placeholder in edit
  mode) — never crash.
- Replace on a non-chart card is not offered.
- A bad `?refresh=` value (non-numeric) → treated as off.
- Chart-card behaviour (deleted chart, query error) is unchanged from Phase A.

## Testing

- **Vitest:** `TextCard` (renders markdown — heading/list/link), `HeadingCard`
  (single line), `LinkCard` (anchor href/target, empty-url disabled); the
  `DashboardCardView` dispatcher routes each `card_type`; per-card title override +
  hide (`""`); add text/heading/link; duplicate (count + new id); replace (swaps
  chart_id, resets options, keeps position); `useDashboardViewControls`
  (param parse, refresh nonce ticking with fake timers); `useChartData` refetches
  when `nonce` changes.
- **Live (Playwright):** add a text + heading + link card and a chart; set a card
  title and hide another; duplicate and replace a chart card; turn on auto-refresh
  (observe a re-fetch) and toggle fullscreen; Save → reload (with the URL params) →
  everything persists / restores; **0 console errors**.
- **Full sweep before each commit:** from `frontend/`,
  `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Out of scope (later phases)

Dashboard filters/parameters and `{{variable}}` text interpolation (Phase C);
click behaviour, tabs, public sharing (Phase D); night mode and internal-entity
link targets (cut).

## Conventions

Metabase as the standard · clean-room (no AGPL) · TDD · full sweep before each
commit · configured git identity (never `--author`), footer
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage
explicit paths · squash-merge the PR.
