import { useState } from "react";
import { IconButton } from "@/components/ui/icon-button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { SeriesDisplay } from "@/components/chart/echarts/options/cartesian";
import type { LineSeriesStyle } from "@/api/charts";
import { SeriesSettingsPopover } from "./SeriesSettingsPopover";

/** Metabase's own cap, verbatim: `lib/settings/graph.ts` sets `truncateAfter: 10` on the series
 *  order widget. Ten rows is roughly the height of the panel's viewport, so the eleventh is the
 *  first one that costs the reader the group heading above it. */
const TRUNCATE_AFTER = 10;

export interface SeriesListPatch {
  seriesOrder?: string[];
  hiddenSeries?: string[];
  seriesColors?: Record<string, string>;
  seriesLabels?: Record<string, string>;
  seriesTypes?: Record<string, SeriesDisplay>;
  seriesLine?: Record<string, LineSeriesStyle>;
}

/**
 * One row per series: move up/down, colour, rename, show/hide.
 *
 * These four settings used to live in two lists rendered back to back under the same heading —
 * colour+name in one, order+visibility in the other — so four series filled eight rows and each
 * name was printed twice. Worse, the two disagreed about order: the colour list drew names in
 * their natural order while the order list drew them ranked, so moving a series up moved it in one
 * list and not the other, and the swatch beside a name stopped belonging to the row above it.
 * Metabase gives a series exactly one row; so does this.
 *
 * `defaultColor` is asked by NAME. It used to be asked by the series' natural INDEX, with a comment
 * explaining that the renderer assigns palette slots before it applies `seriesOrder` so a reordered
 * series keeps the colour it is drawn in. That is no longer how the renderer decides: a colour comes
 * from a hash of the series' NAME (`colorsForKeys`), so there is no slot to line an index up with —
 * and asking by name makes "a reorder cannot move a swatch" true by construction rather than by the
 * caller and this component agreeing about what an index means. It also leaves no index that can be
 * out of range, so a row can no longer be handed `undefined` and turn a controlled input loose.
 *
 * `defaultDisplay` IS still asked by natural index, and that is not an oversight: Metabase's
 * first-is-a-line combo rule is genuinely about position, not about what a series is called.
 *
 * The row also carries the DRILL-IN button (`SeriesSettingsPopover`) for everything about a series
 * that is too big to sit on the row itself. Metabase's row is the same shape: identity here, the
 * rest behind an ellipsis.
 */
