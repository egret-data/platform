/** A compact one-line "value label · value label" summary (replaces KPI cards). */
export function SummaryStrip({
  items,
}: {
  items: { label: string; value: string | number }[];
}) {
  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-muted-foreground">
      {items.map((it, i) => (
        <span key={it.label} className="inline-flex items-center gap-1">
          {i > 0 && <span className="mr-1 opacity-40">·</span>}
          <span className="font-medium tabular-nums text-foreground">{it.value}</span>
          {it.label}
        </span>
      ))}
    </div>
  );
}
