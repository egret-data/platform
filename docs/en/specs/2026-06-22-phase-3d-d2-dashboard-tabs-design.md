# Phase 3d-D2 — Dashboards: tabs (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-22 · **Builds on:** Phase A/B/C (merged PRs #25/#26/#27, `903df67`) and D1 (click behaviour, PR #28)

## Goal

Let a dashboard hold multiple named **tabs**, each with its own grid of cards. A tab bar
switches between them; in edit mode the user can add, rename, reorder, delete, and duplicate
tabs, and move a card to another tab. The second-most-distinctive Metabase dashboard feature
after filters — built to the same fidelity bar as A/B/C/D1.

Phase D is decomposed into independent sub-phases; this is **D2 (tabs)**. **D1 (click
behaviour)** shipped in PR #28. Public sharing/embedding is **out of scope for Phase D**
(per product decision — egret is an internal tool).

## Metabase grounding (clean-room)

Read the Metabase source (`data-portal/metabase`):

- **Data model**: a `dashboard_tab` table `(id, dashboard_id, name, position, entity_id,
  created_at, updated_at)` (`resources/migrations/001_update_migrations.yaml`, v47.00-006),
  plus a **nullable** `dashboard_tab_id` FK with `ON DELETE CASCADE` on the dashcard row
  (v47.00-007). A dashboard with **zero tabs** keeps its cards at `dashboard_tab_id = null`
  and shows no tab bar; a dashboard cannot mix tabs and null-tab cards.
- **Save**: a single whole-dashboard `PUT /api/dashboard/:id` carries `{tabs, dashcards,
  parameters}` together (`dashboards_rest/api.clj`); there are no dedicated tab endpoints.
  New tabs are sent with temporary ids and the response returns a temp→real id mapping
  (`do-update-tabs!`).
- **Semantics**: each card belongs to exactly one tab; each tab is an independent grid
  (card row/col scoped within the tab); dashboard **parameters are dashboard-level**, shared
  across all tabs, and filter values persist when switching tabs.
- **UX**: the tab bar shows in view mode only when there are **≥2 tabs**, and always in edit
  mode. Edit affordances: add (`+`), rename (double-click), reorder (drag), delete (with a
  confirm; enabled only at ≥2 tabs), duplicate tab, and move-card-to-tab. The active tab is
  reflected in the URL. Card data **loads lazily per active tab** — only the selected tab's
  cards are fetched, and revisiting a tab reuses cached data.

### egret today (what D2 extends)

- `dashboards` table (+ `parameters jsonb` from migration 0016) and a **normalized**
  `dashboard_cards` table (`grid_col/grid_row/size_x/size_y`, `options jsonb`, `ordinal`).
- Whole-layout save: `DashboardRepo::replace_cards` deletes all of a dashboard's cards and
  re-inserts the supplied set in one transaction (`crates/egret-bi/src/dashboard.rs`); the
  PUT body is `{name, description, parameters, cards}`.
- `DashboardView` holds `draftCards` / `draftParameters` in edit mode and saves the whole
  layout; Phase B/C established URL-synced view state (`?refresh`, `?fullscreen`, `?slug=`).

## Scope (this PR)

**In:** a `dashboard_tabs` table + nullable `tab_id` on cards; tabs in the GET response and
the whole-save PUT; a tab bar (view + edit); add / rename / reorder / delete (with reassign)
/ duplicate tab; move-card-to-tab; lazy per-tab card rendering; active tab synced to the URL.

**Deferred (named):** Metabase's `entity_id`/timestamps on the tab table (egret has no
serialization/export system and `dashboard_cards` has no timestamps); id+slug tab URLs
(D2 addresses the active tab by **position**); deleting a dashboard back down to zero tabs
(delete is allowed only at ≥2 tabs); per-tab parameters (parameters stay dashboard-level).

## Data model

### Migration `0017_dashboard_tabs.sql`

```sql
CREATE TABLE dashboard_tabs (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id uuid NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
    name         text NOT NULL,
    position     int  NOT NULL DEFAULT 0
);
CREATE INDEX dashboard_tabs_dashboard_idx ON dashboard_tabs (dashboard_id);

ALTER TABLE dashboard_cards
    ADD COLUMN tab_id uuid REFERENCES dashboard_tabs(id) ON DELETE CASCADE;
```

`tab_id` is **nullable** — the Metabase "implicit single view". A dashboard with **zero
tabs** keeps all cards at `tab_id = null` and shows no bar, so **every existing dashboard
keeps working with no data migration**. The bar appears once a dashboard has ≥1 tab.

