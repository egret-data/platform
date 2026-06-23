# Metabase-faithful dashboard grid — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace egret's fixed-rowHeight + ×3 render-scale grid with Metabase's real algorithm — dynamic aspect-ratio rowHeight, vertical compaction, 6px margins, per-viz default/min sizes, an SVG grid background, and a single SE resize handle.

**Architecture:** A new `DashboardGridLayout` wrapper measures container width → computes `rowHeight = max((width/24)/(10/9), 40)`, draws a Metabase-style SVG cell grid behind a plain `react-grid-layout` (compactType vertical, margin [6,6]). Stored geometry (24 cols, grid units) is unchanged; existing dashboards re-flow (accepted). Continues on `feat/dashboard-edit-ux` (folds into PR #39).

**Tech Stack:** React 18 + TS + Vite, Vitest + @testing-library/react, react-grid-layout, lucide-react, Tailwind.

**Spec:** `docs/en/specs/2026-06-23-metabase-grid-design.md`. Clean-room: reimplement Metabase's documented math, do NOT copy its source verbatim.

**Conventions (every task):** TDD (failing test first). Full sweep before each commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage explicit paths. Configured git identity. End every commit message with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Do NOT push until Task 6.

## File map

- Modify `src/components/chart/dashboard-grid.ts` — Metabase constants/helpers; remove scale hack (Task 1)
- Modify `src/components/chart/__tests__/dashboard-grid.test.ts` — replace ×3 tests (Task 1)
- Create `src/components/chart/dashboard/grid-background.ts` — SVG grid pattern (Task 2)
- Delete `src/components/chart/dashboard/GridOverlay.tsx` (Task 2)
- Create `src/components/chart/dashboard/DashboardGridLayout.tsx` — the grid wrapper (Task 3)
- Create `src/components/chart/dashboard/dashboard-grid.css` — Metabase resize-handle look (Task 5)
- Modify `src/routes/explore/DashboardView.tsx` — use the wrapper; raw-unit layout; per-viz sizing (Task 4)
- Modify `src/components/chart/dashboard/AutoHeightMeasure.tsx` — rowHeight-aware (Task 5)
- Modify `src/components/chart/dashboard/DashboardCardView.tsx` — thread rowHeight (Task 5)

---

### Task 1: `dashboard-grid.ts` — Metabase math, drop the scale hack

**Files:**
- Modify: `frontend/src/components/chart/dashboard-grid.ts`
- Test: `frontend/src/components/chart/__tests__/dashboard-grid.test.ts` (rewrite)

- [ ] **Step 1: Rewrite the test** `dashboard-grid.test.ts` (replaces the PR-#39 ×3/pixel-identity cases):
```ts
import { describe, it, expect } from "vitest";
import {
  GRID_COLS, GRID_MARGIN, gridRowHeight, cellWidth, contentRowsFor,
  getPositionForNewDashCard, sizeForViz,
} from "../dashboard-grid";

describe("gridRowHeight (Metabase aspect formula)", () => {
  it("is (width/24)/(10/9), floored", () => {
    expect(gridRowHeight(1200)).toBe(45); // floor(1200/24/(10/9)) = floor(45)
  });
  it("clamps to the 40px minimum for small widths", () => {
    expect(gridRowHeight(0)).toBe(40);
    expect(gridRowHeight(500)).toBe(40); // floor(500/24/(10/9)) ≈ 18 → 40
  });
});

describe("cellWidth", () => {
  it("subtracts the inter-column margins then divides by cols", () => {
    expect(cellWidth(1200)).toBeCloseTo((1200 - (GRID_COLS - 1) * GRID_MARGIN[0]) / GRID_COLS); // (1200-138)/24
  });
});

describe("contentRowsFor(px, rowHeight)", () => {
  it("counts whole rows of (rowHeight + verticalMargin), min 1", () => {
    // rowHeight 45, marginY 6 → row pitch 51
    expect(contentRowsFor(0, 45)).toBe(1);
    expect(contentRowsFor(51, 45)).toBe(1);
    expect(contentRowsFor(52, 45)).toBe(2);
    expect(contentRowsFor(102, 45)).toBe(2);
  });
});

describe("getPositionForNewDashCard (first free slot, L→R T→B)", () => {
  it("places the first card at the origin", () => {
    expect(getPositionForNewDashCard([], 4, 4)).toEqual({ grid_col: 0, grid_row: 0 });
  });
  it("places the next card to the right when there is room", () => {
    const cards = [{ grid_col: 0, grid_row: 0, size_x: 4, size_y: 4 }];
    expect(getPositionForNewDashCard(cards, 4, 4)).toEqual({ grid_col: 4, grid_row: 0 });
  });
  it("wraps to the next row when the row is full", () => {
    const cards = [{ grid_col: 0, grid_row: 0, size_x: 24, size_y: 2 }];
    expect(getPositionForNewDashCard(cards, 6, 4)).toEqual({ grid_col: 0, grid_row: 2 });
  });
});

describe("sizeForViz", () => {
  it("returns Metabase sizes for known viz", () => {
    expect(sizeForViz("table")).toEqual({ default: { w: 12, h: 9 }, min: { w: 4, h: 3 } });
    expect(sizeForViz("scalar")).toEqual({ default: { w: 6, h: 3 }, min: { w: 2, h: 2 } });
    expect(sizeForViz("heading")).toEqual({ default: { w: 24, h: 1 }, min: { w: 1, h: 1 } });
  });
  it("maps egret-specific keys", () => {
    expect(sizeForViz("pivot_table").default).toEqual({ w: 12, h: 9 });
    expect(sizeForViz("smartscalar").min).toEqual({ w: 2, h: 2 });
  });
  it("falls back to a generic chart size for unknown viz", () => {
    expect(sizeForViz("boxplot")).toEqual({ default: { w: 12, h: 6 }, min: { w: 4, h: 3 } });
    expect(sizeForViz("totally-unknown")).toEqual({ default: { w: 12, h: 6 }, min: { w: 4, h: 3 } });
  });
});
```

- [ ] **Step 2: Run — FAIL** (helpers missing/renamed): `npx vitest run src/components/chart/__tests__/dashboard-grid.test.ts`

- [ ] **Step 3: Rewrite `dashboard-grid.ts`** to:
```ts
// Faithful port of Metabase's dashboard grid math (frontend/src/metabase/lib/dashboard_grid.js
// + DashboardGrid.getRowHeight + dashboards/constants.cljc). Clean-room reimplementation.
export const GRID_COLS = 24;             // GRID_WIDTH
export const GRID_ASPECT_RATIO = 10 / 9; // cell w:h
export const GRID_MIN_ROW_HEIGHT = 40;   // px (MIN_ROW_HEIGHT)
export const GRID_MARGIN: [number, number] = [6, 6];   // [horizontal, vertical] gap between cards
export const GRID_PADDING: [number, number] = [0, 0];  // container padding
export const CARD_MIN_SIZE = { w: 2, h: 2 };

/** rowHeight in px: each cell is GRID_ASPECT_RATIO wider than tall, clamped to the 40px floor. */
export function gridRowHeight(containerWidth: number): number {
  return Math.max(Math.floor(containerWidth / GRID_COLS / GRID_ASPECT_RATIO), GRID_MIN_ROW_HEIGHT);
}

/** Pixel width of one grid column after removing the inter-column margins. */
export function cellWidth(containerWidth: number): number {
  return (containerWidth - (GRID_COLS - 1) * GRID_MARGIN[0]) / GRID_COLS;
}

/** Stored rows needed to fit `px` of content (auto-height). A card of n rows occupies
 *  n*rowHeight + (n-1)*marginY px, so the row pitch is rowHeight + marginY. */
export function contentRowsFor(px: number, rowHeight: number): number {
  return Math.max(1, Math.ceil(px / (rowHeight + GRID_MARGIN[1])));
}

interface GridRect { grid_col: number; grid_row: number; size_x: number; size_y: number }

function intersects(a: GridRect, b: GridRect): boolean {
  return !(
    b.grid_col >= a.grid_col + a.size_x ||
    b.grid_col + b.size_x <= a.grid_col ||
    b.grid_row >= a.grid_row + a.size_y ||
    b.grid_row + b.size_y <= a.grid_row
  );
}

/** First free slot scanning left→right, top→bottom (Metabase getPositionForNewDashCard). */
export function getPositionForNewDashCard(
  cards: GridRect[],
  size_x: number,
  size_y: number,
  width: number = GRID_COLS,
): { grid_col: number; grid_row: number } {
  for (let row = 0; row < 1000; row++) {
    for (let col = 0; col <= width - size_x; col++) {
      const candidate: GridRect = { grid_col: col, grid_row: row, size_x, size_y };
      if (!cards.some((c) => intersects(c, candidate))) return { grid_col: col, grid_row: row };
    }
  }
  return { grid_col: 0, grid_row: 0 };
}

interface CardSize { default: { w: number; h: number }; min: { w: number; h: number } }
const GENERIC_CHART: CardSize = { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } };

/** Per-viz default/min sizes, ported from Metabase card-size-defaults (keyed by egret viz/card keys). */
export const CARD_SIZES_BY_VIZ: Record<string, CardSize> = {
  table:       { default: { w: 12, h: 9 }, min: { w: 4, h: 3 } },
  pivot_table: { default: { w: 12, h: 9 }, min: { w: 4, h: 3 } },
  bar:         { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  row:         { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  line:        { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  area:        { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  scatter:     { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  combo:       { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  pie:         { default: { w: 12, h: 8 }, min: { w: 4, h: 3 } },
  progress:    { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  gauge:       { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  funnel:      { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  waterfall:   { default: { w: 14, h: 6 }, min: { w: 4, h: 3 } },
  sankey:      { default: { w: 16, h: 10 }, min: { w: 4, h: 3 } },
  scalar:      { default: { w: 6, h: 3 }, min: { w: 2, h: 2 } },
  smartscalar: { default: { w: 6, h: 3 }, min: { w: 2, h: 2 } },
  text:        { default: { w: 12, h: 3 }, min: { w: 1, h: 1 } },
  heading:     { default: { w: 24, h: 1 }, min: { w: 1, h: 1 } },
  link:        { default: { w: 8, h: 1 }, min: { w: 1, h: 1 } },
};

/** Default + min grid size for a viz/card key, with a generic-chart fallback (e.g. boxplot). */
export function sizeForViz(key: string | null | undefined): CardSize {
  return (key && CARD_SIZES_BY_VIZ[key]) || GENERIC_CHART;
}
```
Notes: this DROPS `GRID_ASPECT_RATIO`'s old `gridRowHeight` only if it differed — it's unchanged. It REMOVES `ROW_PX`, `ROW_SCALE`, `GRID_MARGIN_X`, `GRID_MARGIN_Y`, `toLayoutRows`, `fromLayoutRows`, `CARD_DEFAULT_SIZE`. Keep `CARD_MIN_SIZE` only if still referenced after Task 4; if Task 4 removes all its uses, delete it too (lint will flag an unused export only if re-exported — it won't; leave it, harmless). Keep `GRID_ASPECT_RATIO` and `GRID_MIN_ROW_HEIGHT` (used by `gridRowHeight`).

- [ ] **Step 4: Run the test — PASS.** (Other files still importing the removed symbols will break typecheck — that's expected; Tasks 2-4 fix them. To keep this task's commit green, do Step 5 only after the build passes; if `npm run build` fails solely due to `DashboardView.tsx`/`AutoHeightMeasure.tsx`/`GridOverlay.tsx` still importing removed symbols, COMBINE Task 1's commit with Tasks 2-4 — see Step 5.)

- [ ] **Step 5: Commit.** Because removing the scale exports breaks `DashboardView.tsx`, `AutoHeightMeasure.tsx`, and `GridOverlay.tsx` imports, the full sweep cannot pass until Tasks 2-4 land. **Do not commit Task 1 alone.** Instead: implement Tasks 1→4 (and the AutoHeightMeasure import in Task 5) as a sequence, run ONE full sweep at the end of Task 4, and commit Tasks 1-4 together with message:
```
feat(dashboards): adopt Metabase grid math (dynamic rowHeight, per-viz sizes, placement)
```
(Each task below still has its own test-first steps; the COMMIT is deferred to the end of Task 4. Tasks 5-6 commit separately.)

---

### Task 2: SVG grid background (replaces `GridOverlay`)

**Files:**
- Create: `frontend/src/components/chart/dashboard/grid-background.ts`
- Delete: `frontend/src/components/chart/dashboard/GridOverlay.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/grid-background.test.ts` (create)

- [ ] **Step 1: Write the failing test** `grid-background.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { generateGridBackground } from "../grid-background";

describe("generateGridBackground", () => {
  const bg = generateGridBackground({ cellWidth: 44, rowHeight: 45, cols: 24, marginX: 6, marginY: 6, stroke: "var(--border)" });
  it("returns a url(data:image/svg+xml,...) value", () => {
    expect(bg.startsWith('url("data:image/svg+xml')).toBe(true);
  });
  it("uses var(--border) (OKLCH-safe), not hsl()", () => {
    expect(bg).toContain("var(--border)");
    expect(bg).not.toContain("hsl(");
  });
  it("draws one rect per column with the cell dimensions", () => {
    const decoded = decodeURIComponent(bg);
    expect((decoded.match(/<rect /g) || []).length).toBe(24);
    expect(decoded).toContain("width='44'");
    expect(decoded).toContain("height='45'");
  });
});
```

- [ ] **Step 2: Run — FAIL** (module missing): `npx vitest run src/components/chart/dashboard/__tests__/grid-background.test.ts`

- [ ] **Step 3: Create `grid-background.ts`:**
```ts
interface GridBackgroundOptions {
  cellWidth: number;
  rowHeight: number;
  cols: number;
  marginX: number;
  marginY: number;
  stroke: string;
}

/** A repeating SVG grid (one cell-rect per column, one row tall) as a CSS background url().
 *  Mirrors Metabase's generateGridBackground. `stroke` should be an OKLCH-safe color such as
 *  `var(--border)` — NOT `hsl(var(--border))`, which double-wraps and renders nothing. */
export function generateGridBackground({ cellWidth, rowHeight, cols, marginX, marginY, stroke }: GridBackgroundOptions): string {
  const w = Math.round(cellWidth);
  const h = Math.round(rowHeight);
  const rects = Array.from({ length: cols }, (_, i) => {
    const x = Math.round(i * (cellWidth + marginX));
    return `<rect stroke='${stroke}' stroke-width='1' fill='none' x='${x}' y='0' width='${w}' height='${h}'/>`;
  }).join("");
  const totalWidth = Math.round(cols * cellWidth + (cols - 1) * marginX);
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='${totalWidth}' height='${h + marginY}'>${rects}</svg>`;
  return `url("data:image/svg+xml;utf8,${encodeURIComponent(svg)}")`;
}
```

- [ ] **Step 4: Run the test — PASS.**

- [ ] **Step 5: Delete `GridOverlay.tsx`** (`git rm frontend/src/components/chart/dashboard/GridOverlay.tsx`). Its only importer is `DashboardView.tsx`, rewired in Task 4. (Commit deferred to end of Task 4 per Task 1 Step 5.)

---

### Task 3: `DashboardGridLayout.tsx` — the grid wrapper

**Files:**
- Create: `frontend/src/components/chart/dashboard/DashboardGridLayout.tsx`
- Test: none new (presentational RGL wrapper; behavior verified live in Task 6). It must typecheck/build.

- [ ] **Step 1: Create `DashboardGridLayout.tsx`:**
```tsx
import { useEffect, useRef, useState } from "react";
import RGL, { type Layout } from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";
import "./dashboard-grid.css";
import { GRID_COLS, GRID_MARGIN, GRID_PADDING, gridRowHeight, cellWidth } from "@/components/chart/dashboard-grid";
import { generateGridBackground } from "./grid-background";

interface Props {
  editing: boolean;
  layout: Layout[];
  onLayoutChange: (next: Layout[]) => void;
  onResizeStart: () => void;
  onResizeStop: () => void;
  onDragStart: () => void;
  onDragStop: () => void;
  /** Render-prop: receives the live rowHeight so cards can convert px↔rows (auto-height). */
  children: (rowHeight: number) => React.ReactNode;
}

/** Metabase-style dashboard grid: dynamic aspect-ratio rowHeight, 6px margins, vertical
 *  compaction, and an SVG cell grid behind the cards while editing. */
export function DashboardGridLayout({ editing, layout, onLayoutChange, onResizeStart, onResizeStop, onDragStart, onDragStop, children }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    setWidth(el.clientWidth);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const rowHeight = gridRowHeight(width);
  const background = editing && width > 0
    ? generateGridBackground({ cellWidth: cellWidth(width), rowHeight, cols: GRID_COLS, marginX: GRID_MARGIN[0], marginY: GRID_MARGIN[1], stroke: "var(--border)" })
    : undefined;

  return (
    <div ref={ref} className="relative">
      {background && <div aria-hidden className="pointer-events-none absolute inset-0 z-0" style={{ backgroundImage: background }} />}
      <RGL
        className="layout relative z-10"
        width={width}
        cols={GRID_COLS}
        rowHeight={rowHeight}
        margin={GRID_MARGIN}
        containerPadding={GRID_PADDING}
        compactType="vertical"
        isDraggable={editing}
        isResizable={editing}
        draggableHandle=".dash-draghandle"
        draggableCancel=".dash-nodrag"
        layout={layout}
        onLayoutChange={onLayoutChange}
        onResizeStart={onResizeStart}
        onResizeStop={onResizeStop}
        onDragStart={onDragStart}
        onDragStop={onDragStop}
      >
        {children(rowHeight)}
      </RGL>
    </div>
  );
}
```
Notes: plain `RGL` (not `WidthProvider`) because we supply `width` from our own measurement (needed to compute rowHeight). `background` is set as `backgroundImage` (the helper returns a full `url(...)`). The render-prop passes `rowHeight` to the children factory.

- [ ] **Step 2:** This file won't build until `dashboard-grid.css` exists (Task 5 creates it). Create a placeholder now so Task 3/4 build: `echo "" > frontend/src/components/chart/dashboard/dashboard-grid.css` (Task 5 fills it). (Commit deferred to end of Task 4.)

---

### Task 4: `DashboardView.tsx` — use the wrapper, raw-unit layout, per-viz sizing

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx`

- [ ] **Step 1: Imports.** Replace the grid import (line ~19) and the RGL import (line ~4). Change:
```tsx
import RGL, { WidthProvider, type Layout } from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";
```
to:
```tsx
import { type Layout } from "react-grid-layout";
```
Change the dashboard-grid import (line ~19) to:
```tsx
import { GRID_COLS, sizeForViz, getPositionForNewDashCard } from "@/components/chart/dashboard-grid";
```
Remove the `import { GridOverlay } …` line. Add:
```tsx
import { DashboardGridLayout } from "@/components/chart/dashboard/DashboardGridLayout";
```
Delete `const Grid = WidthProvider(RGL);` and `const ROW_PX = …` (already gone). `chartById` is available for viz lookup.

- [ ] **Step 2: per-viz new-card sizing + placement.** Replace `addCard` (lines ~140-148):
```tsx
  function addCard(chart: Chart) {
    const { w, h } = sizeForViz(chart.viz_type).default;
    const pos = getPositionForNewDashCard(cards, w, h);
    const newCard: DraftCard = {
      id: `new-${chart.id}-${cardSeqRef.current++}`, chart_id: chart.id, card_type: "chart",
      grid_col: pos.grid_col, grid_row: pos.grid_row, size_x: w, size_y: h, options: {}, tab_id: null,
      tabKey: activeTab?.key ?? null,
    };
    setDraftCards([...(cards as DraftCard[]), newCard]);
  }
```
Replace `addVirtualCard` (lines ~167-178):
```tsx
  function addVirtualCard(card_type: "text" | "heading" | "link", options: Record<string, unknown>) {
    setDraftCards((prev) => {
      const base = prev ?? cardsAsDraft();
      const { w, h } = sizeForViz(card_type).default;
      const pos = getPositionForNewDashCard(base, w, h);
      const newCard: DraftCard = {
        id: `new-${card_type}-${cardSeqRef.current++}`, chart_id: null, card_type,
        grid_col: pos.grid_col, grid_row: pos.grid_row, size_x: w, size_y: h, options, tab_id: null,
        tabKey: activeTab?.key ?? null,
      };
      return [...base, newCard];
    });
  }
```
(Leave `duplicateCard`'s `maxY` bottom-placement as-is — duplicating below the source is fine, and Metabase compaction will tidy it.)

- [ ] **Step 3: raw-unit layout map.** Replace the `layout` map (lines ~189-201) with per-viz mins and no scaling. Add a small helper just above it:
```tsx
  const vizKeyOf = (c: DraftCard) => (c.card_type === "chart" ? (chartById.get(c.chart_id ?? "")?.viz_type ?? "") : c.card_type);
  const layout: Layout[] = visibleCards.map((c) => {
    const autoH = !!c.options.autoHeight;
    const min = sizeForViz(vizKeyOf(c as DraftCard)).min;
    return {
      i: c.id, x: c.grid_col, y: c.grid_row, w: c.size_x, h: c.size_y,
      // clamp mins to the card's own size so RGL never warns (minW/minH > current).
      minW: Math.min(min.w, c.size_x),
      minH: autoH ? c.size_y : Math.min(min.h, c.size_y),
      ...(autoH ? { maxH: c.size_y } : {}),
    };
  });
```

- [ ] **Step 4: raw-unit `onLayoutChange`.** Replace the per-card mapping (lines ~222-230) — drop `fromLayoutRows`:
```tsx
      const merged = base.map((c) => {
        const l = next.find((x) => x.i === c.id);
        if (!l) return c;
        const row = Math.max(0, l.y);
        if (l.x === c.grid_col && row === c.grid_row && l.w === c.size_x && l.h === c.size_y) return c;
        changed = true;
        return { ...c, grid_col: l.x, grid_row: row, size_x: l.w, size_y: l.h };
      });
```

- [ ] **Step 5: render the wrapper.** Read the current grid render branch (the `) : (` … `<div className="relative">{editing && <GridOverlay …/>}<Grid …>{visibleCards.map(...)}</Grid></div>` … `)}`). Replace it with the render-prop wrapper, passing `rowHeight` into each card's `DashboardCardView`:
```tsx
      ) : (
        <DashboardGridLayout
          editing={editing}
          layout={layout}
          onLayoutChange={onLayoutChange}
          onResizeStart={() => { interactingRef.current = true; }}
          onResizeStop={() => { interactingRef.current = false; }}
          onDragStart={() => { interactingRef.current = true; }}
          onDragStop={() => { interactingRef.current = false; }}
        >
          {(rowHeight) => visibleCards.map((c) => {
            const baseChart = chartById.get(c.chart_id ?? "");
            const mappings = (c.options.paramMappings as Record<string, string>) ?? {};
            const injected = baseChart ? applyParamsToChart(baseChart, parameters, paramsState.values, mappings) : undefined;
            return (
            <div key={c.id} className="group relative h-full">
              {/* …KEEP the existing card body EXACTLY as today (CardEditToolbar wrapper, the
                  draghandle div, the inner rounded card div, CardFilterMapOverlay)… */}
              {/* On the <DashboardCardView …/> add: rowHeight={rowHeight} */}
            </div>
            );
          })}
        </DashboardGridLayout>
      )}
```
Concretely: take the existing `{visibleCards.map((c) => { … })}` body verbatim, wrap it in `{(rowHeight) => …}`, and on the existing `<DashboardCardView ws={ws} card={c} … autoHeight={!!c.options.autoHeight} onAutoHeight={…}/>` add the prop `rowHeight={rowHeight}`. Do not change anything else in the card body.

- [ ] **Step 6: Full sweep (Tasks 1-4) + the combined commit.**
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard-grid.ts src/components/chart/__tests__/dashboard-grid.test.ts \
  src/components/chart/dashboard/grid-background.ts src/components/chart/dashboard/__tests__/grid-background.test.ts \
  src/components/chart/dashboard/DashboardGridLayout.tsx src/components/chart/dashboard/dashboard-grid.css \
  src/routes/explore/DashboardView.tsx
git rm src/components/chart/dashboard/GridOverlay.tsx
git commit -m "feat(dashboards): adopt Metabase grid math (dynamic rowHeight, per-viz sizes, placement)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
(If `AutoHeightMeasure.tsx` still calls `contentRowsFor(px)` with one arg, typecheck fails here — Task 5 Step 1 is a prerequisite for the sweep. Do Task 5 Step 1 BEFORE running this sweep, and include AutoHeightMeasure/DashboardCardView in this same commit, or split: see Task 5.)

---

### Task 5: Auto-height on dynamic rowHeight + resize-handle CSS

**Files:**
- Modify: `frontend/src/components/chart/dashboard/AutoHeightMeasure.tsx`
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`
- Modify: `frontend/src/components/chart/dashboard/dashboard-grid.css`

> Sequencing: `contentRowsFor` is now 2-arg, so AutoHeightMeasure MUST be updated before the Task 4 sweep passes. Do Task 5 Steps 1-2 alongside Task 4, then run the single combined sweep + commit at Task 4 Step 6 (add the three files below to that `git add`). Task 5 Step 3 (CSS) can be its own follow-up commit.

- [ ] **Step 1: `AutoHeightMeasure.tsx`** — take a `rowHeight` prop:
```tsx
import { useEffect, useRef } from "react";
import { contentRowsFor } from "@/components/chart/dashboard-grid";

/** Measures its content's natural height and reports the stored-row count when it changes by ≥1 row. */
export function AutoHeightMeasure({ rowHeight, onRows, children }: { rowHeight: number; onRows: (rows: number) => void; children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const last = useRef(0);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const measure = () => {
      const px = el.scrollHeight;
      if (px <= 0) return;
      const rows = contentRowsFor(px, rowHeight);
      if (rows !== last.current) { last.current = rows; onRows(rows); }
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, [onRows, rowHeight]);
  return <div ref={ref}>{children}</div>;
}
```

- [ ] **Step 2: `DashboardCardView.tsx`** — thread `rowHeight` to the measurer. Add `rowHeight?: number` to both `ChartCardTile` and `DashboardCardView` prop types. In the two places that render `<AutoHeightMeasure onRows={onAutoHeight}>…</AutoHeightMeasure>` (the `wrap` helper in `DashboardCardView` and the body wrap in `ChartCardTile`), pass `rowHeight={rowHeight ?? 0}`. Thread `rowHeight` from `DashboardCardView` into `<ChartCardTile … rowHeight={rowHeight} />`. (The Task 4 render-prop already passes `rowHeight={rowHeight}` to `<DashboardCardView>`.)

- [ ] **Step 3: `dashboard-grid.css`** — Metabase resize-handle look (the handle only renders in edit mode, so no edit-scoping needed):
```css
/* Metabase-style resize handle: subtle, hover-revealed, SE corner only. */
.react-grid-item > .react-resizable-handle {
  opacity: 0;
  transition: opacity 0.15s linear;
}
.react-grid-item:hover > .react-resizable-handle {
  opacity: 1;
}
.react-grid-item > .react-resizable-handle.react-resizable-handle-se {
  cursor: nwse-resize;
}
/* The dragging/resizing card floats above its neighbours. */
.react-grid-item.react-draggable-dragging,
.react-grid-item.resizing {
  z-index: 3;
}
```
Commit (separate, after the combined Task 1-4+5.1-5.2 commit):
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
git add src/components/chart/dashboard/dashboard-grid.css
git commit -m "style(dashboards): Metabase-style hover-revealed SE resize handle

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Live verification + finalize PR #39

**Files:** none (verification only).

- [ ] **Step 1: Final sweep** (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run` → green.

- [ ] **Step 2: Run the stack** (frontend-only; `npm run dev`), log in `admin@egret.local` / `Admin12345!`, eng workspace. **Do NOT touch "a1".** Create a NEW throwaway dashboard via "+ Dashboard"; add a Table, a Bar, and a Scalar chart in edit mode. Keep the Metabase tab (localhost:3001) open for side-by-side comparison. Delete the throwaway at the end.

- [ ] **Step 3: Live-verify (Playwright MCP, 0 console errors):**
1. **Dynamic rowHeight:** measure the grid container width `W`; assert the rendered `rowHeight ≈ max(floor(W/24/(10/9)), 40)` (e.g. ~45px at ~1100-1200px). A chart card of grid size 12×6 has the same width:height proportions as the equivalent Metabase card.
2. **Margins:** the gap between adjacent cards is ~6px (not 12).
3. **Compaction:** drag a card up into a gap and release — other cards compact vertically (no floating holes); re-loading the dashboard keeps it packed.
4. **SVG grid:** in edit mode a faint cell grid shows behind the cards and its cells align to card edges; it disappears in view mode.
5. **Resize handle:** only the SE corner handle appears, revealed on card hover; dragging it resizes; min sizes hold (a table can't shrink below 4×3).
6. **Per-viz sizing:** a newly added **Table** card is 12×9, a **Scalar** is 6×3, a **Bar** is 12×6 (read back `size_x`/`size_y` or pixel proportions).
7. **Auto-height:** enabling Auto height on a table fits its content and locks vertical resize, now computed against the dynamic rowHeight (no clipping/overshoot).
8. **Existing dashboard:** open PARITY **read-only** (do NOT save) — it re-flows (shorter cards, packed) but renders coherently, no overlap, 0 errors.

- [ ] **Step 4: Clean up & finalize the PR.** Delete the throwaway dashboard. Then push the branch (PR #39 already exists):
```bash
git push
```
Update the PR #39 description to note the grid was reworked to Metabase's algorithm (dynamic rowHeight, vertical compaction, per-viz sizes, SVG grid, SE handle) replacing the earlier ×3 render-scale. Re-run the flaky `egret-ranger` rust check if it fails. Squash-merge when CI is green.

---

## Self-Review

**Spec coverage:**
- Metabase constants/`gridRowHeight`/`cellWidth`/`contentRowsFor(px,rh)`/`getPositionForNewDashCard`/`CARD_SIZES_BY_VIZ`/`sizeForViz` + removed scale helpers → Task 1. ✓
- `generateGridBackground` (OKLCH-safe) + delete GridOverlay → Task 2. ✓
- `DashboardGridLayout` (dynamic rowHeight, margins, compactType vertical, SE handle, SVG bg, render-prop rowHeight) → Task 3. ✓
- DashboardView: wrapper, raw-unit layout/onLayoutChange, per-viz sizing+placement, rowHeight to cards → Task 4. ✓
- Auto-height on dynamic rowHeight + resize-handle CSS → Task 5. ✓
- Backward-compat re-flow accepted; live PARITY check → Task 6. ✓
- Mobile responsive explicitly deferred (not in any task) — matches the spec. ✓

**Type/signature consistency:**
- `contentRowsFor(px, rowHeight)` — 2-arg, defined Task 1, consumed in AutoHeightMeasure (Task 5).
- `sizeForViz(key): {default:{w,h}, min:{w,h}}` — Task 1, used in DashboardView layout map + addCard/addVirtualCard (Task 4).
- `getPositionForNewDashCard(cards, w, h): {grid_col, grid_row}` — Task 1, used in addCard/addVirtualCard (Task 4).
- `generateGridBackground({cellWidth,rowHeight,cols,marginX,marginY,stroke})` — Task 2, used in DashboardGridLayout (Task 3).
- `DashboardGridLayout` render-prop `children: (rowHeight)=>ReactNode` — Task 3, supplied by DashboardView (Task 4); `rowHeight` flows to `DashboardCardView` → `AutoHeightMeasure` (Task 5).
- `GRID_MARGIN`/`GRID_PADDING` `[number,number]` — Task 1, used Task 3.

**Placeholder scan:** No TBD/TODO. The one verbatim-reuse instruction (Task 4 Step 5 card body) explicitly says "keep the existing JSX exactly, add `rowHeight={rowHeight}`" rather than re-pasting the large unchanged block — acceptable (it's a keep-as-is, not an unspecified gap).

**Sequencing note (important):** removing the scale exports (Task 1) breaks `DashboardView`/`AutoHeightMeasure`/`GridOverlay` imports, so the full sweep can only pass once Tasks 2, 3, 4, and 5.1-5.2 are all applied. Implement Tasks 1→4 + 5.1-5.2 as one unit with a SINGLE sweep + commit at Task 4 Step 6; CSS (5.3) and live verify (6) are separate commits. A subagent executing "Task 1" must be told to proceed through Task 4 before committing.
