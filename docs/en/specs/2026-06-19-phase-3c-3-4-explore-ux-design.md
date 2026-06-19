# Phase 3c-3.4 — Chart Explore UX (type gallery + Data/Display tabs)

> Status: approved design. Canonical (English). Vietnamese mirror:
> `docs/vi/specs/2026-06-19-phase-3c-3-4-explore-ux-design.md`.
> Frontend-only UX redesign of the chart Explore page (Phases 3c-3 … 3c-3.3, merged `30c8bbf`).
> Branch `feat/phase-3c-3-4-explore-ux`.

## Goal

Make the chart Explore UX match Metabase/Superset (the user's directive: copy Metabase
closely — [[metabase-fidelity-preference]]):

1. A **Superset-style chart-type gallery** when creating a new chart.
2. The Explore left panel reorganized into **two Metabase-style tabs — Data and Display** —
   with the per-column **Format** and **Conditional Formatting** sections folded into the
   Display tab (the standalone right sidebar is removed, preview goes full-width).
3. Metabase visual polish (underlined tab switcher, section headers, consistent spacing).

This is **pure frontend restructuring** — no backend, no engine, no data-model change. It
reuses the existing `FormatSection`, `ConditionalFormattingSection`, renderers, and the
persist/hydrate logic verbatim.

(Renumbering note: this UX work is **3c-3.4**; the previously-planned table interactions move
to **3c-3.5**, pivot collapse/sticky/sort to **3c-3.6**, drill-through to **3c-3.7**.)

## Scope

**In scope:**
- A chart-type **registry** (`chartTypes.ts`): `{ key, name, description, icon }` for the
  supported viz types (today: Table, Pivot Table — extensible to bar/line/pie later).
- A reusable **`ChartTypeGallery`** component (full-page on new-chart; as a dialog for
  "Change visualization").
- Routing: `/charts/new` shows the gallery (no `?type`) → picking a type → the editor
  (`/charts/new?type=<key>`); `/charts/:id` opens the editor directly.
- `ChartExplore` restructured: a viz header with **"Change visualization"**, a **Data** tab
  and a **Display** tab; the right `SettingsSidebar` removed (Format + Conditional move into
  Display); the preview spans full width.
- Metabase styling (shadcn `Tabs`, uppercase section headers, spacing).

**Out of scope:** any backend/engine change; new viz types (still Table + Pivot); search /
category sidebar / details pane in the gallery (only 2 types — a simple card grid; revisit
when there are many types); table interactions (3c-3.5), pivot collapse (3c-3.6),
drill-through (3c-3.7). The `SettingsSidebar.tsx` shell becomes unused and is deleted.

## 1. Chart-type registry — `frontend/src/components/chart/chartTypes.ts`

```ts
import { Table2, LayoutGrid, type LucideIcon } from "lucide-react";
import type { VizType } from "@/api/charts";

export interface ChartTypeMeta {
  key: VizType;
  name: string;
  description: string;
  icon: LucideIcon;
}

export const CHART_TYPES: ChartTypeMeta[] = [
  { key: "table", name: "Table", description: "Raw rows or an aggregated table with sorting and formatting.", icon: Table2 },
  { key: "pivot_table", name: "Pivot Table", description: "Cross-tab rows × columns × measures with subtotals and grand totals.", icon: LayoutGrid },
];
```

(Pick concrete lucide icons that exist in the installed `lucide-react`; `Table2` and
`LayoutGrid` are placeholders — the implementer confirms availability.)

## 2. `ChartTypeGallery` — `frontend/src/components/chart/ChartTypeGallery.tsx`

A reusable card grid. Used full-page (new) and inside a dialog (change).

```tsx
export function ChartTypeGallery({
  selected, onSelect,
}: { selected?: VizType; onSelect: (key: VizType) => void }) { … }
```

- A responsive grid (`grid grid-cols-2 sm:grid-cols-3 gap-3`) of cards. Each card: the icon,
  the **name** (bold), a one-line **description**; selected card has a bold primary ring.
- Click → `onSelect(key)`. Keyboard-accessible (`role="button"`, Enter/Space).
- Superset's search / category sidebar / examples pane are intentionally omitted (2 types).

## 3. Routing + new-chart flow

In `App.tsx` the routes already are `charts/new` → `ChartExplore` and `charts/:id` →
`ChartExplore`. Keep both pointing at `ChartExplore`; the gallery-vs-editor decision is made
inside it:

