# W1 — Responsive Dashboard Grid Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the dashboard readable on a phone — one column, no horizontal scroll — without
ever letting a mobile visit rewrite the saved desktop layout.

**Architecture:** Metabase does not store two layouts. It keeps one desktop layout and *derives*
the mobile one at render time, then refuses to persist any layout change made while not on the
desktop breakpoint. We port exactly that. egret's `DashboardGridLayout` already measures its own
container with a `ResizeObserver`, so the breakpoint is a pure function of a width it already
has; no new measurement machinery is needed. Separately, the app shell learns a viewport rule so
the 264px navigation sidebar stops eating the canvas on a phone.

**Tech Stack:** react-grid-layout (plain `RGL`, not `Responsive` — we compute `cols`/`layout`
ourselves, as Metabase's own `GridLayout` wrapper does), React 18, Tailwind v4, vitest +
Testing Library.

**Design reference:** `docs/en/specs/2026-08-09-dashboard-parity-design.md` §2.1 and §4 W1.

**Branch:** `feat/dashboard-parity-01-responsive-grid`, from `main`.

---

## Background the implementer needs

**Why not `RGL.Responsive`.** react-grid-layout's `Responsive` component wants a `layouts` object
keyed by breakpoint and writes back per-breakpoint layouts. egret's cards are the single source
of truth for positions (`DashboardView.tsx:338-352` derives the RGL `layout` array from
`visibleCards`, and drag/resize writes straight back into the draft cards). Adopting `Responsive`
would mean either storing a second layout per card or fighting the library's write-back. Metabase
avoids this the same way we will: keep one layout, compute the mobile projection at render.

**The guard that matters most.** `DashboardGrid.tsx:299-301` in Metabase ignores layout changes
unless editing **and** on the desktop breakpoint. Without the second half, react-grid-layout emits
an `onLayoutChange` for the derived one-column layout as soon as it mounts on a phone; egret's
`onLayoutChange` (`DashboardView.tsx:368-383`) would write `x: 0, w: 1` into every draft card. If
the user then saved, the desktop layout would be gone. This is the single highest-risk line in
the workstream.

**Auto-height has the same hazard.** `applyAutoHeight` (`DashboardView.tsx:354-366`) writes
`size_y` from a `ResizeObserver` measurement. On mobile the cards are a different width, so the
measured content height is different, so it would rewrite `size_y` for the desktop layout. It
must be inert off the desktop breakpoint, for the same reason.

**Where the 86px came from.** Two independent causes, and fixing only one leaves the dashboard
broken. `AppShell.tsx:29-31` renders `<Sidebar>` and `<main>` as flex siblings; the sidebar has
no viewport rule, so at 390px it still claims 264px and `<main>` gets 86px after padding. The
grid then divides that 86px into 24 columns.

---

## File structure

| Path | Responsibility after this workstream |
|---|---|
| `frontend/src/components/chart/dashboard/grid-breakpoints.ts` | **New.** Breakpoint constants, `breakpointFor(width)`, `toMobileLayout(...)`, `mobileHeightFor(...)`. Pure — no React, no DOM. |
| `frontend/src/lib/useIsNarrowViewport.ts` | **New.** One `matchMedia` hook returning whether the viewport is below the shell's breakpoint. |
| `frontend/src/components/chart/dashboard/DashboardGridLayout.tsx` | Gains breakpoint awareness: picks `cols`, `margin`, `layout`, and the draggable/resizable flags, and tells its parent which breakpoint is live. |
| `frontend/src/components/layout/AppShell.tsx` | Sidebar collapses below the viewport breakpoint; the localStorage preference continues to govern above it. |
| `frontend/src/routes/explore/DashboardView.tsx` | Wiring only: refuses to persist layout and auto-height changes off the desktop breakpoint. |

---

## Task 1: Breakpoint constants and the mobile projection

**Files:**
- Create: `frontend/src/components/chart/dashboard/grid-breakpoints.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/grid-breakpoints.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/grid-breakpoints.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import {
  GRID_BREAKPOINTS,
  GRID_COLUMNS,
  MOBILE_DEFAULT_CARD_HEIGHT,
  breakpointFor,
  mobileHeightFor,
  toMobileLayout,
} from "../grid-breakpoints";

describe("breakpointFor", () => {
  it("is mobile at and below 752 and desktop at 753", () => {
    // Ported from Metabase utils/dashboard_grid.ts:11-16 — the two numbers are adjacent
    // on purpose, so this pins the boundary rather than a value near it.
    expect(breakpointFor(752)).toBe("mobile");
    expect(breakpointFor(GRID_BREAKPOINTS.desktop)).toBe("desktop");
    expect(breakpointFor(0)).toBe("mobile");
  });
});

describe("mobileHeightFor", () => {
  it("uses the per-kind heights, and text keeps its desktop height when taller", () => {
    expect(mobileHeightFor("link", 1)).toBe(1);
    expect(mobileHeightFor("heading", 1)).toBe(2);
    expect(mobileHeightFor("scalar", 3)).toBe(4);
    expect(mobileHeightFor("smartscalar", 3)).toBe(4);
    expect(mobileHeightFor("text", 1)).toBe(2);
    expect(mobileHeightFor("text", 7)).toBe(7);
    expect(mobileHeightFor("bar", 6)).toBe(MOBILE_DEFAULT_CARD_HEIGHT);
    expect(mobileHeightFor("anything-unknown", 99)).toBe(MOBILE_DEFAULT_CARD_HEIGHT);
  });
});

describe("toMobileLayout", () => {
  const desktop = [
    { i: "b", x: 12, y: 0, w: 12, h: 6 },
    { i: "a", x: 0, y: 0, w: 12, h: 6 },
    { i: "c", x: 0, y: 6, w: 24, h: 3 },
  ];
  const kinds = { a: "bar", b: "scalar", c: "heading" };

  it("stacks every card at x:0 w:1 in reading order, not array order", () => {
    const out = toMobileLayout(desktop, (id) => kinds[id as keyof typeof kinds]);
    expect(out.map((l) => l.i)).toEqual(["a", "b", "c"]);
    expect(out.every((l) => l.x === 0 && l.w === 1)).toBe(true);
  });

  it("assigns heights by kind and stacks y with no gaps", () => {
    const out = toMobileLayout(desktop, (id) => kinds[id as keyof typeof kinds]);
    expect(out.map((l) => [l.h, l.y])).toEqual([[6, 0], [4, 6], [2, 10]]);
  });

  it("never emits minW/maxH that would pin a card wider than the single column", () => {
    // The desktop layout carries minW from per-viz minimums (DashboardView.tsx:342-352).
    // Carrying those into a 1-column grid makes RGL warn and clamp every card back to
    // multi-column widths, which is exactly the bug this projection exists to prevent.
    const withMins = [{ i: "a", x: 0, y: 0, w: 12, h: 6, minW: 4, minH: 3, maxH: 6 }];
    const out = toMobileLayout(withMins, () => "bar");
    expect(out[0].minW).toBe(1);
    expect(out[0].maxH).toBeUndefined();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/grid-breakpoints.test.ts`

Expected: FAIL — `Failed to resolve import "../grid-breakpoints"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/grid-breakpoints.ts`:

```ts
import type { Layout } from "react-grid-layout";

/** Ported from Metabase utils/dashboard_grid.ts:11-26 and grid/utils.ts:21-36.
 *  The two breakpoint numbers are adjacent (752 / 753) because react-grid-layout picks the
 *  largest breakpoint whose value is <= the container width; a gap between them would leave
 *  widths that match neither. */
export const GRID_BREAKPOINTS = { desktop: 753, mobile: 752 } as const;
export const GRID_COLUMNS = { desktop: 24, mobile: 1 } as const;
export const GRID_MARGIN_BY_BREAKPOINT = {
  desktop: [6, 6] as [number, number],
  mobile: [6, 10] as [number, number],
};
export const MOBILE_DEFAULT_CARD_HEIGHT = 6;

export type GridBreakpoint = keyof typeof GRID_COLUMNS;

/** Which breakpoint a container of `width` px is in. Container width, NOT viewport width —
 *  the dashboard canvas is narrowed by the app sidebar and by the edit sidebars, and it is the
 *  canvas that has to fit the cards. */
export function breakpointFor(width: number): GridBreakpoint {
  return width >= GRID_BREAKPOINTS.desktop ? "desktop" : "mobile";
}

/** Mobile row count for a card, keyed on its viz type (chart cards) or card type
 *  (text/heading/link). Mirrors MOBILE_HEIGHT_BY_DISPLAY_TYPE in
 *  visualizations/shared/utils/sizes.ts:36-58. */
export function mobileHeightFor(kind: string, desktopHeight: number): number {
  switch (kind) {
    case "link":
      return 1;
    case "heading":
      return 2;
    case "scalar":
    case "smartscalar":
      return 4;
    // A text card is the one kind whose desktop height is authored content rather than a
    // chart's aspect, so shrinking it would clip what the author wrote.
    case "text":
      return Math.max(2, desktopHeight);
    default:
      return MOBILE_DEFAULT_CARD_HEIGHT;
  }
}

/** Project a desktop layout onto the one-column mobile grid: every card full width, stacked in
 *  reading order (top-to-bottom, then left-to-right), heights by kind.
 *
 *  `kindOf` maps a layout id to the string `mobileHeightFor` keys on. The min/max constraints
 *  are deliberately dropped: they were computed for a 24-column grid and would make RGL clamp
 *  every card back above one column. */
export function toMobileLayout(layout: Layout[], kindOf: (id: string) => string): Layout[] {
  const ordered = [...layout].sort((a, b) => (a.y - b.y) || (a.x - b.x));
  let y = 0;
  return ordered.map((l) => {
    const h = mobileHeightFor(kindOf(l.i), l.h);
    const next: Layout = { i: l.i, x: 0, y, w: 1, h, minW: 1, minH: 1 };
    y += h;
    return next;
  });
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/grid-breakpoints.test.ts`

Expected: PASS, 5 tests.

- [ ] **Step 5: Prove the tests survive mutation**

Change `breakpointFor` to `return "desktop";` and re-run. Expected: FAIL. Change
`toMobileLayout`'s sort to `[...layout]` with no sort and re-run. Expected: FAIL on the
reading-order test. Restore both.

This step is not optional. Six assertions in the chart programme passed against a stubbed
implementation; the only thing that caught them was doing exactly this.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/grid-breakpoints.ts \
        frontend/src/components/chart/dashboard/__tests__/grid-breakpoints.test.ts
git commit -m "feat(dashboard): breakpoint constants and the desktop-to-mobile layout projection"
```

---

## Task 2: The grid renders at its breakpoint

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardGridLayout.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardGridLayout.responsive.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/DashboardGridLayout.responsive.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { DashboardGridLayout } from "../DashboardGridLayout";

// jsdom gives every element clientWidth 0, so the component would always see width 0 and
// always render mobile. Stub the measurement instead of the component under test.
function withContainerWidth(px: number) {
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get() { return px; },
  });
}

const layout = [
  { i: "a", x: 0, y: 0, w: 12, h: 6 },
  { i: "b", x: 12, y: 0, w: 12, h: 6 },
];

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", class {
    observe() {} unobserve() {} disconnect() {}
  });
});

describe("DashboardGridLayout breakpoint", () => {
  it("reports desktop and keeps 24 columns at 1136px", () => {
    withContainerWidth(1136);
    const onBreakpoint = vi.fn();
    render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        onBreakpointChange={onBreakpoint}
        onLayoutChange={() => {}}
        onResizeStart={() => {}} onResizeStop={() => {}}
        onDragStart={() => {}} onDragStop={() => {}}
      >
        {() => <div key="a" data-testid="card-a" />}
      </DashboardGridLayout>,
    );
    expect(onBreakpoint).toHaveBeenCalledWith("desktop");
  });

  it("reports mobile at 390px", () => {
    withContainerWidth(390);
    const onBreakpoint = vi.fn();
    render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        onBreakpointChange={onBreakpoint}
        onLayoutChange={() => {}}
        onResizeStart={() => {}} onResizeStop={() => {}}
        onDragStart={() => {}} onDragStop={() => {}}
      >
        {() => <div key="a" data-testid="card-a" />}
      </DashboardGridLayout>,
    );
    expect(onBreakpoint).toHaveBeenCalledWith("mobile");
  });

  it("draws no edit-mode grid background on mobile", () => {
    // The guide grid describes a 24-column canvas. On a one-column grid it describes nothing
    // real, and it is the loudest thing on a small screen.
    withContainerWidth(390);
    const { container } = render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        onBreakpointChange={() => {}}
        onLayoutChange={() => {}}
        onResizeStart={() => {}} onResizeStop={() => {}}
        onDragStart={() => {}} onDragStop={() => {}}
      >
        {() => <div key="a" data-testid="card-a" />}
      </DashboardGridLayout>,
    );
    expect(container.querySelector("[aria-hidden].pointer-events-none")).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardGridLayout.responsive.test.tsx`

Expected: FAIL — `DashboardGridLayout` has no `kindOf` or `onBreakpointChange` prop, so TypeScript
errors and the `onBreakpoint` expectations never fire.

- [ ] **Step 3: Write the implementation**

Replace the body of `frontend/src/components/chart/dashboard/DashboardGridLayout.tsx` with:

```tsx
import { useEffect, useRef, useState } from "react";
import RGL, { type Layout } from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";
import "./dashboard-grid.css";
import { GRID_PADDING, gridRowHeight, cellWidth, GRID_COLS, GRID_MARGIN } from "@/components/chart/dashboard-grid";
import {
  GRID_COLUMNS,
  GRID_MARGIN_BY_BREAKPOINT,
  breakpointFor,
  toMobileLayout,
  type GridBreakpoint,
} from "./grid-breakpoints";
import { generateGridBackground } from "./grid-background";

interface Props {
  editing: boolean;
  layout: Layout[];
  /** Maps a card id to the string the mobile height table keys on — a chart card's viz type,
   *  or the card type for text/heading/link. Supplied by the route, which is the only place
   *  that knows the chart behind a card. */
  kindOf: (id: string) => string;
  /** Fires whenever the live breakpoint changes. The route needs this to refuse to persist
   *  layout and auto-height changes off the desktop breakpoint. */
  onBreakpointChange: (bp: GridBreakpoint) => void;
  onLayoutChange: (next: Layout[]) => void;
  onResizeStart: () => void;
  onResizeStop: () => void;
  onDragStart: () => void;
  onDragStop: () => void;
  children: (rowHeight: number) => React.ReactNode;
}

/** Metabase-style dashboard grid: 24 columns on desktop, one column below 753px of CONTAINER
 *  width, dynamic aspect-ratio rowHeight, vertical compaction, and an SVG cell grid behind the
 *  cards while editing on desktop. */
export function DashboardGridLayout({
  editing, layout, kindOf, onBreakpointChange, onLayoutChange,
  onResizeStart, onResizeStop, onDragStart, onDragStop, children,
}: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [stroke, setStroke] = useState("oklch(0.8 0 0)");

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    setWidth(el.clientWidth);
    const probe = document.createElement("div");
    probe.style.color = "color-mix(in oklch, var(--border) 80%, var(--foreground))";
    el.appendChild(probe);
    const mixed = getComputedStyle(probe).color;
    probe.remove();
    const border = getComputedStyle(document.documentElement).getPropertyValue("--border").trim();
    if (mixed) setStroke(mixed);
    else if (border) setStroke(border);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, [editing]);

  const breakpoint = breakpointFor(width);
  const isMobile = breakpoint === "mobile";

  // Tell the route on every change, and on first measurement. The route uses it as a guard,
  // so a missed notification means a mobile layout can be persisted.
  useEffect(() => { onBreakpointChange(breakpoint); }, [breakpoint, onBreakpointChange]);

  const cols = GRID_COLUMNS[breakpoint];
  const margin = GRID_MARGIN_BY_BREAKPOINT[breakpoint];
  const effectiveLayout = isMobile ? toMobileLayout(layout, kindOf) : layout;
  const rowHeight = gridRowHeight(width);

  // The guide grid describes a 24-column canvas; on one column it describes nothing, and it is
  // the loudest element on a small screen. Desktop-and-editing only.
  const background = editing && !isMobile && width > 0
    ? generateGridBackground({
        cellWidth: cellWidth(width), rowHeight, cols: GRID_COLS,
        marginX: GRID_MARGIN[0], marginY: GRID_MARGIN[1], stroke,
      })
    : undefined;

  return (
    <div ref={ref} className="relative">
      {background && (
        <div aria-hidden className="pointer-events-none absolute inset-0 z-0" style={{ backgroundImage: background }} />
      )}
      <RGL
        className="layout relative z-10"
        width={width}
        cols={cols}
        rowHeight={rowHeight}
        margin={margin}
        containerPadding={GRID_PADDING}
        compactType="vertical"
        isDraggable={editing && !isMobile}
        isResizable={editing && !isMobile}
        draggableHandle=".dash-draghandle"
        draggableCancel=".dash-nodrag"
        layout={effectiveLayout}
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

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardGridLayout.responsive.test.tsx`

Expected: PASS, 3 tests.

- [ ] **Step 5: Run the existing grid tests to check nothing regressed**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/`

Expected: PASS. If `grid-background.test.ts` or any dashboard test fails, the cause is the new
`kindOf`/`onBreakpointChange` props being required — add them at the call sites in the test
fixtures rather than making the props optional. Optional props would let a future caller forget
the guard silently, which is the failure mode this whole workstream exists to prevent.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/DashboardGridLayout.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardGridLayout.responsive.test.tsx
git commit -m "feat(dashboard): render the grid at its container breakpoint, one column below 753px"
```

---

## Task 3: The route refuses to persist a mobile layout

**Files:**
- Modify: `frontend/src/routes/explore/DashboardView.tsx:354-383` (`applyAutoHeight`, `onLayoutChange`) and the `<DashboardGridLayout>` call site
- Test: `frontend/src/components/chart/dashboard/__tests__/persistGuard.test.ts`

This is the highest-risk task in the workstream. Read "The guard that matters most" above before
starting.

- [ ] **Step 1: Extract the guard so it can be tested without rendering the route**

`DashboardView.tsx` is 769 lines and seven workstreams touch it. Rather than testing the guard
through the whole route, extract the predicate. Add to
`frontend/src/components/chart/dashboard/grid-breakpoints.ts`:

```ts
/** May a layout or auto-height change be written into the draft?
 *
 *  Both halves are load-bearing. `editing` alone is not enough: react-grid-layout emits an
 *  onLayoutChange for the DERIVED one-column layout as soon as it mounts on a narrow screen,
 *  so an editing user on a phone would silently rewrite every card to x:0 w:1 and a Save would
 *  destroy the desktop layout. */
export function mayPersistLayout(editing: boolean, breakpoint: GridBreakpoint): boolean {
  return editing && breakpoint === "desktop";
}
```

- [ ] **Step 2: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/persistGuard.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { mayPersistLayout } from "../grid-breakpoints";

describe("mayPersistLayout", () => {
  it("permits only editing on desktop", () => {
    expect(mayPersistLayout(true, "desktop")).toBe(true);
    expect(mayPersistLayout(true, "mobile")).toBe(false);
    expect(mayPersistLayout(false, "desktop")).toBe(false);
    expect(mayPersistLayout(false, "mobile")).toBe(false);
  });
});
```

- [ ] **Step 3: Run the test to verify it fails, then passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/persistGuard.test.ts`

Expected before Step 1's code is added: FAIL, `mayPersistLayout is not a function`. After: PASS.

- [ ] **Step 4: Wire the guard into the route**

In `frontend/src/routes/explore/DashboardView.tsx`:

Add the breakpoint state near the other view state, and import the helpers:

```tsx
import { mayPersistLayout, type GridBreakpoint } from "@/components/chart/dashboard/grid-breakpoints";
```

```tsx
const [gridBreakpoint, setGridBreakpoint] = useState<GridBreakpoint>("desktop");
```

Change `applyAutoHeight` (currently at `:354-366`) so its first line reads:

```tsx
  function applyAutoHeight(cardId: string, rows: number) {
    if (interactingRef.current) return;            // never fight an active drag/resize
    // A mobile card is a different width, so its measured content height belongs to a layout
    // we are not editing. Writing it back would corrupt the desktop rows.
    if (!mayPersistLayout(editing, gridBreakpoint)) return;
```

Change `onLayoutChange` (currently at `:368-383`) so its first line reads:

```tsx
  function onLayoutChange(next: Layout[]) {
    if (!mayPersistLayout(editing, gridBreakpoint)) return;
```

(That replaces the existing `if (!editing) return;`.)

At the `<DashboardGridLayout>` call site, pass the two new props:

```tsx
        kindOf={(id) => {
          const c = (cards as DraftCard[]).find((x) => x.id === id);
          return c ? vizKeyOf(c) : "";
        }}
        onBreakpointChange={setGridBreakpoint}
```

`vizKeyOf` already exists at `:341` and returns the chart's `viz_type` for chart cards and the
`card_type` for the rest — exactly the key `mobileHeightFor` expects.

- [ ] **Step 5: Run the full frontend suite**

Run each as its own command. Do not chain them, and do not pipe into `rtk` — a swallowed pipe
breaks the `&&` chain behind it and the later gates silently never run.

```bash
cd frontend && npm run lint
cd frontend && npx tsc --noEmit
cd frontend && npm run test
```

Expected: all green. Record the test count.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/explore/DashboardView.tsx \
        frontend/src/components/chart/dashboard/grid-breakpoints.ts \
        frontend/src/components/chart/dashboard/__tests__/persistGuard.test.ts
git commit -m "fix(dashboard): never persist a layout or auto-height change off the desktop breakpoint"
```

---

## Task 4: The app shell yields its sidebar on a narrow viewport

**Files:**
- Create: `frontend/src/lib/useIsNarrowViewport.ts`
- Modify: `frontend/src/components/layout/AppShell.tsx`
- Test: `frontend/src/components/layout/__tests__/AppShell.responsive.test.tsx`

- [ ] **Step 1: Write the hook**

Create `frontend/src/lib/useIsNarrowViewport.ts`:

```ts
import { useEffect, useState } from "react";

/** Viewport width below which the navigation sidebar must not consume layout width.
 *  Deliberately NOT the grid's 753px breakpoint: that one measures the dashboard CONTAINER,
 *  this one measures the window. Collapsing them into one number would make the sidebar's
 *  behaviour depend on which route is mounted. */
export const NARROW_VIEWPORT_PX = 768;

const QUERY = `(max-width: ${NARROW_VIEWPORT_PX - 1}px)`;

export function useIsNarrowViewport(): boolean {
  const [narrow, setNarrow] = useState(() =>
    typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia(QUERY).matches
      : false,
  );
  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia(QUERY);
    const onChange = (e: MediaQueryListEvent) => setNarrow(e.matches);
    setNarrow(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
  return narrow;
}
```

- [ ] **Step 2: Write the failing test**

Create `frontend/src/components/layout/__tests__/AppShell.responsive.test.tsx`:

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { AppShell } from "../AppShell";

function stubMatchMedia(matches: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches, media: query, onchange: null,
    addEventListener: () => {}, removeEventListener: () => {},
    addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false,
  }));
}

beforeEach(() => {
  localStorage.clear();
});

describe("AppShell sidebar on a narrow viewport", () => {
  it("collapses the sidebar below the breakpoint even when localStorage says expanded", () => {
    // The stored preference is a DESKTOP preference. Honouring it at 390px is what leaves the
    // dashboard canvas 86px wide.
    localStorage.setItem("egret:sidebar-collapsed", "0");
    stubMatchMedia(true);
    render(<MemoryRouter><AppShell><div /></AppShell></MemoryRouter>);
    expect(screen.getByTestId("app-sidebar").getAttribute("data-collapsed")).toBe("true");
  });

  it("honours the stored preference above the breakpoint", () => {
    localStorage.setItem("egret:sidebar-collapsed", "0");
    stubMatchMedia(false);
    render(<MemoryRouter><AppShell><div /></AppShell></MemoryRouter>);
    expect(screen.getByTestId("app-sidebar").getAttribute("data-collapsed")).toBe("false");
  });
});
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/layout/__tests__/AppShell.responsive.test.tsx`

Expected: FAIL — no element with `data-testid="app-sidebar"`.

- [ ] **Step 4: Write the implementation**

In `frontend/src/components/layout/AppShell.tsx`, add the import and derive the effective state:

```tsx
import { useIsNarrowViewport } from "@/lib/useIsNarrowViewport";
```

Inside `AppShell`, after the existing `collapsed` state and its effect:

```tsx
  const narrow = useIsNarrowViewport();
  // The stored preference is a desktop preference; below the breakpoint there is no room for
  // the expanded rail regardless of what the user last chose on a wide screen.
  const effectiveCollapsed = narrow || collapsed;
```

Change the `<Sidebar>` line to pass the derived value:

```tsx
          <Sidebar collapsed={effectiveCollapsed} onToggle={() => setCollapsed((c) => !c)} />
```

In `frontend/src/components/layout/Sidebar.tsx`, add the test hook to its root element:

```tsx
      data-testid="app-sidebar"
      data-collapsed={collapsed ? "true" : "false"}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/layout/__tests__/AppShell.responsive.test.tsx`

Expected: PASS, 2 tests.

- [ ] **Step 6: Prove the test survives mutation**

Change `effectiveCollapsed` to `collapsed` and re-run. Expected: the first test FAILS. Restore.

- [ ] **Step 7: Run the full suite and commit**

```bash
cd frontend && npm run lint
cd frontend && npx tsc --noEmit
cd frontend && npm run test
```

```bash
git add frontend/src/lib/useIsNarrowViewport.ts \
        frontend/src/components/layout/AppShell.tsx \
        frontend/src/components/layout/Sidebar.tsx \
        frontend/src/components/layout/__tests__/AppShell.responsive.test.tsx
git commit -m "fix(layout): collapse the navigation sidebar below 768px so narrow viewports keep their canvas"
```

---

## Task 5: Live check — measure, do not eyeball

**Files:** none. This task produces numbers for the PR body.

Every defect that mattered in the chart programme was found by measuring the running app. A
screenshot that "looks right" is not evidence; three of the six live-check findings in that
programme were invisible at a glance and only showed up in the DOM.

- [ ] **Step 1: Build and deploy**

```bash
just deploy-kind
```

Expected: `deployment "egret" successfully rolled out`, and the SPA served at
`http://localhost:30080`.

- [ ] **Step 2: Measure the mobile case**

Open `http://localhost:30080/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`
(admin@egret.local / Admin12345!) at a 390×844 viewport and evaluate:

```js
(() => {
  const g = document.querySelector('.react-grid-layout');
  const items = [...document.querySelectorAll('.react-grid-item')].slice(0, 3)
    .map(e => { const r = e.getBoundingClientRect(); return { x: Math.round(r.x), w: Math.round(r.width) }; });
  return { vw: innerWidth, gridW: Math.round(g.getBoundingClientRect().width), items,
           scrollW: document.documentElement.scrollWidth };
})()
```

Required: `scrollW === vw` (no horizontal scroll), every card's `w` within 4px of `gridW`, and
every `x` identical. The pre-fix baseline for comparison, measured 2026-08-09, was
`gridW: 86, items: [{x:264,w:9}, {x:279,w:9}, {x:295,w:9}], scrollW: 441`.

- [ ] **Step 3: Measure the desktop case is unchanged**

Resize to 1440×900 and evaluate the same expression. Required: `gridW: 1136` and the first three
cards at `x: 264/454/645, w: 184` — the pre-fix desktop values. This workstream must not move
desktop by a pixel.

- [ ] **Step 4: Prove a mobile visit did not rewrite the layout**

This is the assertion the whole workstream turns on, and it cannot be made from a screenshot.

1. At 1440px, note the first card's `grid_col`/`size_x` from
   `GET /api/workspaces/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`.
2. Resize to 390px, enter edit mode, wait for the grid to settle, press **Save**.
3. Re-fetch the dashboard.

Required: the card geometry is byte-identical to step 1. If any card came back `grid_col: 0,
size_x: 1`, the guard in Task 3 is not wired — recheck that `onBreakpointChange` actually fires
before the first `onLayoutChange`.

- [ ] **Step 5: Record the numbers in the PR body**

Open the PR with the three measurements above quoted verbatim, plus the frontend test count.
Then stop — the merge is the user's call.

---

## Self-review checklist for the implementer

- [ ] `DashboardView.tsx` gained no net behaviour, only wiring — the guard lives in
      `grid-breakpoints.ts` where it is testable.
- [ ] `kindOf` and `onBreakpointChange` are **required** props, not optional.
- [ ] Both halves of `mayPersistLayout` are exercised by a test that fails when either is removed.
- [ ] The edit-mode guide grid does not render on mobile.
- [ ] No test asserts a display string where the code keys on a raw value.
