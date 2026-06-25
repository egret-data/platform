# Dashboard Editor — Metabase Parity, Round 2 (Design)

**Status:** Approved
**Date:** 2026-06-24
**Scope:** Frontend only (React/TS). No backend / API / migration changes.

## Problem

Egret's dashboard editor works but doesn't *look/feel* like Metabase. Compared live
(egret `DashboardView` vs Metabase dashboard edit mode), the differences are:

1. **Add controls** — Metabase uses compact, right-aligned **icons on the title row**
   (`+` questions, `T▾` heading/text, `🔗` link, `▦` section, `⏚` filter). Egret uses a
   full-width **labeled strip** below the banner.
2. **Cancel/Save** — Metabase puts them in the **edit banner** (top, right). Egret puts
   them on the title row.
3. **Text + Heading** — Metabase combines them into one **`T▾` dropdown**. Egret has two
   separate buttons.
4. **Right "Add cards" sidebar** — Metabase opens it **on demand** (`+`). Egret opens it
   by default on entering edit.
5. **Tabs** — Metabase gives each tab a **`▾` menu** (rename/duplicate/delete). Egret uses
   double-click-to-rename + hover icons.
6. **Dashboard name** — Metabase lets you **edit the title inline** in edit mode. Egret
   doesn't.
7. **Section previews** — Egret's "Add section" previews render as flat solid blocks; they
   should show the actual mini-layout like Metabase.
8. **Card settings access** — Egret forces you to hover and click the **gear** to configure
   a card. Metabase lets you **click the card** to select/configure it.

## Goals

Make the editor read like Metabase: a compact icon toolbar on the title row, Cancel/Save in
the banner, combined `T▾`, on-demand sidebar, per-tab `▾` menu, inline name editing, clear
section previews, and click-to-select cards (no gear hunting).

## Non-Goals

- No backend changes. `update` already accepts `name`, so inline name editing needs no API
  change. Placeholder cards stay draft-only (unchanged from round 1).
- Keep the Full/Fixed width toggle **removed** (round-1 decision; not reintroduced even
  though Metabase has one).
