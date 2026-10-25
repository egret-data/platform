import { useEffect, useRef, useState } from "react";
import RGL, { type LayoutItem, verticalCompactor } from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import "react-resizable/css/styles.css";
import "./dashboard-grid.css";
import { GRID_COLS, GRID_MARGIN, GRID_PADDING, gridRowHeight, cellWidth } from "@/components/chart/dashboard-grid";
import {
  GRID_COLUMNS,
  GRID_MARGIN_BY_BREAKPOINT,
  breakpointFor,
  toMobileLayout,
  type GridBreakpoint,
} from "./grid-breakpoints";
import { generateGridBackground } from "./grid-background";
import { GRID_STROKE_CSS, GRID_STROKE_FALLBACK } from "./grid-stroke";

interface Props {
  editing: boolean;
  layout: LayoutItem[];
  /** Maps a card id to the string the mobile height table keys on — a chart card's viz type,
   *  or the card type for text/heading/link. Supplied by the route, which is the only place
   *  that knows the chart behind a card. */
  kindOf: (id: string) => string;
  /** The width a side panel takes beside the canvas while it is open (the edit panel and its
   *  gap). The breakpoint is decided as if the canvas had it back: opening the panel on a
   *  1366px window left a 706px canvas, under 753px, and the cards dropped to the one-column
   *  layout, where they cannot be moved, while the author was editing them (KI-282). */
  besidePanelPx?: number;
  /** Fires whenever the live breakpoint changes. The route needs this to refuse to persist
   *  layout and auto-height changes off the desktop breakpoint. */
  onBreakpointChange: (bp: GridBreakpoint) => void;
  /** `readonly` because that is what react-grid-layout 2 hands the callback (its `Layout`
   *  IS `readonly LayoutItem[]`). Both consumers only read it. */
  onLayoutChange: (next: readonly LayoutItem[]) => void;
  onResizeStart: () => void;
  onResizeStop: () => void;
  onDragStart: () => void;
  onDragStop: () => void;
  /** Render-prop: receives the live rowHeight so cards can convert px↔rows (auto-height). */
  children: (rowHeight: number) => React.ReactNode;
}

/** Metabase-style dashboard grid: 24 columns on desktop, one column below 753px of CONTAINER
 *  width, dynamic aspect-ratio rowHeight, 6px margins, vertical compaction, and an SVG cell grid
 *  behind the cards while editing on desktop. */
export function DashboardGridLayout({
  editing,
  layout,
  kindOf,
  besidePanelPx = 0,
  onBreakpointChange,
  onLayoutChange,
  onResizeStart,
  onResizeStop,
  onDragStart,
  onDragStop,
  children,
}: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  // CSS custom properties do NOT resolve inside an SVG embedded as a background-image data-URL
  // (the SVG is an isolated document), so we must read a CONCRETE color and bake it in. The
  // expression itself lives in grid-stroke.ts, where the contrast band it has to satisfy is
  // documented and pinned by a test. Theme-aware: re-read whenever edit mode toggles.
  const [stroke, setStroke] = useState(GRID_STROKE_FALLBACK);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    setWidth(el.clientWidth);
    const probe = document.createElement("div");
    probe.style.color = GRID_STROKE_CSS;
    el.appendChild(probe);
    const resolved = getComputedStyle(probe).color;
    probe.remove();
    setStroke(resolved || GRID_STROKE_FALLBACK);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      const w = el.clientWidth;
      // Report the breakpoint from inside the observer, not only from the effect below. The
      // effect cannot run until React has re-rendered, and a card's own ResizeObserver (for
      // auto-height) fires in the SAME delivery batch as this one — so a route that learned
      // the breakpoint only from a re-render would still believe it was on desktop while a
      // card measured itself at a phone width.
      onBreakpointChange(breakpointFor(w + besidePanelPx));
      setWidth(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [editing, onBreakpointChange, besidePanelPx]);

  const breakpoint = breakpointFor(width + besidePanelPx);
  const isMobile = breakpoint === "mobile";

  // Tell the route on every change, and on first measurement. The route uses it as a guard,
  // so a missed notification means a mobile layout can be persisted.
  useEffect(() => {
    onBreakpointChange(breakpoint);
  }, [breakpoint, onBreakpointChange]);

  // THE guard. react-grid-layout emits `onLayoutChange` with the derived one-column layout the
  // moment it commits a mobile render — measured against RGL 1.5.3, the very first emission of a
  // fresh mount is `[{i:"a",x:0,w:1},{i:"b",x:0,w:1}]` even on a 1136px container, because the
  // container has not been measured yet. Anything downstream that decides via React state is one
  // commit too late (RGL's componentDidUpdate runs before this component's effects), so the
  // refusal has to happen here, in the same closure that produced the layout being emitted.
  const handleLayoutChange = (next: readonly LayoutItem[]) => {
    if (isMobile) return;
    onLayoutChange(next);
  };

  const cols = GRID_COLUMNS[breakpoint];
  const margin = GRID_MARGIN_BY_BREAKPOINT[breakpoint];
  const effectiveLayout = isMobile ? toMobileLayout(layout, kindOf) : layout;
  const rowHeight = gridRowHeight(width);

  // The guide grid describes a 24-column canvas; on one column it describes nothing, and it is
  // the loudest element on a small screen. Desktop-and-editing only.
  const background = editing && !isMobile && width > 0
    ? generateGridBackground({ cellWidth: cellWidth(width), rowHeight, cols: GRID_COLS, marginX: GRID_MARGIN[0], marginY: GRID_MARGIN[1], stroke })
    : undefined;

  return (
    <div ref={ref} className="relative">
      {background && <div aria-hidden className="pointer-events-none absolute inset-0 z-0" style={{ backgroundImage: background }} />}
      <RGL
        className="layout relative z-10"
        width={width}
        // v2 groups the old flat props into config objects and takes a compactor
        // function where v1 took `compactType="vertical"`. Same values, new shape —
        // the legacy adapter under `react-grid-layout/legacy` would accept the v1
        // props, but it is a deprecation path and this is the only call site.
        gridConfig={{
          cols,
          rowHeight,
          margin,
          containerPadding: GRID_PADDING,
        }}
        compactor={verticalCompactor}
        dragConfig={{
          enabled: editing && !isMobile,
          handle: ".dash-draghandle",
          cancel: ".dash-nodrag",
        }}
        resizeConfig={{ enabled: editing && !isMobile }}
        layout={effectiveLayout}
        onLayoutChange={handleLayoutChange}
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
