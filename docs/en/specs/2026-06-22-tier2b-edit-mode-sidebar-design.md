# Tier 2b — Dashboard edit-mode overhaul (Metabase-style right sidebar) (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-22 · **Branch:** `feat/dashboards-tier2b-edit-ux` (off main)
**Part of:** the dashboard Metabase-parity program ([audit backlog](./2026-06-22-dashboard-metabase-parity-audit.md))

## Goal

Make the dashboard **edit mode** look and feel like Metabase. Today every card is buried under a
stack of inline controls (Move-to-tab, Title, per-param mapping, On-click) and adding a chart is a
wall of ~47 buttons. Replace this with: **clean cards** (just the chart + a hover toolbar) and a
**right sidebar** that swaps by context — add a chart (searchable), configure the selected card, or
configure the selected filter. Frontend-only; view mode + the data model are unchanged.

## Metabase grounding

Metabase edit mode: a fixed **right sidebar** whose content depends on what you're doing — a
searchable question picker when adding, a card-settings panel when a card is selected, a filter
panel when a filter is selected; cards themselves are clean with a hover action menu. egret adopts
the same shape using its existing primitives (no new deps).

## Scope

**In:** an `editPanel` sidebar state machine (`add` / `card` / `filter`); a searchable Add panel;
a Card-settings panel (title, per-filter column mapping, on-click, move-to-tab); a Filter-settings
panel (name/type/operator/required + delete); clean cards with a hover `CardEditToolbar`
(settings/duplicate/replace/remove) + selected-card highlight; an edit top-bar (`+ Chart`,
`+ Filter`, Width toggle, Cancel, Save); edit-mode filter **chips** that open the filter panel.

**Out:** view mode (unchanged — filter value pills, etc.); the data model + save payload
(unchanged); Metabase's click-to-map-by-clicking-cards interaction (egret keeps per-card column
mapping, just relocated to the card panel); action buttons / per-column click / sections (other
tiers). No backend change.

## Components & state

All existing draft state and mutators in `DashboardView` are reused unchanged: `draftCards`,
`draftParameters`, `draftTabs`, `draftWidth`, `addCard`, `addVirtualCard`, `duplicateCard`,
`replaceCard` (+ `replacingId` dialog), `removeCard`, `updateCardOptions`, `moveCardToTab`,
`setDraftParameters`, and the param helpers (`slugify`, `applyParamsToChart`). This tier moves
**where** those are triggered (into the sidebar), not what they do.

### New `editPanel` state (in `DashboardView`, above the early return)
```ts
type EditPanel =
  | { kind: "add" }
  | { kind: "card"; cardId: string }
  | { kind: "filter"; paramId: string }
  | null;
const [editPanel, setEditPanel] = useState<EditPanel>(null);
```
- `startEdit` sets `editPanel` to `{ kind: "add" }` (sidebar starts on Add). `cancelEdit`/`onSave`
  reset it to `null`.
- Guard against stale targets: if `editPanel.kind === "card"` and no draft card has that id (e.g.
  removed), or `"filter"` with no matching param, fall back to `{ kind: "add" }`.

### Edit top-bar (replaces the old `<aside>` "Add a chart" wall)
A compact toolbar in the header (view-mode header is unchanged):
- `+ Chart` → `setEditPanel({ kind: "add" })`.
- `+ Filter` → create a new draft param (`slugify("filter", existing)`, `{ type:"text",
  operator:"is" }`) appended to `draftParameters`, then `setEditPanel({ kind: "filter", paramId })`.
- the existing **Width** Full/Fixed toggle (Tier 2a), **Cancel**, **Save**.

