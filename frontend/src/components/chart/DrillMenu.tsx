import type { ChartFilter } from "@/api/charts";
import { drillFilters, type DrillContext } from "./drill";

export function DrillMenu({
  ctx, x, y, onFilterBy, onSeeRecords, onClose,
}: {
  ctx: DrillContext; x: number; y: number;
  onFilterBy: (filter: ChartFilter) => void;
  onSeeRecords: (filters: ChartFilter[]) => void;
  onClose: () => void;
}) {
  return (
    <>
      <button type="button" aria-label="close menu" className="fixed inset-0 z-40 cursor-default" onClick={onClose} />
      <div
        className="fixed z-50 min-w-48 overflow-hidden rounded-md border bg-popover p-1 text-sm shadow-md"
        style={{ left: x, top: y }}
      >
        {ctx.dims.map((d, i) => (
          <button
            key={i}
            type="button"
            className="block w-full rounded px-2 py-1.5 text-left hover:bg-muted"
            onClick={() => { onFilterBy({ col: d.col, op: "eq", value: d.value }); onClose(); }}
          >
            Filter by {d.col} is {String(d.value)}
          </button>
        ))}
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
