# Dashboard Edit Mode — Metabase Polish (Design)

**Status:** Approved
**Date:** 2026-06-24
**Scope:** Frontend only (React/TS). No backend / API / migration changes.

## Problem

The dashboard *edit* page is close but not Metabase-faithful. Four issues, from
user feedback:

1. **Add-controls are scattered.** "Add chart / filter / width" live in the header
   row; "Add text / heading / link" live at the bottom of the right sidebar. The
   right sidebar should hold **only the chart list**; all *add* controls should
   move to a **top toolbar**.
2. **Heading cards show the grid squares.** In edit mode the SVG grid-cell
   background bleeds through heading cards (annoying visual noise).
3. **The Full/Fixed width toggle is not useful.** Replace it with a Metabase-style
   **Add section** feature (predefined card layouts).
4. **Cannot scroll to the bottom of the page in edit mode.** Content at the very
   bottom appears cut off / unreachable.

## Goals

- Edit-mode add controls consolidated into a top toolbar; right sidebar = chart
  list only.
- Heading cards render cleanly with no grid squares behind them.
- "Add section" inserts a heading + placeholder cards from one of three layout
  templates; clicking a placeholder fills it with a chart.
- Bottom-of-page content is reachable by scrolling in edit mode.

## Non-Goals

- No backend changes. `placeholder` cards never persist (dropped on save), so the
  cards API is untouched. The dashboard `width` field stays in the model — only
  its editing UI is removed.
- No change to view mode (read-only) layout or behavior, beyond the heading
  background being inert there.
- No new section templates beyond the three ported from Metabase.

## Current State (reference)

- `routes/explore/DashboardView.tsx` — edit bar, header row (`+ Chart`, `+ Filter`,
  `Full/Fixed` toggle backed by `draftWidth` state), the cards map, the right
  `EditSidebar`, and `addVirtualCard(type, options)` for text/heading/link.
- `components/chart/dashboard/EditSidebar.tsx` → `AddPanel.tsx` — "Add a chart" +
  `ChartPickerList` + a bottom `+ Text / + Heading / + Link` row.
- `components/chart/dashboard/DashboardGridLayout.tsx` — draws the SVG grid-cell
  background (`generateGridBackground`) at `z-0`; cards sit at `z-10`.
- Card wrapper in `DashboardView.tsx` (~line 426):
  `c.card_type === "heading" ? "" : "rounded-sm border bg-card"` — heading has **no
  background**, so the grid shows through.
- `components/chart/dashboard-grid.ts` — `GRID_COLS = 24`, `sizeForViz`,
  `getPositionForNewDashCard(cards, w, h)`, `CARD_SIZES_BY_VIZ` (heading default
  `{w:24,h:1}`, scalar `{w:6,h:3}`).
- `components/chart/dashboard/tabs.ts` — `DraftCard = DashboardCard & { tabKey }`;
  `serializeLayout(draftTabs, draftCards)` maps **every** draft card to the PUT
  payload.
- `card_type` is typed as `string` (both `DashboardCard.card_type` and via
  `DraftCard`), so adding `"placeholder"` needs **no type change**.

## Design

### Part 1 — Edit toolbar; sidebar = chart list only

New component `components/chart/dashboard/DashboardEditToolbar.tsx`:

- A horizontal strip rendered **directly under the "You're editing this dashboard"
  banner**, only while `editing`.
- Icon + label buttons, left-aligned, in this order: **Chart**, **Text**,
  **Heading**, **Link**, **Section** (dropdown), **Filter**.
- Props (callbacks the toolbar invokes; toolbar owns no business logic):
  - `onAddChart()` → `openPanel({ kind: "add" })` (reveals the right sidebar chart
    list; un-collapses it).
  - `onAddText()` / `onAddHeading()` / `onAddLink()` →
    `addVirtualCard("text"|"heading"|"link", …)` (existing logic, moved out of the
    sidebar).
  - `onAddSection(layout: SectionLayout)` → see Part 3.
  - `onAddFilter()` → existing add-filter logic (create param + open filter panel).
