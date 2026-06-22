# Dashboard Metabase-parity audit & program backlog

**Status:** living backlog · **Date:** 2026-06-22 · **Branch:** `fix/dashboard-parity-gaps`

Driven by a side-by-side audit of egret (seeded `PARITY — all features` dashboard) vs a running
Metabase (`E-commerce Insights`, dashboard 1) + a full inventory of Metabase's dashboard source.
Goal: bring egret dashboards to **functional + visual** parity with Metabase.

## Fixed this session

- **Category/ID filter dropdowns 400'd** (`useColumnValues` built a dimensions-only query the
  engine rejects). Now selects the column directly (`columns:[col]`). Live-verified; 369 tests.
  Commit `ed25aa6`.

## Functional gap matrix (egret vs Metabase)

| Area | egret | Gap |
|---|---|---|
| Viz types | 17 (line/area/bar/pie/table/scalar/gauge/progress/pivot/funnel/waterfall/scatter/boxplot/row/sankey/combo/smartscalar) | **map**, object-detail |
| Virtual cards | text / heading / link | **iframe** |
| Grid | 24-col, drag/resize, title override, duplicate/replace | fixed-vs-full width, section templates |
| View controls | auto-refresh, fullscreen, URL-sync (`?slug/tab/refresh/fullscreen`) | **night/theme mode**, **PDF/print export**, sticky param panel |
| Parameters | text/number/date/id/category + operators, per-card mapping, multi-select, URL persist | **value sources** (static list / from-card), **linked filters**, boolean & location types, required-enforcement, search-vs-list widget |
| Click behaviour | menu / cross-filter / custom-URL | link-to-dashboard, link-to-question, **action buttons**, per-column |
| Tabs | full parity (add/rename/reorder/delete+reassign/duplicate/move-card, lazy, `?tab=`) | id+slug tab URL (egret uses position) |
| Sharing | — | **public link**, static embed, **subscriptions/alerts** (previously CUT) |
| Management | create/edit/delete, duplicate-card | dashboard copy/duplicate, archive, caching, bookmarking, info sidebar |

## Visual/style gaps (make egret look like Metabase)

Reference: Metabase dashboard = light-gray canvas, white cards with subtle border + soft shadow,
generous padding, card title top-left, hover `⋯` menu top-right.

1. **Dashboard canvas** — egret is plain white; Metabase uses a light-gray bg (`~#f9fafb`) so white cards pop. → add a muted canvas bg.
2. **Cards** — egret cards: thin border + header row with a divider (boxy). Metabase: rounded card, soft shadow, no divider, title inline, more padding. → restyle the card shell.
3. **Filter widgets** — egret: bordered box of raw native `<select>`/`<select multiple>` (ugly). Metabase: a row of rounded **filter pills** (type icon + name + value summary like "Product Category: 4 selections") that open a **popover** with a search box + checkbox list (category), date picker, etc. → biggest visual+UX gap; build a `FilterPill` + popover widgets.
4. **Tabs** — egret active tab is dark underline; Metabase active is **brand-colored** underline + lighter inactive. → tweak DashboardTabBar styling.
5. **Scalars** — egret big-number is navy; Metabase colors them (and deltas green/red). → already mostly there for smartscalar; align scalar.
6. **Spacing/density** — Metabase is airier (card gaps, padding). → bump grid gap + card padding.
7. **Header** — Metabase has icon actions (edit/share/history/bookmark/info/⋯); egret has text buttons. → optional, lower priority.

## Tiered execution plan

Each tier = its own brainstorm → spec (en+vi) → plan → subagent-driven cycle, with a checkpoint.

- **Tier 1 — bugs + Metabase-style restyle (UX polish).** Remaining bugs; canvas bg + card shell + spacing; **filter pills + popover widgets** (category search+checkbox, date, number, text); brand-colored tabs; required-param enforcement; empty-tab state. *Highest visual payoff, low risk.*
- **Tier 2 — medium features.** Night/theme mode; PDF/print export; dashboard duplicate/copy; parameter value sources (static list + from-card); linked filters; map viz; fixed/full width.
- **Tier 3 — heavy / previously-cut.** Public sharing + static embed; email/Slack subscriptions & alerts; action buttons; link-to-dashboard/question click; per-column click. *Multi-day; needs new infra (share tokens, email, actions).*

## Notes

- Seeded test dashboard: `PARITY — all features` (eng ws) — delete when done.
- URL-shape differences are intentional (egret `?tab=<position>` + `?slug=value`; Metabase `?tab=<id>-<slug>` + repeated `key=val`). Not a parity blocker.