> Columns deliberately omitted vs Metabase: `entity_id` (no egret serialization system) and
> `created_at/updated_at` (egret's `dashboard_cards` has none either — staying consistent).

### Tab identity on save — delete-all + address-by-position

Tabs follow egret's existing card pattern: the whole-save **deletes all of a dashboard's
tabs and re-inserts** the supplied set in order. This mints new tab uuids on every save
(unlike Metabase's id-preserving diff), so the frontend never relies on a stable tab id —
it **addresses the active tab by position**, both in component state and in the URL
(`?tab=<position>`). This is the one deliberate divergence from Metabase's id-based
addressing; the user-visible behaviour (deep-link to a tab, switch tabs, edit + save)
is identical, and it keeps the backend as simple as `replace_cards`.

## Backend

### Repo (`crates/egret-bi/src/dashboard.rs`)

- `Dashboard` GET gains a `tabs: Vec<DashboardTab>` where `DashboardTab { id, name, position }`
  (ordered by `position`). Each `DashboardCard` gains `tab_id: Option<Uuid>`.
- New input types: `NewTab { name: String, position: i32 }`; `NewCard` gains
  `tab_index: Option<usize>` (0-based index into the supplied `tabs`, or `None` for a
  tabless dashboard).
- New `replace_layout(dashboard_id, tabs: &[NewTab], cards: &[NewCard])` (replaces
  `replace_cards`) runs in **one transaction**:
  1. `DELETE FROM dashboard_cards WHERE dashboard_id = $1` (cards first — FK to tabs);
  2. `DELETE FROM dashboard_tabs WHERE dashboard_id = $1`;
  3. insert each tab in order, collecting the new uuids into a `Vec<Uuid>` indexed by position;
  4. insert each card with `tab_id = tab_index.map(|i| tab_ids[i])` (validated in range;
     out-of-range or a non-empty `tab_index` with an empty `tabs` list → `BiError::Invalid`).
- `cards()` gains `tab_id` to its `SELECT`; a `tabs(dashboard_id)` query returns the tab rows.

### API (`dashboard_router`)

- GET response DTO gains `tabs: [{id, name, position}]`; each card DTO gains `tab_id`.
- PUT body becomes `{name, description, parameters, tabs: [{name, position}],
  cards: [{…, tab_index}]}`. The handler runs `update` (metadata + parameters) and
  `replace_layout` (tabs + cards) together, exactly as today it runs `update` + `replace_cards`.
- Validation: a non-empty `tabs` requires every card to have a `tab_index`; an empty `tabs`
  requires every card's `tab_index` to be `None`. Reject mismatches with 400 (mirrors
  Metabase's "can't mix tabs and null-tab cards").

## Frontend

### Types & API (`frontend/src/api/dashboards.ts`)

- `DashboardTab { id: string; name: string; position: number }`.
- `DashboardDetail` gains `tabs: DashboardTab[]`; `DashboardCard` gains `tab_id: string | null`.
- `CardInput` gains `tab_index: number | null`; the PUT body gains `tabs: { name; position }[]`.

### Active tab + URL (`?tab=<position>`)

- A small hook (mirroring `useDashboardViewControls`) reads/writes `?tab=<position>` and
  exposes `activeTabPos` + `setActiveTabPos`. Default `0`; values out of range clamp to `0`.
- **All new hooks are declared above** the `if (isLoading || !dash) return …` early return
  (the Rules-of-Hooks lesson that has bitten `DashboardView` before).

### Tab bar (`DashboardTabBar`)

- Renders the tabs ordered by position. **Visible** in view mode only when `tabs.length ≥ 2`;
  **always** visible in edit mode (so the first tab can be added). A tabless dashboard renders
  exactly as today (no bar).
- View mode: clicking a tab sets `activeTabPos`. Edit mode adds the affordances below.

### Active-tab card filtering + lazy render

- **View mode:** `activeTab = tabs[activeTabPos]`;
  `visibleCards = tabs.length === 0 ? cards : cards.filter(c => c.tab_id === activeTab.id)`.
- **Edit mode:** `activeTab = draftTabs[activeTabPos]`; filter draft cards by the client tab
  key (`card.tabKey === activeTab.key`); a tabless draft shows all cards.
- Only `visibleCards` are mounted, so only the active tab's card queries fire. Switching tabs
  unmounts the previous tab's cards and mounts the new tab's; React Query caches results, so
  revisiting a tab is instant.

### Edit-mode draft state & affordances

- Add `draftTabs: DraftTab[]` alongside `draftCards`/`draftParameters`. A `DraftTab` has a
  client key (`new-tab-<n>`), `name`, `position`; draft cards reference their tab by that
  client key (`card.tabKey`). `startEdit` builds `draftTabs` from the server `tabs` (each
  gets a fresh client key) and maps every draft card's server `tab_id` to the matching tab's
  key (a tabless dashboard yields empty `draftTabs` and `card.tabKey = null`). On save, tabs
  are serialized in position order and each card's `tab_index` is the index of its tab key
  (or `null` when there are no tabs).
- **Add** (`+`): on a tabless dashboard that has cards, the first add creates **Tab 1 holding
  the existing cards + Tab 2 empty** and activates Tab 2 (Metabase's first-tab behaviour);
  every later add appends one empty tab and activates it.
- **Rename**: double-click the tab label → inline text input (Enter/blur commits to the draft).
- **Reorder**: drag tabs to change `position` in the draft.
- **Delete**: a per-tab control, enabled only when `draftTabs.length ≥ 2`. If the tab has
  cards, a confirm dialog offers **Reassign** (move its cards to a chosen other tab) or
  **Delete cards** (drop them with the tab). After delete, `activeTabPos` clamps into range.
- **Duplicate tab**: deep-copies the tab's cards (fresh client ids, same grid positions) into
  a new tab inserted after the source, and activates it.
- **Move card to tab**: a per-card edit action (select another tab) that reassigns the card's
  tab key and recomputes its grid slot on the destination tab's 0-based grid (place below the
  destination's current max row, like `addCard`).

### Grid

Each tab is its own 24-col, 0-based grid. The existing grid component is unchanged — it is
simply fed `visibleCards` (the active tab's cards) instead of all cards.

## Data flow

```
GET dashboard → { …, tabs:[{id,name,position}], cards:[{…, tab_id}] }
  view: activeTabPos (?tab=) → activeTab = tabs[pos] → mount cards where tab_id===activeTab.id
  edit: draftTabs + draftCards(client tab key) → tab bar affordances mutate the draft
  save: PUT { …, tabs:[{name,position}], cards:[{…, tab_index}] }
        → replace_layout: delete cards+tabs; insert tabs (capture ids by index);
                          insert cards with tab_id = tab_ids[tab_index]
```

## Error handling

- PUT with a non-empty `tabs` but a card missing `tab_index` (or empty `tabs` with a card
  carrying `tab_index`) → 400 (can't mix tabs and null-tab cards).
- `tab_index` out of range → 400.
- Delete is disabled at 1 tab (can't delete down to zero via the UI).
- Switching to a tab whose cards have errored shows each card's own error state (unchanged
  per-card behaviour); the tab bar itself never errors.
- A tabless dashboard (no `tabs`) is unchanged from today — Phase A/B/C/D1 behaviour intact.

## Testing

- **Vitest:** tab bar renders/orders by position and is hidden in view at <2 tabs but shown
  in edit; clicking a tab switches `activeTabPos` and syncs `?tab=`; only the active tab's
  cards are mounted (lazy); each draft mutation (add incl. the first-tab split, rename,
  reorder, delete with reassign vs delete-cards, duplicate, move-card) produces the right
  `draftTabs`/`draftCards`; the save payload serializes `tabs` + per-card `tab_index`
  correctly; out-of-range `?tab=` clamps.
- **Backend (`egret-bi` repo test):** `replace_layout` inserts tabs in order, resolves each
  card's `tab_id` by `tab_index`, round-trips through `tabs()`/`cards()`, cascades on tab and
  dashboard delete, and rejects mixed/out-of-range input.
- **Live (manual, Playwright MCP browser — egret has no automated Playwright suite):** add
  tabs to a dashboard, move cards between tabs, rename/reorder/delete (reassign), duplicate;
  switch tabs (only the active tab's cards load); deep-link `?tab=`; confirm a tabless
  dashboard is unaffected. **0 console errors.**
- **Full sweep** before each commit: from `frontend/`, `npm run typecheck && npm run lint &&
  npm run build && npx vitest run`; for backend changes, `cargo test -p egret-bi` and the
  manual Trino/StarRocks check is **not** required (no engine/catalog change).

## Out of scope (later / other phases)

Public sharing/embedding; per-tab parameters; id+slug tab URLs; Metabase's tab `entity_id`
and serialization; deleting a dashboard back to zero tabs.

## Conventions

Metabase as the standard · clean-room (no AGPL) · TDD · full sweep before each commit ·
configured git identity (never `--author`), footer `Co-Authored-By: Claude Opus 4.8
(1M context) <noreply@anthropic.com>` · stage explicit paths · squash-merge · branch
`feat/dashboards-phase-d2` (after D1 merges).