export function SeriesListSection({
  names, order, hidden, colors, labels, types, seriesLine, defaultDisplay, defaultColor,
  hasLineSettings, renameable = true, onChange,
}: {
  names: string[];
  order?: string[];
  hidden?: string[];
  colors?: Record<string, string>;
  /**
   * What each row is CALLED, where that differs from the key it is stored under. Two callers, one
   * meaning: with `renameable` it is the author's stored override (`seriesLabels`) and the input
   * edits it; without, it is a label the caller COMPUTED — the funnel runs its steps through the
   * dimension's `column_settings` exactly as `funnelSteps` does — and the row shows it read-only
   * beside the key. Either way `names`, and everything `onChange` writes, stays keyed on the raw
   * value; this map only ever decides what a row says.
   */
  labels?: Record<string, string>;
  /** Combo only: how each series is drawn. Absent for every other kind, whose display is fixed. */
  types?: Record<string, SeriesDisplay>;
  /** Per-series line appearance, edited inside the drill-in. Meaningless without
   *  `hasLineSettings`, which is what decides whether the drill-in exists on a row at all. */
  seriesLine?: Record<string, LineSeriesStyle>;
  /** The display a series falls back to, by its NATURAL index — Metabase's first-is-a-line rule.
   *  Undefined means this chart kind offers no per-series display at all. */
  defaultDisplay?: (naturalIndex: number) => SeriesDisplay;
  /** Omitted by a chart drawn in ONE colour — the funnel, whose bands differ only in opacity. A
   *  swatch there would be a control that cannot change anything. */
  defaultColor?: (name: string) => string;
  /** Whether this series has line appearance to drill into, by its NATURAL index. Asked of the
   *  caller rather than decided here: the answer is what `seriesDisplayFor` resolves for the chart
   *  kind and its `seriesTypes`, and only the caller holds those. Copying the rule in here is how
   *  the colour rule diverged from the renderer once already (viz/cartesian.tsx records it).
   *
   *  Absent means no row gets a button — bar, row and scatter have no per-series line appearance,
   *  and neither do the funnel's steps. A popover holding nothing is a button that opens nothing. */
  hasLineSettings?: (name: string, naturalIndex: number) => boolean;
  /** False where the renderer has no label override to read — the funnel labels its steps from the
   *  dimension value itself. An editable name there accepts typing and then discards it. */
  renameable?: boolean;
  onChange: (patch: SeriesListPatch) => void;
}) {
  // The stored order can name series that are no longer plotted, and the data can bring new
  // ones the order has never seen. Rank by the stored position, then let anything unnamed
  // keep its natural place after them — the same rule the option builder applies.
  const rank = new Map((order ?? []).map((n, i) => [n, i]));
  const naturalIndex = new Map(names.map((n, i) => [n, i]));
  const ranked = [...names].sort(
    (a, b) => (rank.get(a) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b) ?? Number.MAX_SAFE_INTEGER),
  );

  const move = (i: number, delta: number) => {
    const j = i + delta;
    if (j < 0 || j >= ranked.length) return;
    const a = ranked[i];
    const b = ranked[j];
    // Swap inside the STORED order, not inside the visible list: a series a filter excluded
    // from this run still has a remembered place, and reordering two other series must not
    // throw it away.
    const base = order?.length ? [...order] : [...ranked];
    for (const n of [a, b]) if (!base.includes(n)) base.push(n);
    const ia = base.indexOf(a);
    const ib = base.indexOf(b);
    [base[ia], base[ib]] = [base[ib], base[ia]];
    onChange({ seriesOrder: base });
  };

  // A breakout decides how many series there are, so this list has no natural bound: a 40-value
  // breakout puts 40 rows in a ~300px panel and pushes every group below it off the tab. Metabase
  // caps its own at ten (`lib/settings/graph.ts`, `truncateAfter: 10`) behind a "more" affordance.
  //
  // The cut is taken AFTER ranking, not over `names`: truncating the natural order would hide
  // whichever series the user had just moved to the top, which is the one they are working on.
  //
  // `move`'s bounds deliberately still read `ranked.length`, not the visible count — the last
  // visible row of a truncated list can still be sent down, swapping it with the first hidden one.
  // The alternative is a series that can never be moved past the cut without expanding first, and
  // a disabled arrow that gives no reason.
  const [expanded, setExpanded] = useState(false);
  const visible = expanded ? ranked : ranked.slice(0, TRUNCATE_AFTER);
  const overflow = ranked.length - visible.length;

  return (
    <div className="space-y-1">
      {visible.map((name, i) => (
        <div key={name} className="flex items-center gap-1">
          {/* Side by side — see the note in TableColumnsSection: stacking two 24px targets makes
              the row 58px tall. */}
          <div className="flex shrink-0 items-center leading-none">
            {/* IconButton, not a bare glyph: these measured 8×10 px, because a `▲` with no box is
                only as big as the glyph. See `components/ui/icon-button.tsx`. */}
            <IconButton aria-label={`Move ${name} up`} disabled={i === 0}
              className="text-[10px]" onClick={() => move(i, -1)}>▲</IconButton>
            <IconButton aria-label={`Move ${name} down`} disabled={i === ranked.length - 1}
              className="text-[10px]" onClick={() => move(i, 1)}>▼</IconButton>
          </div>
          {defaultColor && (
            <input
              type="color"
              aria-label={`color ${name}`}
              className="size-6 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
              value={colors?.[name] ?? defaultColor(name)}
              onChange={(e) => onChange({ seriesColors: { ...colors, [name]: e.target.value } })}
            />
          )}
          {defaultDisplay && (
            <Select
              value={types?.[name] ?? defaultDisplay(naturalIndex.get(name) ?? i)}
              onValueChange={(v) => onChange({ seriesTypes: { ...types, [name]: v as SeriesDisplay } })}
            >
              <SelectTrigger aria-label={`display ${name}`} className="w-[4.5rem] shrink-0 px-2 text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="line">Line</SelectItem>
                <SelectItem value="area">Area</SelectItem>
                <SelectItem value="bar">Bar</SelectItem>
              </SelectContent>
            </Select>
          )}
          {renameable ? (
            <Input
              className="min-w-0 flex-1"
              aria-label={`label ${name}`}
              placeholder={name}
              value={labels?.[name] ?? ""}
              onChange={(e) => {
                const next = { ...labels };
                if (e.target.value) next[name] = e.target.value; else delete next[name];
                onChange({ seriesLabels: next });
              }}
            />
          ) : (
            // Boxed like the editable variant, so a row without an input still reads as a row
            // rather than as text floating beside a switch. Metabase frames these the same way.
            //
            // THE LABEL FIRST, THE KEY BESIDE IT — `FormatSection`'s exact shape for a renamed
            // column (`column_title`, then the real name in muted `text-xs`). The funnel's steps
            // are drawn through the dimension's own format, so the row has to be able to say
            // "January 2026" while every control on it still acts on `2026-01-01`; showing only
            // the label would name a step one thing and edit another, and showing only the key is
            // the divergence being closed. Suppressed when they are the same string, which is the
            // common case (a text dimension, or no format set) — a row reading "Paid Paid" would be
            // noise on every funnel that has no format at all.
            <div className="flex min-w-0 flex-1 items-center gap-1.5 rounded-md border px-3 text-sm" title={name}>
              <span className="truncate">{labels?.[name] ?? name}</span>
              {labels?.[name] != null && labels[name] !== name && (
                <span className="shrink-0 truncate text-xs text-muted-foreground">{name}</span>
              )}
            </div>
          )}
          {hasLineSettings?.(name, naturalIndex.get(name) ?? i) && (
            <SeriesSettingsPopover
              name={name}
              label={labels?.[name]}
              // The swatch the row is showing, override included — the popover's header must not
              // disagree with the input two elements to its left.
              color={defaultColor && (colors?.[name] ?? defaultColor(name))}
              seriesLine={seriesLine}
              onChange={onChange}
            />
          )}
          <Switch aria-label={`Show ${name}`} checked={!hidden?.includes(name)}
            onCheckedChange={(v) => onChange({
              hiddenSeries: v ? (hidden ?? []).filter((n) => n !== name) : [...(hidden ?? []), name],
            })} />
        </div>
      ))}
      {ranked.length > TRUNCATE_AFTER && (
        // Named, not a bare chevron: "Show 3 more series" says what is behind it, where a chevron
        // on a list that is already ten rows long reads as one more row.
        <button
          type="button"
          className="text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? "Show fewer series" : `Show ${overflow} more series`}
        </button>
      )}
    </div>
  );
}
