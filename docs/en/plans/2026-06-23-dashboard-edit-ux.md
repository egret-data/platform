# Dashboard edit UX polish — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Polish dashboard edit mode — legible card/tab icons, a searchable "change chart" picker, a collapsible add-chart sidebar, a Metabase-like resize (visible grid + finer steps + edge handles), and auto-height for content cards.

**Architecture:** Frontend-only. Stored card geometry (`grid_col/grid_row/size_x/size_y`) is unchanged on disk; finer vertical granularity comes from a render-time ×3 scale (rowHeight 90→30, units ×3). Auto-height measures content via ResizeObserver and writes `size_y` in the edit draft. No backend, no migration.

**Tech Stack:** React 18 + TS + Vite, Vitest + @testing-library/react + user-event, react-grid-layout, lucide-react, Tailwind.

**Spec:** `docs/en/specs/2026-06-23-dashboard-edit-ux-design.md`. Branch `feat/dashboard-edit-ux` (off main, spec committed).

**Conventions (every task):** TDD (failing test first). Full sweep before each commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage explicit paths. Configured git identity. End every commit message with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Do NOT push until Task 6.

## File map

- Modify `src/components/chart/dashboard/CardEditToolbar.tsx` — lucide icons (Task 1)
- Modify `src/components/chart/dashboard/DashboardTabBar.tsx` — lucide icons, hover-reveal (Task 1)
- Create `src/components/chart/dashboard/viz-icons.ts` — shared `ICON_BY_VIZ` (Task 2)
- Create `src/components/chart/dashboard/ChartPickerList.tsx` — searchable icon list (Task 2)
- Modify `src/components/chart/dashboard/AddPanel.tsx` — use ChartPickerList + shared icons (Task 2)
- Modify `src/routes/explore/DashboardView.tsx` — Replace dialog, sidebar collapse, render-scale, overlay, auto-height wiring (Tasks 2-5)
- Modify `src/components/chart/dashboard/EditSidebar.tsx` — collapse button (Task 3)
- Modify `src/components/chart/dashboard-grid.ts` — ROW_PX/ROW_SCALE + pure helpers (Task 4)
- Create `src/components/chart/dashboard/GridOverlay.tsx` — edit-mode grid lines (Task 4)
- Create `src/components/chart/dashboard/AutoHeightMeasure.tsx` — content measurer (Task 5)
- Modify `src/components/chart/dashboard/DashboardCardView.tsx` — auto-height measuring (Task 5)
- Modify `src/components/chart/dashboard/CardSettingsPanel.tsx` — auto-height toggle (Task 5)

---

### Task 1: Legible card-toolbar + tab icons (① + ④)

**Files:**
- Modify: `frontend/src/components/chart/dashboard/CardEditToolbar.tsx`
- Modify: `frontend/src/components/chart/dashboard/DashboardTabBar.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/CardEditToolbar.test.tsx` (create)
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx` (create)

- [ ] **Step 1: Write the failing tests.**

`CardEditToolbar.test.tsx`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { CardEditToolbar } from "../CardEditToolbar";

const noop = () => {};

describe("CardEditToolbar", () => {
  it("renders all five controls for a chart card", () => {
    render(<CardEditToolbar cardType="chart" onSettings={noop} onViz={noop} onDuplicate={noop} onReplace={noop} onRemove={noop} />);
    for (const label of ["card settings", "card visualization", "duplicate card", "replace card", "remove card"]) {
      expect(screen.getByRole("button", { name: label })).toBeTruthy();
    }
  });
  it("hides viz/replace for non-chart cards", () => {
    render(<CardEditToolbar cardType="text" onSettings={noop} onViz={noop} onDuplicate={noop} onReplace={noop} onRemove={noop} />);
    expect(screen.queryByRole("button", { name: "card visualization" })).toBeNull();
    expect(screen.queryByRole("button", { name: "replace card" })).toBeNull();
    expect(screen.getByRole("button", { name: "remove card" })).toBeTruthy();
  });
});
```

`DashboardTabBar.test.tsx`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { DashboardTabBar } from "../DashboardTabBar";

const tab = (key: string, name: string, position: number) => ({ key, name, position }) as never;
const noop = () => {};
const baseProps = { activeTabPos: 0, editing: true, onSelect: noop, onAdd: noop, onRename: noop, onReorder: noop, onDelete: noop, onDuplicate: noop };