- No iframe card type (Metabase's `🔗▾` is "link or iframe"; egret keeps link only).
- No `⋯` "more" menu in edit mode (egret's view controls already live in view mode).

## Current State (anchors)

`routes/explore/DashboardView.tsx`:
- Edit banner (~line 279): `bg-primary/90` strip "You're editing this dashboard", **no
  buttons**.
- `<DashboardEditToolbar>` rendered right after the banner (round 1): a full-width labeled
  strip.
- Title row (~line 290): `<h1 className="text-xl font-bold">{dash.name}</h1>` + a header
  button group that, **while editing**, holds Cancel + Save; in view mode holds
  bookmark/refresh/fullscreen/info/Edit.
- `startEdit` (~line 125): sets `setEditPanel({ kind: "add" })` and `setDraftWidth`-removed.
- Sidebar (~line 452): `editing && (sidebarCollapsed ? <expand pill> : <EditSidebar>)` —
  defaults to **open**. `sidebarCollapsed` state + `openPanel` (sets panel and
  `setSidebarCollapsed(false)`).
- Card map (~line 415): `<div className="group relative h-full">` → floating
  `CardEditToolbar` (`-top-3.5`) + inner `<div class="...overflow-hidden...">` (drag handle
  + `PlaceholderCard`/`DashboardCardView` + `CardFilterMapOverlay`). The inner wrapper gets
  `ring-2 ring-primary` when `editPanel.kind === "card" && editPanel.cardId === c.id`.
- `onSave` (~line 231): sends `name: dashboard.name`.

`components/chart/dashboard/DashboardEditToolbar.tsx`: labeled buttons Chart/Text/Heading/
Link/Section(dropdown)/Filter + `SectionLayoutPreview` (scaled mini-rects, currently low
contrast).

`components/chart/dashboard/DashboardTabBar.tsx`: per-tab button, double-click → rename
input; hover `Copy`/`X` icons; drag reorder; `+` add. Props: `onRename/onReorder/onDelete/
onDuplicate/onAdd/onSelect`.

`components/chart/dashboard/CardEditToolbar.tsx`: `Settings`(gear) · `BarChart3`(viz,
chart-only) · `Copy`(duplicate) · `RefreshCw`(replace, chart-only) · `Trash2`(remove).

`components/chart/dashboard/EditSidebar.tsx`: `aside` with a header (back-to-add / "Add
cards" + collapse button) and the active panel body. Props include `onClose` (→ back to
add), `onCollapse`.

`api/dashboards.ts`: `update` mutation body includes `name: string` (line ~105).

## Design

### A. Edit chrome relayout (banner + title-row icon toolbar)

1. **Banner holds Cancel/Save.** The edit banner becomes a flex row: left = "✎ You're
   editing this dashboard"; right = `Cancel` + `Save` buttons styled for the primary bg
   (e.g. `variant="ghost"`/outline with `text-primary-foreground` / a light Save). Only
   rendered while editing.
2. **Title row hosts the icon toolbar (edit) / view controls (view).** Remove the separate
   full-width `DashboardEditToolbar` strip. In edit mode, the title row's right side renders
   the **icon toolbar** (refactored `DashboardEditToolbar`, see B). In view mode, the right
   side keeps the existing bookmark/refresh/fullscreen/info/Edit group. Cancel/Save are no
   longer on the title row (moved to the banner).

### B. `DashboardEditToolbar` → icon-only, title-row, combined `T▾`

Refactor the component to a compact icon row (tooltips, no text labels), in this order:
- `+` (`Plus`) → `onAddChart` (opens the add panel; see D).
- `T▾` (`Type` + caret) → **dropdown** with `Heading` and `Text` items → `onAddHeading` /
  `onAddText`.
- `🔗` (`Link2`) → `onAddLink`.
- `▦` (`LayoutGrid`) → **Section dropdown** (the 3 templates with improved previews, see C)
  → `onAddSection(layout)`.
- `⏚` (`Filter`) → `onAddFilter`.

Each icon button: `size-8`, ghost hover, `aria-label` + `title` for the tooltip. The
component no longer renders a full-width bordered strip; it returns an inline
`flex items-center gap-1` group meant to sit at the right of the title row.

### C. Section preview like Metabase

Rework `SectionLayoutPreview` so the mini-layout is legible:
- Container: a small bordered box (`border border-border bg-card`), e.g. `64×40px`.
- Each card rect: filled `bg-primary/70` (cards) / `bg-foreground/30` (heading bar), with a
  **1px gap** between rects so individual cards read as separate blocks (inset each rect by
  the gap; don't let them touch). Round corners `rounded-[1px]`.
- Scale columns by `boxWidth/24` and rows by a small per-row px, as today, but guarantee a
  visible gap (subtract ~1px from each rect's width/height).
- Result: each template visibly shows its arrangement (KPI grid = 2×2; large+KPIs-right =
  one big + 3 small; KPIs+chart-below = a row of 4 + one wide).

### D. On-demand sidebar (driven by `editPanel`)

- `startEdit` sets `setEditPanel(null)` (was `{ kind: "add" }`) → entering edit shows the
  **full-width canvas, no sidebar**.
- Render the sidebar **only when `editing && editPanel != null`**. Remove the
  `sidebarCollapsed` state, the collapsed "expand pill", and `onCollapse`. `openPanel`
  simplifies to `setEditPanel(p)`.
- Triggers that open the sidebar: `+` toolbar → `setEditPanel({ kind: "add" })`; clicking a
  card → `{ kind: "card", cardId }` (see G); add-filter → `{ kind: "filter", paramId }`;
  viz → `{ kind: "viz", cardId }`.
- `EditSidebar` header gets a **close (`X`)** that sets `editPanel = null` (hides the
  sidebar; canvas returns to full width). Sub-panels (card/filter/viz) keep a **back (`‹`)**
  that returns to `{ kind: "add" }`. The collapse button is removed.

### E. Tabs: per-tab `▾` menu

In `DashboardTabBar`, while editing, replace the hover `Copy`/`X` icons with a single
**`▾` (`ChevronDown`) button per tab** that opens a dropdown (`dropdown-menu`):
- `Rename` → enters the existing inline-rename input (`setRenaming(t.key)`).
- `Duplicate` → `onDuplicate(t.key)`.
- `Delete` → `onDelete(t.key)` (disabled / hidden when `< 2` tabs).

Keep drag-to-reorder and the active-tab underline. Double-click-to-rename may stay as a
shortcut. The `+` add-tab button is unchanged.

### F. Inline dashboard-name editing

- Add `draftName` state; `startEdit` sets `draftName = dashboard.name`; `cancelEdit` clears
  it.
- In edit mode the title renders an `<input>` styled like the heading (large, bold,
  transparent border until focus), `aria-label="dashboard name"`, value `draftName`. In view
  mode it stays an `<h1>`.
- `onSave` sends `name: draftName.trim() || dashboard.name` (never save an empty name).

### G. Click a card to select/configure; remove the gear

- In edit mode, the **inner card wrapper** gets an `onClick` that selects the card:
  `openPanel({ kind: "card", cardId: c.id })`. Gate it to non-placeholder cards
  (`c.card_type !== "placeholder"`) so placeholders keep their own "pick a chart" click.
  The existing `ring-2 ring-primary` selection highlight already reflects the selected card.
- The `onClick` goes on the **inner** wrapper (the `overflow-hidden` div), not the outer
  `group` div, so clicking the floating `CardEditToolbar` (a sibling above) does not also
  select. Clicking the drag-handle strip selecting too is acceptable.
- Remove the **`Settings` (gear)** button from `CardEditToolbar` — clicking the card now
  opens settings, so the gear is redundant (this is the user's specific annoyance). Keep
  viz (chart-only), duplicate, replace (chart-only), remove.
- View mode is unaffected (cards still drill/cross-filter on click; this `onClick` is
  edit-only).

## Testing

**Unit (Vitest):**
- Name round-trip: editing the title input updates the saved payload (`onSave` sends the
  edited `name`). Extend/adjust `DashboardView` tests.
- Toolbar: the `T▾` dropdown exposes `Heading` and `Text`; clicking each calls the right
  add handler. Section dropdown still lists the 3 templates.
- Tabs: the per-tab `▾` menu exposes Rename/Duplicate/Delete and calls the right callbacks;
  Delete disabled at `< 2` tabs.
- Card click: clicking a chart card in edit mode opens the card settings panel
  (`editPanel.kind === "card"`); a placeholder click still opens the chart picker.
- Sidebar: on `startEdit` no sidebar is shown (`editPanel === null`); `+` opens it; close
  hides it.
- Update any existing tests that assumed the labeled strip, the auto-open sidebar, Cancel/
  Save on the title row, or the gear button — adapt to the new UI without gutting
  assertions.

**Live (Playwright, throwaway dashboard, 0 console errors):** compare side-by-side with
Metabase — icon toolbar on the title row; Cancel/Save in the banner; `T▾` menu; Section
previews show real shapes; sidebar opens only on `+`; per-tab `▾` menu; inline name edit
persists after save; clicking a card opens its settings (no gear). Save round-trip intact.
Delete the throwaway dashboard + scratch screenshots afterward.

## Files

**Modified:**
- `frontend/src/routes/explore/DashboardView.tsx` — banner Cancel/Save; title-row icon
  toolbar placement; remove labeled strip; on-demand sidebar (`editPanel`-driven, drop
  `sidebarCollapsed`); inline `draftName`; click-to-select card.
- `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx` — icon-only layout,
  `T▾` dropdown, improved `SectionLayoutPreview`.
- `frontend/src/components/chart/dashboard/DashboardTabBar.tsx` — per-tab `▾` menu.
- `frontend/src/components/chart/dashboard/EditSidebar.tsx` — close-hides + back-to-add;
  remove collapse.
- `frontend/src/components/chart/dashboard/CardEditToolbar.tsx` — remove the gear (Settings).

**Backend:** none.

## Risks / Notes

- `editPanel`-driven sidebar visibility: ensure every existing path that set
  `sidebarCollapsed` is updated; the sidebar must reappear when any panel opens and hide on
  close. Verify card/filter/viz settings still open correctly.
- Click-to-select must not fire when interacting inside the floating toolbar or when
  finishing a drag; put the handler on the inner wrapper and keep placeholders on their own
  handler.
- OKLCH tokens only (no literal colors) in the new toolbar/preview/tab markup.
