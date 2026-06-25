# Dashboard Edit Mode — Metabase Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the dashboard edit page Metabase-faithful — a top add-toolbar, chart-only right sidebar, clean headings, an "Add section" feature replacing the width toggle, and a working bottom scroll.

**Architecture:** Frontend-only React/TS. A new `DashboardEditToolbar` consolidates all add controls under the edit banner; the right sidebar (`AddPanel`) becomes chart-list-only. "Add section" is a pure layout module (`dashboard-sections.ts`) that emits a heading + draft-only `placeholder` cards; a new `PlaceholderCard` renders the drop target, clicking it fills the slot with a chart, and unfilled placeholders are dropped before save (no backend change). Heading cards get the canvas background so the SVG grid no longer bleeds through.

**Tech Stack:** React 18, TypeScript, Vite, Vitest, Tailwind v4 (OKLCH tokens), shadcn (`dropdown-menu`, `dialog`), `react-grid-layout`, `lucide-react`.

**Conventions:**
- All commands run from `egret/frontend/`.
- Full sweep before every commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Git identity is already configured (`hoangquocvietbk2.13@gmail.com`) — never override the author.
- End every commit message with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Work on branch `feat/dashboard-edit-metabase-polish` (already created).
- Never touch the user's `a1` dashboard during live verification — use a throwaway dashboard and delete it after.

**Spec:** `docs/en/specs/2026-06-24-dashboard-edit-metabase-polish-design.md`

---

## File Structure

**New:**
- `frontend/src/components/chart/dashboard/dashboard-sections.ts` — section layout templates + `dropPlaceholders` helper (pure, tested).
- `frontend/src/components/chart/dashboard/PlaceholderCard.tsx` — edit-only empty drop target.
- `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx` — top add-toolbar.
- `frontend/src/components/chart/dashboard/__tests__/dashboard-sections.test.ts` — unit tests.

**Modified:**
- `frontend/src/components/chart/dashboard/AddPanel.tsx` — chart list only.
- `frontend/src/components/chart/dashboard/EditSidebar.tsx` — drop `onAddVirtual`.
- `frontend/src/routes/explore/DashboardView.tsx` — toolbar wiring, addSection, placeholder render + fill, drop-on-save, remove width toggle/`draftWidth`, heading bg, bottom-scroll fix.

---

## Task 1: Section layout module + `dropPlaceholders`

**Files:**
- Create: `frontend/src/components/chart/dashboard/dashboard-sections.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/dashboard-sections.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/dashboard-sections.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { layoutOptions, dropPlaceholders } from "../dashboard-sections";
import type { DraftCard } from "../tabs";

const byId = Object.fromEntries(layoutOptions.map((l) => [l.id, l]));

describe("dashboard-sections layouts", () => {
  it("kpi_grid: heading + 2x2 of 12x5 placeholders", () => {
    expect(byId.kpi_grid.build({ col: 0, row: 0 })).toEqual([
      { card_type: "heading", grid_col: 0, grid_row: 0, size_x: 24, size_y: 1 },
      { card_type: "placeholder", grid_col: 0, grid_row: 1, size_x: 12, size_y: 5 },
      { card_type: "placeholder", grid_col: 12, grid_row: 1, size_x: 12, size_y: 5 },
      { card_type: "placeholder", grid_col: 0, grid_row: 6, size_x: 12, size_y: 5 },
      { card_type: "placeholder", grid_col: 12, grid_row: 6, size_x: 12, size_y: 5 },
    ]);
  });

  it("large_chart_kpi_right: heading + 18x9 large + 3 right 6x3", () => {
    expect(byId.large_chart_kpi_right.build({ col: 0, row: 0 })).toEqual([
      { card_type: "heading", grid_col: 0, grid_row: 0, size_x: 24, size_y: 1 },
      { card_type: "placeholder", grid_col: 0, grid_row: 1, size_x: 18, size_y: 9 },
      { card_type: "placeholder", grid_col: 18, grid_row: 1, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 18, grid_row: 4, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 18, grid_row: 7, size_x: 6, size_y: 3 },
    ]);
  });

  it("kpi_chart_below: heading + 4 top 6x3 + 24x9 large", () => {
    expect(byId.kpi_chart_below.build({ col: 0, row: 0 })).toEqual([
      { card_type: "heading", grid_col: 0, grid_row: 0, size_x: 24, size_y: 1 },
      { card_type: "placeholder", grid_col: 0, grid_row: 1, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 6, grid_row: 1, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 12, grid_row: 1, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 18, grid_row: 1, size_x: 6, size_y: 3 },
      { card_type: "placeholder", grid_col: 0, grid_row: 4, size_x: 24, size_y: 9 },
    ]);
  });

  it("respects the origin offset (heading at row, body below)", () => {
    const cards = byId.kpi_grid.build({ col: 0, row: 7 });
    expect(cards[0]).toMatchObject({ card_type: "heading", grid_row: 7 });
    expect(cards[1]).toMatchObject({ grid_row: 8 });
    expect(cards[4]).toMatchObject({ grid_row: 13 });
  });

  it("dropPlaceholders removes only placeholder cards", () => {
    const cards = [
      { id: "a", card_type: "chart" },
      { id: "b", card_type: "placeholder" },
      { id: "c", card_type: "heading" },
    ] as unknown as DraftCard[];
    expect(dropPlaceholders(cards).map((c) => c.id)).toEqual(["a", "c"]);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npx vitest run src/components/chart/dashboard/__tests__/dashboard-sections.test.ts`
