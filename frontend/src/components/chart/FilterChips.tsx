import type { ChartFilter } from "@/api/charts";

export function FilterChips({
  filters, onRemove,
}: { filters: ChartFilter[]; onRemove: (index: number) => void }) {
  if (filters.length === 0) return null;
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {filters.map((f, i) => (
        <span key={i} className="inline-flex items-center gap-1 rounded-full border bg-muted/50 px-2.5 py-0.5 text-xs">
          <span className="font-medium">{f.col}</span>
          <span className="text-muted-foreground">{"value" in f ? `= ${String(f.value)}` : f.op}</span>
          <button type="button" aria-label="remove filter" className="ml-0.5 text-muted-foreground hover:text-destructive"
            onClick={() => onRemove(i)}>×</button>
        </span>
      ))}
    </div>
  );
}