- Icons: reuse `lucide-react` already in the app — `BarChart3` (Chart),
  `Type` (Text), `Heading` (Heading), `Link2` (Link), `LayoutGrid` (Section),
  `Filter` (Filter). Buttons use the existing small ghost/outline button styling
  so the strip reads like a toolbar.
- The **Section** button is a dropdown. Use the existing dropdown primitive in the
  codebase (`DropdownMenu` from `components/ui/` if present; otherwise a small
  popover built with the existing `Popover`/positioned `div` pattern used
  elsewhere). Each item shows the template label and a tiny CSS preview of the
  layout (a few `div` blocks — no need to port Metabase's SVG preview).

`DashboardView.tsx` changes:

- In edit mode, the header `<div className="flex gap-2">` keeps **only Cancel and
  Save**. Remove the `+ Chart` button, the `+ Filter` button, and the Full/Fixed
  toggle block.
- Render `<DashboardEditToolbar … />` between the edit banner and the page title /
  filter chips area (it belongs to the editing chrome, above the canvas).

`AddPanel.tsx` changes:

- Remove the bottom `+ Text / + Heading / + Link` row and the `onAddVirtual` prop.
- The panel becomes: title "Add a chart" + `ChartPickerList`. (Title may be kept or
  simplified; keep "Add a chart".)

`EditSidebar.tsx` changes:

- Drop the `onAddVirtual` prop from `Props` and from the `AddPanel` usage. All other
  panels (card/filter/viz settings) are unchanged.

### Part 2 — Heading cards: no grid squares

The grid SVG background is painted by `DashboardGridLayout` at `z-0`, behind the
`z-10` cards. Chart/text/link cards carry `bg-card`, which masks the grid; the
heading branch carries no background.

Fix: in the card wrapper, give heading cards the **canvas background** so they paint
over the grid squares while staying visually seamless with the canvas (no border,
no card chrome — the Metabase look).

Change the wrapper className for `card_type === "heading"` from `""` to
`"bg-dashboard"`:

```tsx
// DashboardView.tsx card wrapper
className={`relative h-full overflow-hidden ${
  c.card_type === "heading" ? "bg-dashboard" : "rounded-sm border bg-card"
} ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}
```

- `bg-dashboard` is the canvas color, so the heading still looks like floating text
  on the canvas, but the grid cells in its footprint are covered.
- In view mode the grid is not drawn, so `bg-dashboard` is invisibly equal to the
  canvas — no regression. `HeadingCard.tsx` itself is unchanged.

### Part 3 — Add section (replaces Full/Fixed)

#### 3a. Section templates

New module `components/chart/dashboard/dashboard-sections.ts`, a clean-room port of
Metabase's `sections.ts`, expressed in egret's grid model:

```ts
export type SectionId = "kpi_grid" | "large_chart_kpi_right" | "kpi_chart_below";

export interface SectionLayout {
  id: SectionId;
  label: string;
  /** Cards (relative to an insertion origin) for this section: a heading + placeholders. */
  build: (origin: { col: number; row: number }) => SectionDraftCard[];
}

