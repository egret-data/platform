import { cn } from "@/lib/utils";

export interface SummaryStripItem {
  label: string;
  value: string | number;
  /** When set, the item becomes a clickable filter toggle. */
  onClick?: () => void;
  /** Highlights the item as the active filter (only meaningful with onClick). */
  active?: boolean;
}

/** A compact one-line "value label · value label" summary (replaces KPI cards).
 * Items with an `onClick` render as toggle chips so a summary can double as a
 * quick filter (e.g. click "active" to filter the list to active rows). */
export function SummaryStrip({ items }: { items: SummaryStripItem[] }) {
  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-muted-foreground">
      {items.map((it, i) => {
        const body = (
          <>
            <span className="font-medium tabular-nums text-foreground">{it.value}</span>
            {it.label}
          </>
        );
        return (
          <span key={it.label} className="inline-flex items-center gap-1">
            {i > 0 && <span className="mr-1 opacity-40">·</span>}
            {it.onClick ? (
              <button
                type="button"
                onClick={it.onClick}
                aria-pressed={it.active ?? false}
                className={cn(
                  "inline-flex items-center gap-1 rounded-md px-2 py-0.5 transition-colors hover:bg-accent hover:text-foreground",
                  it.active && "bg-accent text-foreground ring-1 ring-inset ring-border",
                )}
              >
                {body}
              </button>
            ) : (
              body
            )}
          </span>
        );
      })}
    </div>
  );
}
