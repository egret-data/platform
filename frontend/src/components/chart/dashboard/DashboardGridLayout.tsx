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
  // CSS custom properties do NOT resolve inside an SVG embedded as a background-image data-URL
  // (the SVG is an isolated document), so we must read --border to a concrete color and bake it in.
  // Re-read when editing toggles so a theme change between sessions is picked up.
  const [stroke, setStroke] = useState("oklch(0.85 0 0)");
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    setWidth(el.clientWidth);
    const border = getComputedStyle(document.documentElement).getPropertyValue("--border").trim();
    if (border) setStroke(border);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, [editing]);

  const rowHeight = gridRowHeight(width);
  const background = editing && width > 0
    ? generateGridBackground({ cellWidth: cellWidth(width), rowHeight, cols: GRID_COLS, marginX: GRID_MARGIN[0], marginY: GRID_MARGIN[1], stroke })
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