/** The grid+type fields a section produces. The caller adds id/tabKey/options. */
export interface SectionDraftCard {
  card_type: "heading" | "placeholder";
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
}
```

Constants (mirroring Metabase): `GRID_COLS = 24`, `HEADING_HEIGHT = 1`,
`SCALAR_W = 6`, `SCALAR_H = 3`.

Three templates (origin `{col, row}` is the top-left of the section; heading spans
full width at `row`, body starts at `row + HEADING_HEIGHT`):

1. **`kpi_grid` — "KPI grid":** heading (24×1) + a 2×2 grid of placeholders, each
   `12 × 5` (left/right halves, two rows). Body rows: `r=row+1`:
   `(0,r,12,5)`, `(12,r,12,5)`, `(0,r+5,12,5)`, `(12,r+5,12,5)`.
2. **`large_chart_kpi_right` — "Large chart w/ KPIs to the right":** heading (24×1)
   + one large placeholder `18 × 9` on the left, and a column of three `6 × 3`
   placeholders on the right. Body `r=row+1`: large `(0,r,18,9)`; right column
   `(18,r,6,3)`, `(18,r+3,6,3)`, `(18,r+6,6,3)`.
3. **`kpi_chart_below` — "KPIs w/ large chart below":** heading (24×1) + a row of
   four `6 × 3` placeholders, then a `24 × 9` placeholder below. Body `r=row+1`:
   `(0,r,6,3)`, `(6,r,6,3)`, `(12,r,6,3)`, `(18,r,6,3)`; large `(0,r+3,24,9)`.

These numbers are exact ports of Metabase's `getKpiGridLayout` /
`getLargeChartKpiColLayout` / `getKpiLargeChartBelowLayout` (with Metabase's
`scalarCardHeight = 5` for the KPI grid).

#### 3b. Placeholder card type & rendering

- `placeholder` is a draft-only `card_type` (no `chart_id`). It carries the grid
  position/size from the template.
- New component `components/chart/dashboard/PlaceholderCard.tsx`: a full-size
  dashed-border drop target with a centered "+" (and label "Add a chart"). Clicking
  anywhere on it calls `onPick()`.
- In `DashboardView.tsx`'s card map, when `c.card_type === "placeholder"`, render
  `<PlaceholderCard onPick={() => startFillPlaceholder(c.id)} />` **instead of**
  `<DashboardCardView>`. The placeholder wrapper has no `bg-card`/border (the
  `PlaceholderCard` draws its own dashed box); it still gets the drag handle and the
  `CardEditToolbar` (so a placeholder can be moved/removed like any card).

#### 3c. Insert + fill flow

- `addSection(layout)` in `DashboardView.tsx`:
  - Compute the section's vertical origin = `nextRow` below all cards on the active
    tab (the max of `grid_row + size_y` over the active tab's cards, or 0), at
    `col = 0`.
  - `build({col:0, row:nextRow})` → section cards; map each to a `DraftCard` with a
    fresh id (`new-section-<seq>`), `chart_id: null`, `options: {}`,
    `tab_id: null`, `tabKey: activeTab?.key ?? null`. Heading cards get
    `options: { text: "" }`.
  - Append to `draftCards`.
- **Fill:** clicking a placeholder sets `fillingPlaceholderId` and opens the chart
  picker. Reuse the existing **Replace chart dialog** (`ChartPickerList` inside the
  `Dialog`) — i.e. the same dialog used by `replacingId`, generalized to also handle
  filling a placeholder. On pick:
  - Replace the placeholder card in `draftCards` with a `chart` card:
    `{ ...placeholder, card_type: "chart", chart_id: chart.id, options: {} }` —
    **keeping the placeholder's `grid_col/row/size_x/size_y`** so the chart adopts
    the slot's footprint (not the chart's default size).
  - Clear `fillingPlaceholderId`.

#### 3d. Drop placeholders on save

- In `onSave`, before `serializeLayout`, filter out unfilled placeholders:
  `const persistCards = (cards as DraftCard[]).filter(c => c.card_type !== "placeholder")`.
  Pass `persistCards` to `serializeLayout`. Placeholders never reach the API.
- Any other code that serializes cards for a non-edit PUT (e.g. `saveDescription`)
  is unaffected — it rebuilds from `dashboard.cards` (server state), which never
  contains placeholders.

#### 3e. Remove the width toggle

- Delete the `Full/Fixed` toggle UI and the `draftWidth` state.
- `effectiveWidth` becomes `dashboard.width` in both view and edit modes (the
  fixed-width wrapper still honors a stored `"fixed"` value; new dashboards keep
  their default).
- `onSave` sends `width: dashboard.width` (unchanged), so existing dashboards keep
  their stored width and nothing breaks.

### Part 4 — Bottom-of-page scroll in edit mode

The scroll container is `AppShell`'s `<main className="… overflow-auto p-6">`. In
edit mode `DashboardView` lays out `flex gap-4` with the grid column
(`min-w-0 flex-1`) and the right `EditSidebar` (`sticky top-4
max-h-[calc(100vh-7rem)] overflow-auto`).

This is a "look at it" bug. The implementer **must reproduce it live** (Playwright,
a throwaway dashboard with enough cards to overflow the viewport) before fixing.
Likely fix (confirm against the repro): add bottom breathing room to the edit
canvas so the last card row + its resize handle clear the viewport bottom and are
fully scrollable — e.g. a `pb-24` (or a spacer div) on the editing grid column, or
removing/raising a height cap that clips the column. The fix must:

- Let the last row of cards (and its bottom resize handle) be scrolled fully into
  view in edit mode.
- Not change view-mode spacing.

If the repro reveals a different root cause (e.g. an `overflow: hidden` ancestor or
the sticky sidebar capping the flex row), fix that instead — the requirement is
"bottom content reachable", not a specific CSS change.

## Testing

**Unit (Vitest):**

- `components/chart/dashboard/__tests__/dashboard-sections.test.ts`:
  - Each of the 3 templates returns the expected card count and exact positions:
    - `kpi_grid`: 1 heading (`0,row,24,1`) + 4 placeholders at the positions in 3a.
    - `large_chart_kpi_right`: 1 heading + 1 large (`0,r,18,9`) + 3 right
      (`18,r,6,3`), (`18,r+3,6,3`), (`18,r+6,6,3`).
    - `kpi_chart_below`: 1 heading + 4 top (`0/6/12/18,r,6,3`) + 1 large
      (`0,r+3,24,9`).
  - Origin offset respected: `build({col:0, row:7})` shifts every card's `grid_row`
    by 7 (heading at row 7, body from row 8).
- A test for **drop-on-save**: filtering `card_type !== "placeholder"` removes
  placeholders from the serialized payload while keeping chart/heading cards. (Test
  the filter helper or the `serializeLayout` input directly.)

**Live verification (Playwright, manual driving, 0 console errors):**

- On a **throwaway** dashboard (never the user's `a1`):
  1. Toolbar: Chart/Text/Heading/Link/Section/Filter all present under the edit
     banner; right sidebar shows only the chart list.
  2. Add a Heading → no grid squares behind it.
  3. Add section (each of the 3 templates) → heading + placeholders appear; click a
     placeholder → chart picker → chart fills the slot at the slot's size.
  4. Save → reload → placeholders are gone, filled charts persist, heading persists.
  5. Scroll to the very bottom in edit mode → last row fully reachable.
- Delete the throwaway dashboard afterward. Scratch screenshots go to the parent
  `data-portal/` dir, not the repo, and are cleaned up.

## Files

**New:**
- `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx`
- `frontend/src/components/chart/dashboard/dashboard-sections.ts`
- `frontend/src/components/chart/dashboard/PlaceholderCard.tsx`
- `frontend/src/components/chart/dashboard/__tests__/dashboard-sections.test.ts`

**Modified:**
- `frontend/src/routes/explore/DashboardView.tsx` (toolbar wiring, addSection,
  placeholder render + fill, drop-on-save, remove width toggle/`draftWidth`, heading
  bg, bottom-scroll fix)
- `frontend/src/components/chart/dashboard/AddPanel.tsx` (chart list only)
- `frontend/src/components/chart/dashboard/EditSidebar.tsx` (drop `onAddVirtual`)

**Backend:** none.

## Risks / Notes

- `placeholder` is intentionally draft-only; ensure **every** path that reads cards
  for rendering or value-source logic in edit mode tolerates a card with
  `chart_id: null` and `card_type: "placeholder"` (it already filters
  `card_type === "chart"` in the param/value-source code, so placeholders are
  naturally skipped — verify during implementation).
- Keep the heading default size (`24×1`) and `bg-dashboard` token usage consistent
  with the existing OKLCH token convention (use `bg-dashboard`, not a literal
  color).
