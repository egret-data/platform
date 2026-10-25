import { useLayoutEffect, useRef, useState } from "react";
import type { ChartFilter } from "@/api/charts";
import { drillFilters, type DrillContext, type DrillDim } from "./drill";
import { isExtractUnit, temporalLabel } from "./temporal-units";
import { displayName, type FieldLabels } from "./config/field-names";

/**
 * What one "Filter by" entry will actually do.
 *
 * "See these records" learned to send a half-open range for a bucketed dimension; this menu item
 * did not, and kept emitting `= 2026-01-01`. The two halves of one menu disagreed, and the half
 * that stayed on screen — the chart filter — was the one that under-reported. The label follows:
 * "is 2026-01-01" under a column grouped by month names a day nobody asked about.
 */
function describe(d: DrillDim, filters: ChartFilter[], labels?: FieldLabels): string {
  // The column by its display name, as the axis it was clicked on names it (KI-050).
  const col = displayName(d.col, labels);
  if (filters.length === 2 && "value" in filters[0] && "value" in filters[1]) {
    return `${col} ≥ ${String(filters[0].value)} and < ${String(filters[1].value)}`;
  }
  return `${col} is ${isExtractUnit(d.unit) ? temporalLabel(d.unit, d.value) : String(d.value)}`;
}

/** Gap kept between the menu and the viewport edge. */
const EDGE = 8;

/** The menu's top-left corner: at the pointer, moved left and up just enough to keep a menu of
 *  `w`×`h` inside the viewport (KI-069). Never past the top-left margin. */
export function clampMenuPosition(x: number, y: number, w: number, h: number, vw: number, vh: number) {
  return {
    left: Math.max(EDGE, Math.min(x, vw - w - EDGE)),
    top: Math.max(EDGE, Math.min(y, vh - h - EDGE)),
  };
}

export function DrillMenu({
  ctx, x, y, labels, onFilterBy, onSeeRecords, onClose,
}: {
  ctx: DrillContext; x: number; y: number;
  /** Display names by column; a column without one is named as it is. */
  labels?: FieldLabels;
  /** An array, not one filter: a bucketed dimension needs both ends of its range. */
  onFilterBy: (filters: ChartFilter[]) => void;
  onSeeRecords: (filters: ChartFilter[]) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });
  // Measured before paint, so the menu never flashes at the unclamped position.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    setPos(clampMenuPosition(x, y, width, height, window.innerWidth, window.innerHeight));
  }, [x, y]);
  return (
    <>
      <button type="button" aria-label="close menu" className="fixed inset-0 z-40 cursor-default" onClick={onClose} />
      <div
        ref={ref}
        data-testid="drill-menu"
        className="fixed z-50 min-w-48 overflow-hidden whitespace-nowrap rounded-md border bg-popover p-1 text-sm shadow-md"
        style={pos}
      >
        {ctx.dims.map((d, i) => {
          const filters = drillFilters({ dims: [d] });
          // An extraction unit contributes no filter (its value is an integer field, not an
          // instant), so the entry would be a button that changes nothing.
          if (filters.length === 0) return null;
          return (
            <button
              key={i}
              type="button"
              className="block w-full rounded px-2 py-1.5 text-left hover:bg-muted"
              onClick={() => { onFilterBy(filters); onClose(); }}
            >
              Filter by {describe(d, filters, labels)}
            </button>
          );
        })}
        {ctx.dims.length > 0 && <div className="my-1 border-t" />}
        <button
          type="button"
          className="block w-full rounded px-2 py-1.5 text-left hover:bg-muted"
          onClick={() => { onSeeRecords(drillFilters(ctx)); onClose(); }}
        >
          See these records
        </button>
      </div>
    </>
  );
}
