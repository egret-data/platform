import { useEffect, useRef, useState } from "react";
import RGL, { type Layout } from "react-grid-layout";
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
  // (the SVG is an isolated document), so we must read a CONCRETE color and bake it in. We want a
  // grid a touch darker than --border (which is nearly invisible), so blend it toward --foreground
  // via color-mix and read the computed result off a probe element. Theme-aware (re-read on edit).
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
    if (mixed) setStroke(mixed);          // preferred: border blended toward foreground (visible)
    else if (border) setStroke(border);   // fallback: plain --border if color-mix is unavailable
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, [editing]);

  const breakpoint = breakpointFor(width);
  const isMobile = breakpoint === "mobile";

  // Tell the route on every change, and on first measurement. The route uses it as a guard,
  // so a missed notification means a mobile layout can be persisted.
  useEffect(() => {
    onBreakpointChange(breakpoint);
  }, [breakpoint, onBreakpointChange]);

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