Expected: FAIL — cannot resolve `../dashboard-sections`.

- [ ] **Step 3: Write the module**

Create `frontend/src/components/chart/dashboard/dashboard-sections.ts`:

```ts
import type { DraftCard } from "./tabs";

// Clean-room port of Metabase's dashboard sections (frontend/src/metabase/dashboard/sections.ts),
// expressed in egret's 24-col grid model. Each section emits a full-width heading plus
// placeholder slots the user clicks to fill with a chart.
export type SectionId = "kpi_grid" | "large_chart_kpi_right" | "kpi_chart_below";

export interface SectionDraftCard {
  card_type: "heading" | "placeholder";
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
}

export interface SectionLayout {
  id: SectionId;
  label: string;
  build: (origin: { col: number; row: number }) => SectionDraftCard[];
}

const GRID_COLS = 24;
const HEADING_HEIGHT = 1;
const SCALAR_W = 6;
const SCALAR_H = 3;

function heading({ col, row }: { col: number; row: number }): SectionDraftCard {
  return { card_type: "heading", grid_col: col, grid_row: row, size_x: GRID_COLS, size_y: HEADING_HEIGHT };
}

function ph(grid_col: number, grid_row: number, size_x: number, size_y: number): SectionDraftCard {
  return { card_type: "placeholder", grid_col, grid_row, size_x, size_y };
}

// 2x2 grid of half-width KPI placeholders (Metabase getKpiGridLayout: scalarCardHeight = 5).
function buildKpiGrid({ col, row }: { col: number; row: number }): SectionDraftCard[] {
  const r = row + HEADING_HEIGHT;
  const w = GRID_COLS / 2; // 12
  const h = 5;
  return [
    heading({ col, row }),
    ph(col, r, w, h), ph(col + w, r, w, h),
    ph(col, r + h, w, h), ph(col + w, r + h, w, h),
  ];
}

// Large chart with a column of three KPI placeholders on the right (getLargeChartKpiColLayout).
function buildLargeChartKpiRight({ col, row }: { col: number; row: number }): SectionDraftCard[] {
  const r = row + HEADING_HEIGHT;
  const largeW = GRID_COLS - SCALAR_W; // 18
  const right = [
    ph(col + largeW, r, SCALAR_W, SCALAR_H),
    ph(col + largeW, r + SCALAR_H, SCALAR_W, SCALAR_H),
    ph(col + largeW, r + SCALAR_H * 2, SCALAR_W, SCALAR_H),
  ];
  const large = ph(col, r, largeW, SCALAR_H * right.length); // 18 x 9
  return [heading({ col, row }), large, ...right];
}

// Row of four KPI placeholders, then a full-width large chart below (getKpiLargeChartBelowLayout).
function buildKpiChartBelow({ col, row }: { col: number; row: number }): SectionDraftCard[] {
  const r = row + HEADING_HEIGHT;
  const topRow = [
    ph(col, r, SCALAR_W, SCALAR_H),
    ph(col + SCALAR_W, r, SCALAR_W, SCALAR_H),
    ph(col + SCALAR_W * 2, r, SCALAR_W, SCALAR_H),
    ph(col + SCALAR_W * 3, r, SCALAR_W, SCALAR_H),
  ];
  const large = ph(col, r + SCALAR_H, GRID_COLS, 9);
  return [heading({ col, row }), ...topRow, large];
}

export const layoutOptions: SectionLayout[] = [
  { id: "kpi_grid", label: "KPI grid", build: buildKpiGrid },
  { id: "large_chart_kpi_right", label: "Large chart with KPIs on the right", build: buildLargeChartKpiRight },
  { id: "kpi_chart_below", label: "KPIs with large chart below", build: buildKpiChartBelow },
];

/** Unfilled placeholder cards are edit-only and must be removed before persisting to the API. */
export function dropPlaceholders(cards: DraftCard[]): DraftCard[] {
  return cards.filter((c) => c.card_type !== "placeholder");
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npx vitest run src/components/chart/dashboard/__tests__/dashboard-sections.test.ts`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/components/chart/dashboard/dashboard-sections.ts src/components/chart/dashboard/__tests__/dashboard-sections.test.ts
git commit -m "feat(dashboard): add section layout templates + dropPlaceholders

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: PlaceholderCard component