- `ChartExplore` reads `id = useParams().id` and `type = useSearchParams().get("type")`.
- **If `!id && !type`** → render `<ChartTypeGallery onSelect={(k) => setSearchParams({ type: k })} />`
  (full-page). Picking a type sets `?type=<key>` and the editor renders.
- **If `type` (new) or `id` (edit)** → render the editor. For a new chart the initial `viz`
  comes from `?type`; for an existing chart it comes from hydration (unchanged).

## 4. Editor restructure — `ChartExplore.tsx`

The editor keeps all current state and logic (datasetId, pRows/pCols/pVals, tableMetrics/
tableCols, rowLimit, columnSettings, conditionalFormatting, run/poll/save/hydrate). Only the
**layout** changes.

### 4.1 Viz header (top of the left panel)
The current Table/Pivot toggle is replaced by a small header showing the current viz type's
name + a **"Change visualization"** button that opens `ChartTypeGallery` in a `Dialog`.
Selecting a type sets `viz` and closes the dialog (config is preserved; switching viz already
clears the stale query via the existing `changeViz`). On the **Display** preview, `viz`
drives Table vs Pivot exactly as today.

### 4.2 Two tabs — `Data` and `Display` (shadcn `Tabs`)
A `<Tabs defaultValue="data">` with an underlined `TabsList` (Metabase look):

- **Data tab** (`TabsContent value="data"`): Dataset picker; then the viz-specific data
  controls that exist today — **Pivot**: the `OrderedPicker` Rows / Columns / Values; **Table**:
  the Metrics checkbox list + Columns checkbox list. Then **Row limit**.
- **Display tab** (`TabsContent value="display"`): for **Pivot**, the Row-totals / Column-totals
  / Condense toggles; then — for **both** viz types — the existing **`FormatSection`** (per
  the result columns, filtered of `__grp_*`) and **`ConditionalFormattingSection`** (with
  `isPivot`). These are the same components used by the now-removed `SettingsSidebar`, moved
  here unchanged. The Display tab shows a hint ("Run the chart to configure columns") when
  there are no result columns yet.

### 4.3 Remove the right sidebar
Delete the `settingsOpen` state, the Settings toggle button, the right-hand `SettingsSidebar`
render, and the flex-row wrapper. The preview area spans full width. Delete
`SettingsSidebar.tsx` (superseded — Format/Conditional now live in the Display tab).

## 5. Styling (Metabase)
- Tab switcher: underlined active state (shadcn `Tabs` styled, or a small custom underline).
- Section headers within tabs: uppercase, small, semibold, muted (matches the existing
  `text-xs font-medium uppercase tracking-wide text-muted-foreground` already used).
- Consistent spacing (`space-y-3` sections, `space-y-1` fields).

## 6. Persistence / hydration
Unchanged. `paramsForSave()` already writes everything (spec + pivot layout + columnSettings +
conditionalFormatting); hydration restores it. The Data/Display split is purely presentational;
no param shape changes. A new chart created via the gallery saves the same `viz_type` +
`params`.

## 7. Error handling
- Gallery: picking a type always yields a valid `viz`; no failure path.
- Editor with `?type` of an unknown value → fall back to the gallery (treat as no type).
- Display tab with no result columns → the hint (no crash), same as the sidebar's empty state.

## 8. Testing
- `ChartTypeGallery` test: renders a card per `CHART_TYPES`, calls `onSelect(key)` on click,
  marks the selected card.
- `ChartExplore` routing/tab tests are light (no existing ChartExplore unit test); rely on
  typecheck + the manual smoke. Existing renderer/section/engine tests are untouched and must
  stay green.
- **Manual Playwright smoke:** New chart → gallery → pick Pivot → editor; Data tab (rows/cols/
  values) + Display tab (totals + Format currency + a Conditional rule) → Run → preview reflects
  both; "Change visualization" → Table → re-run; Save → reopen → everything hydrates.

## 9. File structure
- Create `frontend/src/components/chart/chartTypes.ts`.
- Create `frontend/src/components/chart/ChartTypeGallery.tsx` (+ test).
- Modify `frontend/src/routes/explore/ChartExplore.tsx` (gallery branch + 2-tab layout + viz
  header + remove sidebar).
- Delete `frontend/src/components/chart/settings/SettingsSidebar.tsx` (+ no test existed).
- Reused unchanged: `FormatSection`, `ConditionalFormattingSection`, `RuleEditor`, `OrderedPicker`
  (currently inside ChartExplore — stays), `ChartTable`, `pivot/PivotTable`, `format.ts`,
  `conditional.ts`, `charts.ts`.
- `App.tsx` routes unchanged (both `charts/new` + `charts/:id` already → `ChartExplore`).