describe("DashboardTabBar (edit mode)", () => {
  it("shows duplicate + delete per tab and an add-tab button", () => {
    render(<DashboardTabBar tabs={[tab("a", "Overview", 0), tab("b", "Detail", 1)]} {...baseProps} />);
    expect(screen.getByRole("button", { name: "duplicate tab Overview" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "delete tab Detail" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "add tab" })).toBeTruthy();
  });
  it("disables delete when only one tab remains", () => {
    render(<DashboardTabBar tabs={[tab("a", "Only", 0)]} {...baseProps} />);
    expect((screen.getByRole("button", { name: "delete tab Only" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
```

- [ ] **Step 2: Run — confirm FAIL.**
`npx vitest run src/components/chart/dashboard/__tests__/CardEditToolbar.test.tsx src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx`
Expected: the toolbar test passes accidentally only if labels already match (they do today, but icons are emoji) — to make this a real TDD step, the new tests primarily lock current aria-labels; they will PASS pre-change. That is acceptable here because Task 1 is a visual refactor with behavior-preserving aria-labels: the tests guard against regressions while you swap the icons. Run them, see them green, then refactor and keep them green.

- [ ] **Step 3: Rewrite `CardEditToolbar.tsx`** (lucide icons, larger hit area, tooltips):
```tsx
import { Settings, BarChart3, Copy, RefreshCw, Trash2 } from "lucide-react";

interface Props {
  cardType: string;
  onSettings: () => void;
  onViz: () => void;
  onDuplicate: () => void;
  onReplace: () => void;
  onRemove: () => void;
}

export function CardEditToolbar({ cardType, onSettings, onViz, onDuplicate, onReplace, onRemove }: Props) {
  const btn = "inline-flex size-7 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground";
  return (
    <div className="dash-nodrag flex items-center gap-0.5 rounded-md border bg-card p-0.5 shadow-sm">
      <button type="button" aria-label="card settings" title="Settings" className={btn} onClick={onSettings}><Settings className="h-4 w-4" /></button>
      {cardType === "chart" && <button type="button" aria-label="card visualization" title="Visualization" className={btn} onClick={onViz}><BarChart3 className="h-4 w-4" /></button>}
      <button type="button" aria-label="duplicate card" title="Duplicate" className={btn} onClick={onDuplicate}><Copy className="h-4 w-4" /></button>
      {cardType === "chart" && <button type="button" aria-label="replace card" title="Replace chart" className={btn} onClick={onReplace}><RefreshCw className="h-4 w-4" /></button>}
      <button type="button" aria-label="remove card" title="Remove" className={`${btn} text-destructive hover:text-destructive`} onClick={onRemove}><Trash2 className="h-4 w-4" /></button>
    </div>
  );
}
```

- [ ] **Step 4: Edit `DashboardTabBar.tsx`** — import lucide icons, wrap each tab item in `group`, replace emoji, hover-reveal actions. Change the import line 1 area to add:
```tsx
import { Copy, X, Plus } from "lucide-react";
```
Change the tab item wrapper (line ~37) from `className="flex items-center"` to `className="group flex items-center"`. Replace the `{editing && (<>…</>)}` action block (lines ~47-53) with:
```tsx
            {editing && (
              <>
                <button type="button" aria-label={`duplicate tab ${t.name}`} title="Duplicate tab" className="rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:bg-accent hover:text-foreground group-hover:opacity-100" onClick={() => onDuplicate(t.key)}><Copy className="h-3.5 w-3.5" /></button>
                <button type="button" aria-label={`delete tab ${t.name}`} title="Delete tab" className="rounded p-0.5 text-destructive opacity-0 transition-opacity hover:bg-accent group-hover:opacity-100 disabled:cursor-not-allowed disabled:opacity-30"
                  disabled={ordered.length < 2} onClick={() => onDelete(t.key)}><X className="h-3.5 w-3.5" /></button>
              </>
            )}
```
Replace the add-tab button (line ~58) with:
```tsx
      {editing && (
        <button type="button" aria-label="add tab" title="Add tab" className="inline-flex items-center rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground" onClick={onAdd}><Plus className="h-4 w-4" /></button>
      )}
```

- [ ] **Step 5: Run the two tests — PASS.** Then full sweep + commit.
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/CardEditToolbar.tsx src/components/chart/dashboard/DashboardTabBar.tsx src/components/chart/dashboard/__tests__/CardEditToolbar.test.tsx src/components/chart/dashboard/__tests__/DashboardTabBar.test.tsx
git commit -m "feat(dashboards): lucide icons for card toolbar + tab controls

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Searchable "change chart" picker (②) + shared icons

**Files:**
- Create: `frontend/src/components/chart/dashboard/viz-icons.ts`
- Create: `frontend/src/components/chart/dashboard/ChartPickerList.tsx`
- Modify: `frontend/src/components/chart/dashboard/AddPanel.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/ChartPickerList.test.tsx` (create)

- [ ] **Step 1: Write the failing test** `ChartPickerList.test.tsx`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChartPickerList } from "../ChartPickerList";

const chart = (id: string, name: string, viz = "bar") => ({ id, name, viz_type: viz, dataset_id: "d", description: "", created_by: null, updated_at: "", params: { spec: {} } }) as never;

describe("ChartPickerList", () => {
  it("filters rows by the search query", async () => {
    const user = userEvent.setup();
    render(<ChartPickerList charts={[chart("1", "Revenue bar"), chart("2", "Orders table", "table")]} onPick={() => {}} />);
    expect(screen.getByRole("button", { name: "Revenue bar" })).toBeTruthy();
    await user.type(screen.getByRole("textbox", { name: "search charts" }), "orders");
    expect(screen.queryByRole("button", { name: "Revenue bar" })).toBeNull();
    expect(screen.getByRole("button", { name: "Orders table" })).toBeTruthy();
  });
  it("calls onPick with the chart when a row is clicked", async () => {
    const user = userEvent.setup();
    const onPick = vi.fn();
    const c = chart("1", "Revenue bar");
    render(<ChartPickerList charts={[c]} onPick={onPick} />);
    await user.click(screen.getByRole("button", { name: "Revenue bar" }));
    expect(onPick).toHaveBeenCalledWith(c);
  });
  it("shows an empty state when no chart matches", async () => {
    const user = userEvent.setup();
    render(<ChartPickerList charts={[chart("1", "Revenue bar")]} onPick={() => {}} />);
    await user.type(screen.getByRole("textbox", { name: "search charts" }), "zzz");
    expect(screen.getByText("No charts")).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run — FAIL** (module missing): `npx vitest run src/components/chart/dashboard/__tests__/ChartPickerList.test.tsx`

- [ ] **Step 3a: Create `viz-icons.ts`:**
```ts
import { CHART_TYPES } from "@/components/chart/chartTypes";

/** viz_type → lucide icon component, sourced from the canonical chart-type registry. */
export const ICON_BY_VIZ = new Map(CHART_TYPES.map((t) => [t.key, t.icon] as const));
```

- [ ] **Step 3b: Create `ChartPickerList.tsx`:**
```tsx
import { useState } from "react";
import { BarChart3 } from "lucide-react";
import type { Chart } from "@/api/charts";
import { ICON_BY_VIZ } from "./viz-icons";

interface Props {
  charts: Chart[];
  onPick: (c: Chart) => void;
  autoFocus?: boolean;
}

export function ChartPickerList({ charts, onPick, autoFocus }: Props) {
  const [q, setQ] = useState("");
  const shown = charts.filter((c) => c.name.toLowerCase().includes(q.toLowerCase()));
  return (
    <div className="flex flex-col gap-2">
      <input autoFocus={autoFocus} aria-label="search charts" placeholder="Search charts…" className="h-8 w-full rounded border bg-background px-2 text-sm" value={q} onChange={(e) => setQ(e.target.value)} />
      <div className="-mx-1 flex max-h-[60vh] flex-col overflow-auto px-1">
        {shown.length === 0 ? <p className="px-1 py-2 text-xs text-muted-foreground">No charts</p>
          : shown.map((c) => {
            const Icon = ICON_BY_VIZ.get(c.viz_type) ?? BarChart3;
            return (
              <button key={c.id} type="button" aria-label={c.name} className="flex items-center gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent/60" onClick={() => onPick(c)}>
                <Icon className="h-4 w-4 shrink-0 text-primary" aria-hidden />
                <span className="min-w-0 flex-1 truncate">{c.name}</span>
              </button>
            );
          })}
      </div>
    </div>
  );
}
```

- [ ] **Step 3c: Refactor `AddPanel.tsx`** to reuse the shared icons + picker list (drops its local `ICON_BY_VIZ`, search, and list):
```tsx
import type { Chart } from "@/api/charts";
import { ChartPickerList } from "./ChartPickerList";

interface Props { charts: Chart[]; onAddChart: (c: Chart) => void; onAddVirtual: (t: "text" | "heading" | "link") => void }

export function AddPanel({ charts, onAddChart, onAddVirtual }: Props) {
  return (
    <div className="flex h-full flex-col gap-2 p-3">
      <div className="text-sm font-medium">Add a chart</div>
      <ChartPickerList charts={charts} onPick={onAddChart} />
      <div className="flex gap-1 border-t pt-2">
        <button type="button" aria-label="Add Text" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("text")}>+ Text</button>
        <button type="button" aria-label="Add Heading" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("heading")}>+ Heading</button>
        <button type="button" aria-label="Add Link" className="rounded border px-2 py-1 text-xs hover:bg-accent/40" onClick={() => onAddVirtual("link")}>+ Link</button>
      </div>
    </div>
  );
}
```

- [ ] **Step 3d: Swap the Replace dialog body in `DashboardView.tsx`.** Add the import near the other dashboard-component imports (after line ~30):
```tsx
import { ChartPickerList } from "@/components/chart/dashboard/ChartPickerList";
```
Replace the `<div className="flex flex-wrap gap-1 py-2">…</div>` block inside the Replace `Dialog` (lines ~493-499) with:
```tsx
          <ChartPickerList charts={charts ?? []} onPick={(ch) => replaceCard(replacingId!, ch)} autoFocus />
```

- [ ] **Step 4: Run the test — PASS.** Full sweep + commit.
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/viz-icons.ts src/components/chart/dashboard/ChartPickerList.tsx src/components/chart/dashboard/AddPanel.tsx src/routes/explore/DashboardView.tsx src/components/chart/dashboard/__tests__/ChartPickerList.test.tsx
git commit -m "feat(dashboards): searchable chart picker (shared list for Add + Replace)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Collapsible add-chart sidebar (③)

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Modify: `frontend/src/components/chart/dashboard/EditSidebar.tsx`
- Test: none new (pure UI toggle — verified live in Task 6). Keep the full sweep green.

- [ ] **Step 1: Add a collapse button to `EditSidebar.tsx`.** Add `PanelLeftClose` import and an `onCollapse` prop, render a header row with the collapse button. Replace the top of the component:
```tsx
import { PanelLeftClose } from "lucide-react";
```
Add to `Props`:
```tsx
  onCollapse: () => void;
```
Replace the `<aside …>` opening + the existing `{onPanel && (…back to add…)}` header with:
```tsx
    <aside className="sticky top-4 max-h-[calc(100vh-7rem)] w-[340px] shrink-0 self-start overflow-auto rounded-lg border bg-card">
      <div className="flex items-center justify-between border-b px-2 py-1.5">
        {onPanel
          ? <button type="button" aria-label="back to add" className="text-xs text-muted-foreground hover:text-foreground" onClick={props.onClose}>‹ Add cards</button>
          : <span className="px-1 text-xs font-medium text-muted-foreground">Add cards</span>}
        <button type="button" aria-label="collapse sidebar" title="Hide panel" className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground" onClick={props.onCollapse}><PanelLeftClose className="h-4 w-4" /></button>
      </div>
```
(Keep the rest of the panel-render JSX as-is.)

- [ ] **Step 2: In `DashboardView.tsx`, add collapse state + rail + auto-expand.** Add `PanelLeftOpen` to the lucide import (line 2: `import { Info, Pencil, Star, PanelLeftOpen } from "lucide-react";`). Add state near the other edit state (after line ~71):
```tsx
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  // Opening any settings panel must reveal the sidebar even if the user collapsed it.
  const openPanel = useCallback((p: EditPanel) => { setEditPanel(p); setSidebarCollapsed(false); }, []);
```
Replace the toolbar callbacks `onSettings`/`onViz` (lines ~397-398) to use `openPanel`:
```tsx
                    onSettings={() => openPanel({ kind: "card", cardId: c.id })}
                    onViz={() => openPanel({ kind: "viz", cardId: c.id })}
```
Replace the filter-chip select (line ~361) to use `openPanel`:
```tsx
        ? <EditFilterChips parameters={parameters} activeId={editPanel?.kind === "filter" ? editPanel.paramId : null} onSelect={(id) => openPanel({ kind: "filter", paramId: id })} />
```
Wrap the sidebar render (lines ~423-443) so collapsed shows a rail:
```tsx
        {editing && (sidebarCollapsed ? (
          <div className="sticky top-4 self-start">
            <button type="button" aria-label="expand sidebar" title="Show panel" className="inline-flex size-9 items-center justify-center rounded-lg border bg-card text-muted-foreground hover:bg-accent hover:text-foreground" onClick={() => setSidebarCollapsed(false)}>
              <PanelLeftOpen className="h-4 w-4" />
            </button>
          </div>
        ) : (
          <EditSidebar
            panel={editPanel}
            charts={charts ?? []}
            /* …keep all existing EditSidebar props unchanged… */
            onClose={() => setEditPanel({ kind: "add" })}
            onCollapse={() => setSidebarCollapsed(true)}
          />
        ))}
```
(Keep every existing `EditSidebar` prop; only add `onCollapse` and wrap in the collapsed/expanded conditional.)

- [ ] **Step 3: Full sweep + commit.**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/routes/explore/DashboardView.tsx src/components/chart/dashboard/EditSidebar.tsx
git commit -m "feat(dashboards): collapsible edit sidebar (auto-expands on panel open)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Resize — render-scale + grid overlay + edge handles (⑤)

**Files:**
- Modify: `frontend/src/components/chart/dashboard-grid.ts`
- Create: `frontend/src/components/chart/dashboard/GridOverlay.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/components/chart/__tests__/dashboard-grid.test.ts` (create)

- [ ] **Step 1: Write the failing test** `dashboard-grid.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { ROW_PX, ROW_SCALE, toLayoutRows, fromLayoutRows, contentRowsFor } from "../dashboard-grid";

describe("dashboard-grid render scale", () => {
  it("renders a stored row as ROW_SCALE layout rows", () => {
    expect(toLayoutRows(4)).toBe(12);
    expect(toLayoutRows(0)).toBe(0);
  });
  it("round-trips stored↔layout for untouched values", () => {
    for (const r of [0, 1, 2, 4, 6, 10]) expect(fromLayoutRows(toLayoutRows(r))).toBe(r);
  });
  it("keeps the same pixel height as the old 90px grid", () => {
    expect(ROW_PX * ROW_SCALE).toBe(90); // 4 stored rows → 12 × 30px = 360px (was 4 × 90px)
  });
  it("contentRowsFor rounds content px up to whole stored rows (min 1)", () => {
    expect(contentRowsFor(89)).toBe(1);
    expect(contentRowsFor(90)).toBe(1);
    expect(contentRowsFor(91)).toBe(2);
    expect(contentRowsFor(0)).toBe(1);
  });
});
```

- [ ] **Step 2: Run — FAIL** (exports missing): `npx vitest run src/components/chart/__tests__/dashboard-grid.test.ts`

- [ ] **Step 3a: Extend `dashboard-grid.ts`** (append; keep existing exports):
```ts
// Render-time vertical scale: stored geometry is unchanged on disk, but the grid renders at a
// finer rowHeight (30px) with ×3 the row units, so each stored row = 3 drag steps. A 6×4 stored
// card still renders at 360px (12 × 30). Persisted size_y/grid_row are divided back on save.
export const ROW_PX = 30;
export const ROW_SCALE = 3;

export function toLayoutRows(storedRows: number): number {
  return storedRows * ROW_SCALE;
}

export function fromLayoutRows(layoutRows: number): number {
  return Math.round(layoutRows / ROW_SCALE);
}

/** Stored rows needed to fit `px` of content (auto-height). One stored row = ROW_PX*ROW_SCALE px. */
export function contentRowsFor(px: number): number {
  return Math.max(1, Math.ceil(px / (ROW_PX * ROW_SCALE)));
}
```

- [ ] **Step 3b: Create `GridOverlay.tsx`:**
```tsx
import { useEffect, useRef, useState } from "react";

/** Faint cell grid shown behind the dashboard in edit mode (Metabase-style). Non-interactive. */
export function GridOverlay({ cols, rowPx }: { cols: number; rowPx: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setW(el.clientWidth));
    ro.observe(el);
    setW(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  const colW = w > 0 ? w / cols : 0;
  return (
    <div ref={ref} aria-hidden className="pointer-events-none absolute inset-0 z-0"
      style={colW ? {
        backgroundImage: "linear-gradient(to right, hsl(var(--border)) 1px, transparent 1px), linear-gradient(to bottom, hsl(var(--border)) 1px, transparent 1px)",
        backgroundSize: `${colW}px ${rowPx}px`,
        opacity: 0.4,
      } : undefined} />
  );
}
```

- [ ] **Step 3c: Wire into `DashboardView.tsx`.**
1. Imports: remove the local `const ROW_PX = 90;` (line 35). Extend the dashboard-grid import (line 19) and add GridOverlay:
```tsx
import { GRID_COLS, CARD_MIN_SIZE, CARD_DEFAULT_SIZE, ROW_PX, ROW_SCALE, toLayoutRows, fromLayoutRows } from "@/components/chart/dashboard-grid";
import { GridOverlay } from "@/components/chart/dashboard/GridOverlay";
```
2. Scale the `layout` map (lines ~184-189) vertically:
```tsx
  const layout: Layout[] = visibleCards.map((c) => ({
    i: c.id, x: c.grid_col, y: toLayoutRows(c.grid_row), w: c.size_x, h: toLayoutRows(c.size_y),
    minW: Math.min(CARD_MIN_SIZE.w, c.size_x), minH: toLayoutRows(Math.min(CARD_MIN_SIZE.h, c.size_y)),
  }));
```
3. Un-scale in `onLayoutChange` (lines ~196-201): compute back stored rows and compare against stored values:
```tsx
      const merged = base.map((c) => {
        const l = next.find((x) => x.i === c.id);
        if (!l) return c;
        const row = Math.max(0, fromLayoutRows(l.y));
        const sizeY = Math.max(1, fromLayoutRows(l.h));
        if (l.x === c.grid_col && row === c.grid_row && l.w === c.size_x && sizeY === c.size_y) return c;
        changed = true;
        return { ...c, grid_col: l.x, grid_row: row, size_x: l.w, size_y: sizeY };
      });
```
4. Add `resizeHandles` + smaller rowHeight to `<Grid>` (lines ~376-387) and wrap it with the overlay. Replace the `) : (` … `<Grid …>` opening with:
```tsx
      ) : (
        <div className="relative">
          {editing && <GridOverlay cols={GRID_COLS} rowPx={ROW_PX} />}
          <Grid
            className="layout relative z-10"
            cols={GRID_COLS}
            rowHeight={ROW_PX}
            margin={[12, 12]}
            draggableHandle=".dash-draghandle"
            draggableCancel=".dash-nodrag"
            isDraggable={editing}
            isResizable={editing}
            resizeHandles={["s", "e", "se"]}
            layout={layout}
            onLayoutChange={onLayoutChange}
          >
```
And close it: change the matching `</Grid>` (line ~420) to:
```tsx
          </Grid>
        </div>
```

- [ ] **Step 4: Run the grid test — PASS.** Full sweep + commit.
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard-grid.ts src/components/chart/dashboard/GridOverlay.tsx src/routes/explore/DashboardView.tsx src/components/chart/__tests__/dashboard-grid.test.ts
git commit -m "feat(dashboards): finer resize grid (render scale ×3) + grid overlay + edge handles

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Auto-height for content cards (⑥)

**Files:**
- Create: `frontend/src/components/chart/dashboard/AutoHeightMeasure.tsx`
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`
- Modify: `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx`
- Test: `frontend/src/components/chart/__tests__/dashboard-grid.test.ts` (already covers `contentRowsFor`; no new pure-fn test needed). Measurement + lock are verified live (Task 6).

**Eligibility:** text/heading/link cards always; chart cards only when the chart's `viz_type` is a table/pivot. Before coding, open `src/components/chart/chartTypes.ts` and confirm the exact `key` strings for the Table and Pivot viz types (expected `"table"` and `"pivot_table"`). Use the confirmed strings in the `isAutoHeightEligible` helper below.

- [ ] **Step 1: Create `AutoHeightMeasure.tsx`:**
```tsx
import { useEffect, useRef } from "react";
import { contentRowsFor } from "@/components/chart/dashboard-grid";

/** Measures its content's natural height and reports the stored-row count when it changes by ≥1 row. */
export function AutoHeightMeasure({ onRows, children }: { onRows: (rows: number) => void; children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const last = useRef(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const px = el.scrollHeight;
      if (px <= 0) return;
      const rows = contentRowsFor(px);
      if (rows !== last.current) { last.current = rows; onRows(rows); }
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, [onRows]);
  return <div ref={ref}>{children}</div>;
}
```

- [ ] **Step 2: Wire measuring into `DashboardCardView.tsx`.** Add props `autoHeight?: boolean; onAutoHeight?: (rows: number) => void` to BOTH `DashboardCardView` and `ChartCardTile`. Import the measurer:
```tsx
import { AutoHeightMeasure } from "./AutoHeightMeasure";
```
In `ChartCardTile`, extract the render body and wrap it when auto-height is active. Replace the content container (lines ~36-44) with:
```tsx
      <div ref={chartRef} className="min-h-0 flex-1 overflow-auto px-2 pb-2">
        {(() => {
          const body = blocked ? <div className="flex h-full items-center justify-center px-3 text-center text-sm text-muted-foreground">Set the required filters to load this card</div>
            : data.error ? <div className="text-sm text-destructive">{data.error}</div>
            : data.isLoading ? <div className="text-sm text-muted-foreground">Loading…</div>
            : (() => {
              const eff = applyVizOverride(chart, card.options.vizOverride as Record<string, unknown> | undefined);
              return <ChartRenderer vizType={eff.viz_type} params={eff.params} columns={data.columns} rows={data.rows} onValueClick={onValueClick} />;
            })();
          return autoHeight && onAutoHeight ? <AutoHeightMeasure onRows={onAutoHeight}>{body}</AutoHeightMeasure> : body;
        })()}
      </div>
```
Update `ChartCardTile`'s destructured props + type to include `autoHeight, onAutoHeight`. In `DashboardCardView`, thread them through and wrap the virtual-card returns. Replace the function body (lines ~50-61) with:
```tsx
export function DashboardCardView({ ws, card, chart, refreshNonce, parameters, paramValues, onValueClick, blocked, editing, autoHeight, onAutoHeight }: { ws: string; card: DashboardCard; chart: Chart | undefined; refreshNonce?: number; parameters?: DashboardParameter[]; paramValues?: Record<string, unknown>; onValueClick?: (ctx: DrillContext, x: number, y: number) => void; blocked?: boolean; editing?: boolean; autoHeight?: boolean; onAutoHeight?: (rows: number) => void }) {
  const wrap = (body: React.ReactNode) => (autoHeight && onAutoHeight ? <AutoHeightMeasure onRows={onAutoHeight}>{body}</AutoHeightMeasure> : body);
  if (card.card_type === "text") {
    const text = interpolateParams(asString(card.options.text) ?? "", parameters ?? [], paramValues ?? {});
    return wrap(<TextCard text={text} />);
  }
  if (card.card_type === "heading") {
    const text = interpolateParams(asString(card.options.text) ?? "", parameters ?? [], paramValues ?? {});
    return wrap(<HeadingCard text={text} />);
  }
  if (card.card_type === "link") return wrap(<LinkCard url={asString(card.options.url)} label={asString(card.options.label)} />);
  return <ChartCardTile ws={ws} card={card} chart={chart} refreshNonce={refreshNonce} onValueClick={onValueClick} blocked={blocked} editing={editing} autoHeight={autoHeight} onAutoHeight={onAutoHeight} />;
}
```

- [ ] **Step 3: Add the auto-height toggle to `CardSettingsPanel.tsx`.** Add a helper + a reusable toggle row, and render it in the eligible blocks. After the `moveToTab` const (line ~25) add (replace `"table"`/`"pivot_table"` with the confirmed keys if different):
```tsx
  const eligibleChart = card.card_type === "chart" && (chart?.viz_type === "table" || chart?.viz_type === "pivot_table");
  const eligible = card.card_type === "text" || card.card_type === "heading" || card.card_type === "link" || eligibleChart;
  const autoHeightRow = eligible ? (
    <label className="flex items-center justify-between gap-2 text-sm"><span>Auto height</span>
      <input type="checkbox" aria-label="auto height" checked={!!card.options.autoHeight} onChange={(e) => onUpdateOptions({ autoHeight: e.target.checked })} />
    </label>
  ) : null;
```
Render `{autoHeightRow}` next to `{moveToTab}` in each return: the text/heading block (after line ~31), the link block (after line ~41), and the chart block (after line ~83, before `{moveToTab}`).

- [ ] **Step 4: Wire DashboardView to apply auto-height + lock vertical resize.** In `DashboardView.tsx`:
1. Add an interaction guard ref near the other refs (after line ~64):
```tsx
  const interactingRef = useRef(false);
```
2. Add an apply function near `onLayoutChange` (after line ~204):
```tsx
  function applyAutoHeight(cardId: string, rows: number) {
    if (interactingRef.current) return;            // never fight an active drag/resize
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      let changed = false;
      const merged = base.map((c) => {
        if (c.id !== cardId || c.size_y === rows) return c;
        changed = true;
        return { ...c, size_y: rows };
      });
      return changed ? merged : base;
    });
  }
```
   (If the existing draft seed helper is not named `cardsAsDraft`, use the same expression `onLayoutChange` uses to seed `base` — match it exactly.)
3. Lock vertical for auto-height cards in the `layout` map (extend Task 4's map):
```tsx
  const layout: Layout[] = visibleCards.map((c) => {
    const h = toLayoutRows(c.size_y);
    const autoH = !!c.options.autoHeight;
    return {
      i: c.id, x: c.grid_col, y: toLayoutRows(c.grid_row), w: c.size_x, h,
      minW: Math.min(CARD_MIN_SIZE.w, c.size_x),
      minH: autoH ? h : toLayoutRows(Math.min(CARD_MIN_SIZE.h, c.size_y)),
      ...(autoH ? { maxH: h } : {}),
    };
  });
```
4. Set the interaction guard via `<Grid>` start/stop handlers (add to the `<Grid …>` props from Task 4):
```tsx
            onResizeStart={() => { interactingRef.current = true; }}
            onResizeStop={() => { interactingRef.current = false; }}
            onDragStart={() => { interactingRef.current = true; }}
            onDragStop={() => { interactingRef.current = false; }}
```
5. Pass auto-height props to the card (in the `<DashboardCardView … />` at line ~406), only in edit mode (view uses the saved height):
```tsx
                  blocked={!editing && requiredUnset} editing={editing}
                  autoHeight={!!c.options.autoHeight}
                  onAutoHeight={editing ? (rows) => applyAutoHeight(c.id, rows) : undefined}
```

- [ ] **Step 5: Run `npx vitest run src/components/chart/__tests__/dashboard-grid.test.ts`** (contentRowsFor already covered) — PASS. Full sweep + commit.
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/AutoHeightMeasure.tsx src/components/chart/dashboard/DashboardCardView.tsx src/components/chart/dashboard/CardSettingsPanel.tsx src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboards): auto-height for table + text/heading/link cards

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Live verification + PR

**Files:** none (verification only).

- [ ] **Step 1: Final sweep** (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run` → green.

- [ ] **Step 2: Run the stack** (frontend-only; dev backend on :8080 is fine), `npm run dev`, log in `admin@egret.local` / `Admin12345!`, eng workspace. **Do NOT touch "a1" or "PARITY — all features".** Create a NEW throwaway dashboard via "+ Dashboard" (avoids the duplicate-name 409 seen with trashed copies) and add a couple of charts to it in edit mode (one **Table** chart + one **Bar** chart). Delete the throwaway at the end.

- [ ] **Step 3: Live-verify (Playwright MCP, 0 console errors):**
1. **Icons (①④):** card hover toolbar shows crisp lucide icons (settings/viz/duplicate/replace/remove) with larger hit areas; tab duplicate/delete appear on tab hover as lucide Copy/X; add-tab is a Plus.
2. **Change chart (②):** the replace icon opens a dialog with a **search box + scrollable list** (viz icons + names); typing filters; picking replaces the card. No wall of pills.
3. **Sidebar (③):** the collapse button hides the 340px panel → a thin rail with an expand button; the grid widens; clicking a card's Settings re-expands the panel to that card.
4. **Resize (⑤):** in edit mode a faint **grid overlay** is visible; dragging the card's bottom/right/corner handles resizes in ~30px steps (finer than before); an **existing** dashboard's card pixel sizes are unchanged after this change (open PARITY read-only — do NOT edit/save — and confirm cards look identical).
5. **Auto-height (⑥):** on the throwaway, enable "Auto height" in a **Table** card's settings → the card height snaps to fit the table and vertical resize is locked (only width handles act); toggling off restores manual resize. The Bar chart card shows **no** Auto-height toggle.
6. Re-confirm chart **export** still works (Tier P1 regression): a chart card's PNG is a full image (not 32×32).

- [ ] **Step 4: Clean up & open PR.** Delete the throwaway dashboard. Then:
```bash
git push -u origin feat/dashboard-edit-ux
```
Open PR `feat(dashboards): edit-mode UX polish (icons, picker, sidebar, resize, auto-height)`; summarise the six items; note frontend-only, stored geometry unchanged (render-scale ×3), auto-height for content cards only. End the PR body with the `🤖 Generated with [Claude Code]` footer. Squash-merge when CI is green (a flaky `egret-ranger` rust test may need a rerun — unrelated to this frontend change).

---

## Self-Review

**Spec coverage:**
- ① card icons → Task 1. ④ tab icons → Task 1. ✓
- ② change-chart searchable picker + shared ICON_BY_VIZ → Task 2. ✓
- ③ collapsible sidebar + auto-expand → Task 3. ✓
- ⑤ render-scale (ROW_PX 30 / ROW_SCALE 3 / helpers) + grid overlay + `resizeHandles ['s','e','se']` → Task 4. ✓
- ⑥ auto-height (eligibility, measure, ≥1-row threshold, skip-during-drag, maxH/minH lock) → Task 5. ✓
- Backward-compat (existing pixels unchanged) → Task 4 test (`ROW_PX*ROW_SCALE===90`) + Task 6 live check. ✓
- Testing per spec: ChartPickerList, render-scale helpers, contentRowsFor, CardEditToolbar, DashboardTabBar (unit); sidebar/resize/auto-height (live). ✓

**Type/signature consistency:**
- `toLayoutRows`/`fromLayoutRows`/`contentRowsFor`/`ROW_PX`/`ROW_SCALE` defined in Task 4 `dashboard-grid.ts`, consumed in Tasks 4-5 (`DashboardView`, `AutoHeightMeasure`).
- `ChartPickerList({charts,onPick,autoFocus})` defined Task 2, reused in AddPanel + Replace dialog (Task 2).
- `ICON_BY_VIZ` from `viz-icons.ts` (Task 2) used by ChartPickerList + AddPanel.
- `EditSidebar` gains `onCollapse` (Task 3) — supplied by DashboardView (Task 3).
- `DashboardCardView`/`ChartCardTile` gain `autoHeight`/`onAutoHeight` (Task 5) — supplied by DashboardView (Task 5).
- `CardEditToolbar` props unchanged (Task 1) — call site in DashboardView unaffected except `openPanel` (Task 3).

**Placeholder scan:** No TBD/TODO. The one confirm-before-coding note (Task 5 viz_type keys) is bounded with an expected value (`"table"`/`"pivot_table"`) and a one-file lookup. Task 3 has no unit test by design (pure toggle, live-verified) — called out explicitly.

**Ordering note:** Tasks 4 and 5 both edit the `layout` map and the `<Grid>` element in DashboardView; Task 5 extends what Task 4 wrote (autoH `maxH`/`minH` and the start/stop handlers). Apply them in order.