**Files:**
- Create: `frontend/src/components/chart/dashboard/PlaceholderCard.tsx`

- [ ] **Step 1: Write the component**

Create `frontend/src/components/chart/dashboard/PlaceholderCard.tsx`:

```tsx
import { Plus } from "lucide-react";

/** Edit-mode-only empty drop target produced by "Add section". Click to fill it with a chart.
 *  `dash-nodrag` so a click fills (the wrapper's top strip remains the drag handle). */
export function PlaceholderCard({ onPick }: { onPick: () => void }) {
  return (
    <button
      type="button"
      aria-label="add a chart"
      onClick={onPick}
      className="dash-nodrag flex h-full w-full flex-col items-center justify-center gap-1 rounded-sm border-2 border-dashed border-border bg-card/40 text-muted-foreground hover:border-primary hover:text-foreground"
    >
      <Plus className="h-5 w-5" />
      <span className="text-xs">Add a chart</span>
    </button>
  );
}
```

- [ ] **Step 2: Verify it compiles**

Run: `npm run typecheck`
Expected: PASS (no type errors). The component is not yet imported anywhere; that's fine.

- [ ] **Step 3: Commit**

```bash
git add src/components/chart/dashboard/PlaceholderCard.tsx
git commit -m "feat(dashboard): add PlaceholderCard drop target

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: DashboardEditToolbar component

**Files:**
- Create: `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx`

- [ ] **Step 1: Write the component**

Create `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx`. It contains the toolbar and a small presentational `SectionLayoutPreview` subcomponent (a mini sketch derived from `build()`):

```tsx
import { BarChart3, Type, Heading, Link2, LayoutGrid, Filter } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
import { layoutOptions, type SectionLayout } from "./dashboard-sections";

interface Props {
  onAddChart: () => void;
  onAddText: () => void;
  onAddHeading: () => void;
  onAddLink: () => void;
  onAddSection: (layout: SectionLayout) => void;
  onAddFilter: () => void;
}

const ITEM = "inline-flex items-center gap-1.5 rounded px-2 py-1 text-sm text-foreground hover:bg-accent";

// Mini layout sketch: scale the 24-col grid to a small preview box.
const PREVIEW_COLS = 24;
const PREVIEW_WIDTH = 56;   // px
const PREVIEW_ROW_PX = 3;   // px per grid row
const COL_PX = PREVIEW_WIDTH / PREVIEW_COLS;

function SectionLayoutPreview({ layout }: { layout: SectionLayout }) {
  const cards = layout.build({ col: 0, row: 0 });
  const rows = Math.max(...cards.map((c) => c.grid_row + c.size_y));
  return (
    <div className="relative shrink-0 rounded-sm bg-muted" style={{ width: PREVIEW_WIDTH, height: rows * PREVIEW_ROW_PX }}>
      {cards.map((c, i) => (
        <div
          key={i}
          className={`absolute rounded-[1px] ${c.card_type === "heading" ? "bg-foreground/40" : "bg-primary/50"}`}
          style={{
            left: c.grid_col * COL_PX,
            top: c.grid_row * PREVIEW_ROW_PX,
            width: c.size_x * COL_PX,
            height: c.size_y * PREVIEW_ROW_PX,
          }}
        />
      ))}
    </div>
  );
}

