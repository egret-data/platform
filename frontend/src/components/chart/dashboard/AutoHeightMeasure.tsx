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
