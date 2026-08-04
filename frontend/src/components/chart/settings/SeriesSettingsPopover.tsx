import { Settings2 } from "lucide-react";
import type { LineSeriesStyle } from "@/api/charts";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { SeriesLineStyleSection } from "./SeriesLineStyleSection";

/**
 * One series' settings, drilled into from its row in the series list — Metabase's shape, where
 * the row carries identity (grabber, swatch, name, hide) and an ellipsis opens
 * `ChartSettingsWidgetPopover` holding everything else about that one series.
 *
 * WHY A POPOVER AND NOT MORE COLUMNS. The per-series line settings were an N-row grid of Selects:
 * every new field cost a column on every row, so the list grew in both directions and the answer
 * was to hide the whole thing behind "More options" and leave two of the four fields unbuilt. A
 * drill-in is bounded by construction — a fifth setting is a row inside a panel nobody is looking
 * at until they ask for it — which is why `shape` and `markers` could finally be rendered.
 *
 * WHY THE BUTTON IS SOMETIMES ABSENT. Everything this popover holds is line appearance, so a
 * series the builder draws as a bar would open an empty panel. Metabase self-prunes the same way
 * (a `bar` series' line rows carry `hidden`); it keeps its ellipsis because its popover still has
 * axis position and show-values in it, which egret does not make per-series. The caller decides —
 * see `hasLineSettings` on SeriesListSection — because resolving a display means asking
 * `seriesDisplayFor`, and only the caller holds the chart kind and its config.
 *
 * The header restates the swatch and the name because a popover is torn out of its row visually:
 * `ChartNestedSettingSeriesSingle` does the same, and without it a popover that opens over a
 * neighbouring row gives no way to tell which series is being edited.
 */
export function SeriesSettingsPopover({ name, label, color, seriesLine, onChange }: {
  name: string;
  /** The rename, if this series has one — the header should say what the legend says. */
  label?: string;
  /** Omitted by a chart drawn in one colour, exactly as SeriesListSection omits the swatch. */
  color?: string;
  seriesLine?: Record<string, LineSeriesStyle>;
  onChange: (patch: { seriesLine?: Record<string, LineSeriesStyle> }) => void;
}) {
  const shown = label || name;
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          // Labelled by the RAW series name, not by the rename: it is the key everything else on
          // this row is addressed by, and a test that drives "settings <rename>" would stop
          // finding the row the moment someone typed in the name box.
          aria-label={`settings ${name}`}
          title={`Settings for ${shown}`}
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
        >
          <Settings2 className="h-3.5 w-3.5" aria-hidden />
        </button>
      </PopoverTrigger>
      {/* Opens toward the chart, not back over the panel — the settings panel is the left column, so
          `side="right"` puts the popover on the empty canvas beside it, which is where Metabase's
          own lands too. (`side="left"` was tried first and covers the very rows the popover is
          about.) Radix flips it on collision, so a panel docked right still gets a visible popover.
          Sized as Metabase's is: miw 336px, mah 40rem, overflow-y auto — the list of settings is
          short today but this is the panel every future per-series setting lands in. */}
      <PopoverContent align="start" side="right" className="max-h-[40rem] w-80 overflow-y-auto">
        <div className="mb-3 flex items-center gap-2 border-b pb-2">
          {color && (
            <span
              aria-hidden
              className="h-3 w-3 shrink-0 rounded-full"
              // OKLCH tokens are not in play here: this is a stored hex the user picked, or the
              // palette's own answer for this series. It goes in raw.
              style={{ backgroundColor: color }}
            />
          )}
          <span className="min-w-0 truncate text-sm font-medium" title={shown}>{shown}</span>
        </div>
        <SeriesLineStyleSection name={name} seriesLine={seriesLine} onChange={onChange} />
      </PopoverContent>
    </Popover>
  );
}