/** Edit-mode add toolbar: all "add" controls in one strip under the edit banner. */
export function DashboardEditToolbar({ onAddChart, onAddText, onAddHeading, onAddLink, onAddSection, onAddFilter }: Props) {
  return (
    <div className="-mx-4 mb-4 flex flex-wrap items-center gap-1 border-b bg-card px-4 py-1.5">
      <button type="button" className={ITEM} onClick={onAddChart}>
        <BarChart3 className="h-4 w-4 text-primary" /> Chart
      </button>
      <button type="button" className={ITEM} onClick={onAddText}>
        <Type className="h-4 w-4" /> Text
      </button>
      <button type="button" className={ITEM} onClick={onAddHeading}>
        <Heading className="h-4 w-4" /> Heading
      </button>
      <button type="button" className={ITEM} onClick={onAddLink}>
        <Link2 className="h-4 w-4" /> Link
      </button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className={ITEM} aria-label="add section">
            <LayoutGrid className="h-4 w-4" /> Section
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          {layoutOptions.map((layout) => (
            <DropdownMenuItem key={layout.id} className="gap-2" onClick={() => onAddSection(layout)}>
              <SectionLayoutPreview layout={layout} />
              <span>{layout.label}</span>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      <button type="button" className={ITEM} onClick={onAddFilter}>
        <Filter className="h-4 w-4" /> Filter
      </button>
    </div>
  );
}
```

- [ ] **Step 2: Verify it compiles**

Run: `npm run typecheck`
Expected: PASS. (Not yet wired into `DashboardView`; that's Task 5.)

- [ ] **Step 3: Commit**

```bash
git add src/components/chart/dashboard/DashboardEditToolbar.tsx
git commit -m "feat(dashboard): add edit-mode add-toolbar with section previews

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Right sidebar = chart list only

**Files:**
- Modify: `frontend/src/components/chart/dashboard/AddPanel.tsx`
- Modify: `frontend/src/components/chart/dashboard/EditSidebar.tsx`

- [ ] **Step 1: Strip the virtual-card row from AddPanel**

Replace the entire contents of `frontend/src/components/chart/dashboard/AddPanel.tsx` with:

```tsx
import type { Chart } from "@/api/charts";
import { ChartPickerList } from "./ChartPickerList";

interface Props { charts: Chart[]; onAddChart: (c: Chart) => void }

export function AddPanel({ charts, onAddChart }: Props) {
  return (
    <div className="flex h-full flex-col gap-2 p-3">
      <div className="text-sm font-medium">Add a chart</div>
      <ChartPickerList charts={charts} onPick={onAddChart} />
    </div>
  );
}
```

- [ ] **Step 2: Drop `onAddVirtual` from EditSidebar**

In `frontend/src/components/chart/dashboard/EditSidebar.tsx`, remove the `onAddVirtual` prop from the `Props` interface. Delete this line:

```tsx
  onAddVirtual: (t: "text" | "heading" | "link") => void;
```

Then update the `AddPanel` usage (currently passes `onAddVirtual`):

```tsx
      {panel?.kind === "add" && <AddPanel charts={props.charts} onAddChart={props.onAddChart} onAddVirtual={props.onAddVirtual} />}
```

to:

```tsx
      {panel?.kind === "add" && <AddPanel charts={props.charts} onAddChart={props.onAddChart} />}
```

- [ ] **Step 3: Verify (expected to fail at the call site)**

Run: `npm run typecheck`
Expected: FAIL — `DashboardView.tsx` still passes `onAddVirtual` to `<EditSidebar>` (a now-unknown prop). This is fixed in Task 5. Do NOT fix it here.

- [ ] **Step 4: Commit**

```bash
git add src/components/chart/dashboard/AddPanel.tsx src/components/chart/dashboard/EditSidebar.tsx
git commit -m "refactor(dashboard): make Add-cards sidebar chart-list only

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Wire the toolbar; remove header add-buttons + width toggle

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

This task wires `DashboardEditToolbar`, removes the header `+ Chart` / `+ Filter` buttons and the Full/Fixed toggle, deletes the `draftWidth` state, and extracts the add-filter handler. (The `onAddSection` handler is added in Task 6; for now wire it to a stub that is replaced in Task 6 — see Step 6.)

- [ ] **Step 1: Add the import**

In `frontend/src/routes/explore/DashboardView.tsx`, add to the imports near the other dashboard-component imports (e.g. after the `CardEditToolbar` import on line 25):

```tsx
import { DashboardEditToolbar } from "@/components/chart/dashboard/DashboardEditToolbar";
import { layoutOptions, type SectionLayout } from "@/components/chart/dashboard/dashboard-sections";
```

(`layoutOptions` import is unused until Task 6 wires `addSection`; if `npm run lint` flags it as unused at the end of this task, leave the `SectionLayout` type import and remove `layoutOptions` from this line — it is re-added in Task 6. To avoid churn, prefer completing Task 6 before the final lint.)

- [ ] **Step 2: Delete the `draftWidth` state**

Remove this line (line 58):

```tsx
  const [draftWidth, setDraftWidth] = useState<"fixed" | "full">("full");
```

- [ ] **Step 3: Fix `effectiveWidth`**

Change (line 96):

```tsx
  const effectiveWidth = editing ? draftWidth : dashboard.width;
```

to:

```tsx
  const effectiveWidth = dashboard.width;
```

- [ ] **Step 4: Remove `setDraftWidth` from `startEdit`**

In `startEdit` (around line 130), delete this line:

```tsx
    setDraftWidth(dashboard.width);
```

- [ ] **Step 5: Use the stored width in `onSave`**

In `onSave` (around line 234), change `width: draftWidth` to `width: dashboard.width`:

```tsx
      await update.mutateAsync({ id: dashboard.id, name: dashboard.name, description: dashboard.description, parameters, tabs: tabPayload, width: dashboard.width, cards: cardPayload });
```

- [ ] **Step 6: Add `addFilter` and `addSection` handlers**

Add these two functions next to the other card handlers (e.g. after `addVirtualCard`, around line 176). The `addSection` body is finalized in Task 6, but define it now so the toolbar wiring type-checks:

```tsx
  function addFilter() {
    const slug = slugify("filter", parameters.map((p) => p.slug));
    const id = `new-${slug}-${cardSeqRef.current++}`;
    setDraftParameters([...parameters, { id, name: "Filter", slug, type: "text", operator: "is" }]);
    openPanel({ kind: "filter", paramId: id });
  }

  function addSection(layout: SectionLayout) {
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      const onTab = base.filter((c) => c.tabKey === (activeTab?.key ?? null));
      const row = onTab.reduce((m, c) => Math.max(m, c.grid_row + c.size_y), 0);
      const built: DraftCard[] = layout.build({ col: 0, row }).map((s) => ({
        id: `new-section-${cardSeqRef.current++}`,
        chart_id: null,
        card_type: s.card_type,
        grid_col: s.grid_col,
        grid_row: s.grid_row,
        size_x: s.size_x,
        size_y: s.size_y,
        options: s.card_type === "heading" ? { text: "" } : {},
        tab_id: null,
        tabKey: activeTab?.key ?? null,
      }));
      return [...base, ...built];
    });
  }
```

- [ ] **Step 7: Render the toolbar under the edit banner**

The edit banner is (lines 278-282):

```tsx
      {editing && (
        <div className="-mx-4 -mt-4 mb-4 flex items-center gap-2 bg-primary/90 px-4 py-2 text-sm font-medium text-primary-foreground">
          <Pencil className="h-4 w-4" /> You're editing this dashboard
        </div>
      )}
```

Immediately after that block (before `<div className={effectiveWidth === "fixed" ? ...`), add:

```tsx
      {editing && (
        <DashboardEditToolbar
          onAddChart={() => openPanel({ kind: "add" })}
          onAddText={() => addVirtualCard("text", { text: "" })}
          onAddHeading={() => addVirtualCard("heading", { text: "" })}
          onAddLink={() => addVirtualCard("link", { url: "", label: "" })}
          onAddSection={addSection}
          onAddFilter={addFilter}
        />
      )}
```

- [ ] **Step 8: Strip the header add-buttons + width toggle**

Replace the editing branch of the header (lines 326-341):

```tsx
          {editing ? (
            <>
              <Button size="sm" variant="outline" onClick={() => openPanel({ kind: "add" })}>+ Chart</Button>
              <Button size="sm" variant="outline" onClick={() => {
                const slug = slugify("filter", parameters.map((p) => p.slug));
                const id = `new-${slug}-${cardSeqRef.current++}`;
                setDraftParameters([...parameters, { id, name: "Filter", slug, type: "text", operator: "is" }]);
                openPanel({ kind: "filter", paramId: id });
              }}>+ Filter</Button>
              <div className="flex items-center gap-1 rounded border p-0.5">
                <button type="button" aria-label="width full" className={`rounded px-2 py-0.5 text-xs ${draftWidth === "full" ? "bg-accent" : ""}`} onClick={() => setDraftWidth("full")}>Full</button>
                <button type="button" aria-label="width fixed" className={`rounded px-2 py-0.5 text-xs ${draftWidth === "fixed" ? "bg-accent" : ""}`} onClick={() => setDraftWidth("fixed")}>Fixed</button>
              </div>
              <Button size="sm" variant="ghost" onClick={cancelEdit}>Cancel</Button>
              <Button size="sm" onClick={onSave} disabled={update.isPending}>Save</Button>
            </>
          ) : (
            <Button size="sm" variant="outline" onClick={startEdit}>Edit</Button>
          )}
```

with:

```tsx
          {editing ? (
            <>
              <Button size="sm" variant="ghost" onClick={cancelEdit}>Cancel</Button>
              <Button size="sm" onClick={onSave} disabled={update.isPending}>Save</Button>
            </>
          ) : (
            <Button size="sm" variant="outline" onClick={startEdit}>Edit</Button>
          )}
```

- [ ] **Step 9: Remove the `onAddVirtual` prop from the EditSidebar usage**

In the `<EditSidebar>` JSX (around line 471), delete this line:

```tsx
            onAddVirtual={(t) => addVirtualCard(t, t === "link" ? { url: "", label: "" } : { text: "" })}
```

- [ ] **Step 10: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS. The toolbar appears under the banner; the header shows only Cancel/Save; the width toggle is gone.

- [ ] **Step 11: Run the unit suite**

Run: `npx vitest run`
Expected: PASS (existing tests + Task 1's tests).

- [ ] **Step 12: Commit**

```bash
git add src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): move add controls to edit toolbar, drop width toggle

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Render + fill placeholders; heading background; drop-on-save

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

`addSection` and the toolbar are already wired (Task 5). This task makes placeholders render, fill, and not persist, and fixes the heading grid bleed-through.

- [ ] **Step 1: Add imports**

Add to the imports in `frontend/src/routes/explore/DashboardView.tsx`:

```tsx
import { PlaceholderCard } from "@/components/chart/dashboard/PlaceholderCard";
import { dropPlaceholders } from "@/components/chart/dashboard/dashboard-sections";
```

(If `layoutOptions` was removed in Task 5 Step 1 to satisfy lint, keep only the `SectionLayout` type + `dropPlaceholders` here; `addSection` uses the `SectionLayout` type and `layout.build`, not `layoutOptions`.)

- [ ] **Step 2: Add the fill state + handler**

Next to the `replacingId` state (around line 69, `const [replacingId, setReplacingId] = useState<string | null>(null);`), add:

```tsx
  const [fillingPlaceholderId, setFillingPlaceholderId] = useState<string | null>(null);
```

Next to `replaceCard` (around line 159), add:

```tsx
  function fillPlaceholder(cardId: string, chart: Chart) {
    setDraftCards((prev) =>
      (prev ?? cardsAsDraft()).map((c) =>
        c.id === cardId ? { ...c, card_type: "chart", chart_id: chart.id, options: {} } : c,
      ),
    );
    setFillingPlaceholderId(null);
  }
```

(The chart adopts the placeholder's `grid_col/row/size_x/size_y` via the `...c` spread — the slot's footprint is preserved.)

- [ ] **Step 3: Drop placeholders before serializing in `onSave`**

In `onSave` (around line 232), change:

```tsx
    const { tabs: tabPayload, cards: cardPayload } = serializeLayout(draftTabs ?? [], (cards as DraftCard[]));
```

to:

```tsx
    const persistCards = dropPlaceholders(cards as DraftCard[]);
    const { tabs: tabPayload, cards: cardPayload } = serializeLayout(draftTabs ?? [], persistCards);
```

- [ ] **Step 4: Heading background + placeholder render in the card wrapper**

The inner card wrapper is (line 426):

```tsx
              <div className={`relative h-full overflow-hidden ${c.card_type === "heading" ? "" : "rounded-sm border bg-card"} ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
```

Replace it with (heading gets `bg-dashboard` to mask the grid; placeholder gets no card chrome):

```tsx
              <div className={`relative h-full overflow-hidden ${
                c.card_type === "heading" ? "bg-dashboard"
                : c.card_type === "placeholder" ? ""
                : "rounded-sm border bg-card"
              } ${editing && editPanel?.kind === "card" && editPanel.cardId === c.id ? "ring-2 ring-primary" : ""}`}>
```

Then, inside that wrapper, the `<DashboardCardView ... />` block (lines 428-433) must be guarded so placeholders render `PlaceholderCard` instead. The current block is:

```tsx
                <DashboardCardView ws={ws} card={c} chart={injected} refreshNonce={view.refreshNonce} parameters={parameters} paramValues={paramsState.values}
                  blocked={!editing && requiredUnset} editing={editing}
                  autoHeight={!!c.options.autoHeight}
                  rowHeight={rowHeight}
                  onAutoHeight={editing ? (rows) => applyAutoHeight(c.id, rows) : undefined}
                  onValueClick={editing ? undefined : (ctx, x, y) => onCardValueClick(c, ctx, x, y)} />
```

Replace it with:

```tsx
                {c.card_type === "placeholder" ? (
                  <PlaceholderCard onPick={() => setFillingPlaceholderId(c.id)} />
                ) : (
                  <DashboardCardView ws={ws} card={c} chart={injected} refreshNonce={view.refreshNonce} parameters={parameters} paramValues={paramsState.values}
                    blocked={!editing && requiredUnset} editing={editing}
                    autoHeight={!!c.options.autoHeight}
                    rowHeight={rowHeight}
                    onAutoHeight={editing ? (rows) => applyAutoHeight(c.id, rows) : undefined}
                    onValueClick={editing ? undefined : (ctx, x, y) => onCardValueClick(c, ctx, x, y)} />
                )}
```

- [ ] **Step 5: Add the "Add a chart" fill dialog**

After the Replace-chart `<Dialog>` (ends around line 531), add a parallel dialog:

```tsx
      <Dialog open={fillingPlaceholderId !== null} onOpenChange={(o) => { if (!o) setFillingPlaceholderId(null); }}>
        <DialogContent>
          <DialogHeader><DialogTitle>Add a chart</DialogTitle></DialogHeader>
          <ChartPickerList charts={charts ?? []} onPick={(ch) => fillPlaceholder(fillingPlaceholderId!, ch)} autoFocus />
        </DialogContent>
      </Dialog>
```

- [ ] **Step 6: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS.

- [ ] **Step 7: Run the unit suite**

Run: `npx vitest run`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): render+fill section placeholders, clean heading bg, drop placeholders on save

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Fix bottom-of-page scroll in edit mode

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

This is a "look at it" bug — **reproduce it live first**, then apply the fix and confirm the repro is resolved.

- [ ] **Step 1: Reproduce the bug**

Ensure the dev stack is up (frontend `http://localhost:5173`, backend `:8080`). Using Playwright MCP:
1. Log in (`admin@egret.local` / `Admin12345!`).
2. Open a dashboard with enough cards to overflow the viewport (or create a throwaway dashboard and use the new "Add section" twice to make it tall). **Never use the `a1` dashboard.**
3. Click **Edit**.
4. Scroll the main content area to the bottom. Confirm the symptom: the last row of cards (and its bottom resize handle) cannot be fully scrolled into view.

Record what you observe (a screenshot to the parent `/Users/1pro-l4r8/Documents/github/data-portal/` dir, not the repo).

- [ ] **Step 2: Apply the likely fix (bottom breathing room on the editing canvas column)**

The grid column wrapper is (line 389):

```tsx
        <div className={editing || showInfo ? "min-w-0 flex-1" : ""}>
```

Change it to add bottom padding while editing so the last row + resize handle clear the viewport bottom:

```tsx
        <div className={editing ? "min-w-0 flex-1 pb-24" : showInfo ? "min-w-0 flex-1" : ""}>
```

- [ ] **Step 3: Re-verify against the repro**

Reload the same throwaway dashboard in edit mode (Playwright) and scroll to the bottom. Expected: the last row of cards and its bottom-right resize handle are now fully visible/grabbable. Confirm 0 console errors (`browser_console_messages`).

If the symptom persists, the root cause is elsewhere (e.g. an `overflow: hidden` ancestor or the sticky sidebar capping the flex row height). In that case, inspect the computed layout, fix the actual clipping container, and keep the requirement: **the last row must be scrollable into view in edit mode, with no change to view-mode spacing.** Document the final change in the commit message.

- [ ] **Step 4: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/routes/explore/DashboardView.tsx
git commit -m "fix(dashboard): make edit-mode bottom rows fully scrollable

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Full sweep + live verification + cleanup

**Files:** none (verification only).

- [ ] **Step 1: Full sweep**

Run from `egret/frontend/`:

```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
```

Expected: all PASS, no lint errors (in particular, no unused-import warnings for `layoutOptions`/`SectionLayout`/`dropPlaceholders`).

- [ ] **Step 2: Live verification (Playwright, throwaway dashboard, 0 console errors)**

Log in (`admin@egret.local` / `Admin12345!`). On a **throwaway** dashboard (create one; never `a1`), verify:

1. **Toolbar:** under the "You're editing" banner, the strip shows **Chart, Text, Heading, Link, Section, Filter**. The header row shows only **Cancel / Save** (no `+ Chart`, `+ Filter`, or Full/Fixed). The right "Add cards" sidebar shows only the chart search + list.
2. **Heading:** add a Heading via the toolbar → no grid squares render behind it (it sits cleanly on the canvas).
3. **Add section:** open the **Section** dropdown → 3 entries with mini previews. Insert each template → a heading + dashed placeholder slots appear at the expected layout. Click a placeholder → the "Add a chart" dialog opens → pick a chart → it fills the slot at the slot's size.
4. **Save round-trip:** leave at least one placeholder unfilled, then **Save** → reload the page → unfilled placeholders are gone; filled charts and the heading persist with their positions.
5. **Bottom scroll:** with a tall dashboard in edit mode, scroll to the very bottom → the last row of cards and its resize handle are fully reachable.
6. **Console:** `browser_console_messages` shows 0 errors throughout.

Capture verification screenshots to `/Users/1pro-l4r8/Documents/github/data-portal/` (parent dir, NOT the repo).

- [ ] **Step 3: Cleanup**

Delete the throwaway dashboard (and any throwaway charts created for it). Delete the scratch screenshots from the parent dir. Confirm `git status` in the repo shows only the intended source/spec/plan changes (no stray scratch files).

- [ ] **Step 4: Final whole-branch review**

Per subagent-driven-development, dispatch the final code reviewer over the whole branch diff (`git diff main...HEAD`), then proceed to `superpowers:finishing-a-development-branch`.

---

## Self-Review notes

- **Spec coverage:** Part 1 → Tasks 3,4,5. Part 2 (heading squares) → Task 6 Step 4. Part 3 (Add section: templates → Task 1; placeholder card → Task 2; render/fill/insert → Tasks 5,6; drop-on-save → Tasks 1,6; remove width toggle → Task 5) ✓. Part 4 (bottom scroll) → Task 7. Tests → Task 1 + Task 8 live verify. ✓
- **Type consistency:** `SectionDraftCard` (Task 1) → consumed by `addSection`/`SectionLayoutPreview` (Tasks 3,5). `dropPlaceholders(DraftCard[])` (Task 1) → used in `onSave` (Task 6). `fillPlaceholder(cardId, chart)` and `fillingPlaceholderId` consistent across Task 6. `card_type` is `string`, so `"placeholder"` needs no type change. ✓
- **Import ordering caveat:** Task 5 Step 1 notes the `layoutOptions` import may be unused until Task 6; the final lint in Task 8 is the gate. Complete Task 6 before relying on a clean lint.