### Edit filter chips (new `EditFilterChips.tsx`)
In edit mode only, a row of chips — one per `draftParameters` entry (chip label = param name) —
that on click call `onSelect(param.id)` → `setEditPanel({ kind: "filter", paramId })`. The chip for
the currently-selected filter is highlighted. (No value popovers in edit mode — that's view mode.)
```ts
interface EditFilterChipsProps { parameters: DashboardParameter[]; activeId: string | null; onSelect: (id: string) => void }
```

### Clean cards + `CardEditToolbar.tsx`
In edit mode, every card renders its **display** via `DashboardCardView` (chart, or rendered
text/heading/link) — no inline inputs. A `CardEditToolbar` overlays the top-right on hover:
```ts
interface CardEditToolbarProps {
  cardType: string;           // show ↺ replace only for "chart"
  onSettings: () => void;     // ⚙ → setEditPanel({ kind:"card", cardId })
  onDuplicate: () => void;    // ⧉
  onReplace: () => void;      // ↺  (chart only)
  onRemove: () => void;       // ✕
}
```
The card wrapper gets `ring-2 ring-primary` when `editPanel.kind === "card" && cardId === c.id`.
(`DashboardCardView` is extended to render text/heading/link displays in edit mode too — today it
short-circuits to inline inputs; the inputs move to the card panel.)

### Right sidebar (`EditSidebar.tsx` + three panels)
A fixed-width (`w-[340px] shrink-0 border-l bg-card`) panel rendered beside the grid in edit mode.
`EditSidebar` switches on `editPanel.kind`:

- **`AddPanel`** — a search `Input`; a scrollable list of charts (`useCharts`, filtered by
  name-includes-search, case-insensitive) where each row click calls `addCard(chart)` (panel stays
  open for multi-add); `+ Text` / `+ Heading` / `+ Link` buttons calling `addVirtualCard(...)`.
- **`CardSettingsPanel`** — for the selected card (looked up in `draftCards`):
  - chart card: **Title** input (`updateCardOptions(id, { title })`); a **Filters** section with one
    column `<select>` per dashboard param (value = `options.paramMappings[param.id]`, options =
    the chart's `dimensions + columns`), writing `paramMappings`; an **On click** section
    (menu / cross-filter / custom-URL — the existing config relocated); a **Move to tab** select
    (when `tabs.length >= 2`, calls `moveCardToTab`).
  - text/heading card: the **text** input. link card: **url** + **label** inputs.
- **`FilterSettingsPanel`** — the existing `ParameterEditor` fields laid out vertically (name, type,
  operator, required) for the selected param, plus a **Delete filter** button (removes it from
  `draftParameters` and sets `editPanel` to `{ kind: "add" }`).

`EditSidebar` receives the relevant slices + callbacks from `DashboardView` (it owns no draft
state). Panels are small, focused, separately testable.

## Layout (edit mode)
```
<canvas bg-muted/30 p-4>
  <edit top-bar: title · [+ Chart][+ Filter][Full|Fixed][Cancel][Save]>
  <EditFilterChips …>
  <DashboardTabBar … editing>
  <div class="flex gap-4">
     <div class="min-w-0 flex-1"> {grid of clean cards + CardEditToolbar} </div>
     <EditSidebar class="w-[340px] shrink-0">
  </div>
</canvas>
```
View mode is the existing single-column layout (no sidebar). The Tier-2a fixed-width wrapper
applies to view mode; in edit mode the sidebar coexists with a full-width grid column.

## Data flow

```
edit top-bar / card toolbar / filter chip → setEditPanel(…)
EditSidebar(editPanel) → AddPanel | CardSettingsPanel | FilterSettingsPanel
  AddPanel → addCard / addVirtualCard            (existing draft mutators)
  CardSettingsPanel → updateCardOptions / moveCardToTab
  FilterSettingsPanel → setDraftParameters
onSave → existing PUT { name, description, parameters, tabs, cards, width }  (unchanged)
```

## Error handling

- Stale `editPanel` (card/filter removed) → reset to `{ kind: "add" }` (no crash, no blank panel).
- Removing the selected card / deleting the selected filter closes/swaps the panel.
- Adding a chart while on a tabbed dashboard puts it on the active tab (existing `addCard` rule).
- A dashboard with no charts available shows "No charts yet" in the Add panel.
- View mode is untouched: filter value pills, drill/click behaviour (D1), cross-filter,
  required-gating, fullscreen, auto-refresh, tabs — all unchanged and their tests stay green.
- Rules of Hooks: `editPanel` `useState` goes above the `if (isLoading || !dash) return …` guard.

## Testing

- **Vitest:**
  - `AddPanel`: typing in search filters the chart list; clicking a chart calls `onAdd(chart)`;
    `+ Text`/`+ Heading`/`+ Link` call `onAddVirtual(type)`.
  - `CardSettingsPanel`: for a chart card renders Title + a mapping select per param + On-click +
    Move-to-tab, and editing each calls the right callback with the right args; for a text card
    renders the text input.
  - `FilterSettingsPanel`: renders name/type/operator/required for the param; editing calls
    `onChange`; Delete calls `onDelete`.
  - `EditFilterChips`: one chip per param; click calls `onSelect(id)`; the active chip is marked.
  - `DashboardView` (edit): entering edit shows the sidebar on Add; clicking a card's settings (⚙)
    opens the card panel + highlights the card; `+ Filter` appends a param and opens its filter
    panel; clicking a filter chip opens its panel; cards render their display (no inline title
    input on the card face). Existing `DashboardView.*` suites stay green (view mode unchanged).
- **Live (Playwright MCP, side-by-side with Metabase `localhost:3001/dashboard/1` edit mode):**
  enter edit → clean cards + right sidebar; add a chart via search; select a card → configure
  title/mapping/on-click/move-to-tab in the sidebar; add + configure a filter; reorder/rename/
  delete tabs still work; Save → persists; view mode unchanged. **0 console errors.**
- **Full sweep** before each commit (from `frontend/`): `npm run typecheck && npm run lint &&
  npm run build && npx vitest run`.

## Conventions

Metabase as the visual standard · clean-room (no AGPL) · TDD · full sweep before each commit ·
reuse existing Radix primitives (`scroll-area`, `input`, `button`, `select`, `popover`) — no new
deps · configured git identity (never `--author`), footer `Co-Authored-By: Claude Opus 4.8
(1M context) <noreply@anthropic.com>` · stage explicit paths · squash-merge · branch
`feat/dashboards-tier2b-edit-ux`.
