// src/components/chart/PieLegend.tsx
import { cn } from "@/lib/utils";
import {
  PIE_LEGEND_COLUMN_WIDTH_PX,
  type PieLegendEntry,
  type PieLegendPlacement,
} from "@/components/chart/echarts/options/pie";

/**
 * The pie's legend — real markup, rendered BESIDE the chart rather than drawn inside its canvas.
 *
 * It used to be an ECharts `legend` component, which does not work with a `sunburst` series (the
 * shape this pie is moving to), so it would have silently disappeared the moment the series type
 * changed. Metabase draws its own the same way and for the same reason: an `<ol><li><span>` next
 * to the chart, not inside the SVG.
 *
 * That move is also an accessibility gain, which is why the markup is a real `<ul>` of `<button>`s
 * rather than a pile of styled `<div>`s: an ECharts legend is SVG `<text>` and `<path>` with no
 * roles, no list structure and no keyboard focus, so a screen-reader or keyboard user could
 * neither read the key nor use its toggles. Each entry is a toggle button carrying `aria-pressed`
 * — pressed meaning the slice is showing — and the swatch is `aria-hidden` because the colour it
 * repeats is meaningless read aloud.
 *
 * Layout mirrors what the option builder is told (`pieLegendPlacement`, in options/pie.ts, is the
 * one place that decides): a fixed-width column beside a wide, short chart, or a row-height-capped
 * block under a narrow, tall one. Either way it scrolls rather than growing without limit, which
 * is what the ECharts scroll legend's paging used to do.
 */
export function PieLegend({ entries, placement, toggledOff, onToggle, highlighted, onHover }: {
  entries: PieLegendEntry[];
  placement: PieLegendPlacement;
  /** Entry names the reader has clicked off, by display name — the same list the option builder
   * takes as `toggledOffSlices`, so the two can't disagree about what is showing. */
  toggledOff: string[];
  onToggle: (name: string) => void;
  /**
   * The one entry to keep lit: the ring-0 ancestor of whatever slice is under the pointer, or of
   * whichever entry the reader is pointing at here. Everything else fades, exactly as Metabase's
   * legend does — the point being that on a sunburst a slice four rings out is a long way from
   * the entry that names its colour family, and this is what connects them.
   *
   * `null`/absent means nothing is hovered and every entry reads normally.
   */
  highlighted?: string | null;
  /** This legend's own half of the two-way hover: the entry entered (or focused), and `null` when
   * it is left. Focus counts deliberately — the chart's centre reads out a value that appears
   * nowhere else, so it must not be reachable by pointer alone. */
  onHover?: (name: string | null) => void;
}) {
  if (entries.length === 0) return null;
  const side = placement === "side";
  return (
    <ul
      data-pie-legend={placement}
      className={cn(
        "flex min-h-0 shrink-0 flex-col gap-0.5 overflow-y-auto py-1 text-xs",
        // The column is centred against the circle it labels, like Metabase's; the block under a
        // chart is capped so a long list can never squeeze the chart out of its own card.
        side ? "self-center pr-2" : "max-h-[30%] w-full",
      )}
      style={side ? { width: PIE_LEGEND_COLUMN_WIDTH_PX } : undefined}
    >
      {entries.map((entry) => {
        const off = toggledOff.includes(entry.name);
        // Something else is hovered, so this entry steps back. Kept separate from `off`, which is
        // a state the reader chose and which stays legible (greyed text, greyed swatch); this is a
        // momentary de-emphasis of everything that isn't what you are looking at.
        const faded = highlighted != null && highlighted !== entry.name;
        return (
          <li key={entry.name} className="min-w-0">
            <button
              type="button"
              aria-pressed={!off}
              onClick={() => onToggle(entry.name)}
              onMouseEnter={() => onHover?.(entry.name)}
              onMouseLeave={() => onHover?.(null)}
              onFocus={() => onHover?.(entry.name)}
              onBlur={() => onHover?.(null)}
              data-pie-legend-faded={faded || undefined}
              className={cn(
                "flex w-full min-w-0 items-center gap-2 rounded px-1 py-0.5 text-left transition-opacity hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                faded && "opacity-40",
              )}
            >
              <span
                data-pie-legend-swatch
                aria-hidden="true"
                className="size-2.5 shrink-0 rounded-[2px]"
                // A toggled-off entry greys out, exactly as the ECharts legend did.
                style={{ background: off ? "var(--muted-foreground)" : entry.color }}
              />
              {/* `min-w-0` is what lets this cell shrink below its text's natural width at all —
                  without it a flex item refuses to, and a long name pushes the percentage out of
                  the column instead of truncating. `title` keeps the full text reachable. */}
              <span
                title={entry.name}
                className={cn("min-w-0 flex-1 truncate", off && "text-muted-foreground")}
              >
                {entry.name}
              </span>
              {entry.percent != null && (
                <span className="shrink-0 tabular-nums text-muted-foreground">{entry.percent}</span>
              )}
            </button>
          </li>
        );
      })}
    </ul>
  );
}
