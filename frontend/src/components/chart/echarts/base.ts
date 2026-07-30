import { ARIA_A11Y, CHART_THEME, COLOR_FALLBACK, SERIES_PALETTE, mixColor, resolveColor, resolveFontFamily } from "./palette";

/**
 * The parts of an ECharts option that should look the same in every chart type.
 *
 * Before this, five of the eight option builders restated their own fonts, legends and
 * tooltips (and three restated none, so they silently used ECharts' defaults). Each piece
 * here is composed rather than copied, so "what a tooltip looks like in egret" is one
 * answer, in one place.
 */

/**
 * Text shared by axis labels, legends and tooltips: the size, and the PRODUCT'S OWN TYPEFACE.
 *
 * A function rather than the constant it used to be, because the font is resolved from a CSS
 * custom property at render time (see `resolveFontFamily`) — a module-level constant would freeze
 * whatever the token said when this file was first imported, and would go stale on a theme change
 * exactly the way a module-level colour would.
 */
export function baseTextStyle() {
  return { fontSize: CHART_THEME.labelFontSize, fontFamily: resolveFontFamily() };
}

/** Everything a chart gets before it adds its own series. */
export function baseOption() {
  return {
    // Screen-reader data description; decal patterns so filled series stay distinguishable
    // without relying on colour (WCAG "colour is not the only visual means").
    aria: ARIA_A11Y,
    color: SERIES_PALETTE,
    // At the option ROOT, which is where Metabase states it too: ECharts hands this down as the
    // default for the text every component draws, so one line puts a whole chart in the app's
    // font instead of the browser's. It does NOT reach `graphic` elements — the pie's centre
    // total names the font itself for that reason.
    textStyle: baseTextStyle(),
  };
}

// ---------------------------------------------------------------------------------------------
// What a tooltip looks like in egret.
//
// Measured against Metabase side by side (its sample question 7 against egret's Sunburst Demo, both
// at 1440x920) egret was drawing ECharts' FACTORY tooltip, because no option here had ever said
// otherwise: a white box with a 1px border, every cell the same 13px grey, and the share in an
// accent blue. Two of those actively mislead.
//   - The border takes the HOVERED SLICE'S OWN COLOUR (ECharts' `trigger: "item"` default). It is
//     not a legend and it is not a state; nothing about a tooltip's outline should change with what
//     happens to be under the pointer, so the colour reads as meaning something and means nothing.
//   - An accent on the share claims the share is the number to read. The number to read is the
//     value; Metabase says so by DIMMING the share relative to it.
//
// So the treatment copied here is Metabase's RELATIONSHIP, not its palette: one floating surface,
// NO border, and three levels of emphasis down the row — value at full strength, name lighter,
// share dimmer still. Its own colours are a literal dark slate (rgb(48, 61, 70)) with white ink,
// which is a good answer for a product that has no dark mode and a disappearing one for egret,
// whose surfaces flip. egret's equivalent is the pair every OTHER floating surface in this app is
// tokenised with — `--popover` / `--popover-foreground` (see ui/popover.tsx, ui/dropdown-menu.tsx)
// — with the dimmer levels mixed from that one ink, so their ordering holds in either theme by
// construction rather than by two hand-picked tokens happening to agree.
//
// SHARED, DELIBERATELY, BY EVERY CHART TYPE. This file exists to give "what a tooltip looks like in
// egret" one answer (see the header above), and a tooltip whose surface depends on which viz you
// are hovering is the same class of meaningless variation as the coloured border. Only the pie has
// a formatter of its own (a sibling breakdown — see options/pie.ts); the rest emit plain text and
// simply inherit the ink and size below.

/** Tooltip body text. A step up from `CHART_THEME.labelFontSize`, which is an AXIS-TICK size:
 * 13px is right for text you read in passing at the edge of a chart and cramped for a table you
 * have deliberately stopped to read. Metabase's own tooltip rows are 14px for the same reason. */
export const TOOLTIP_BODY_FONT_SIZE = 14;
/** The header stays at 13 — it names the comparison; it is not part of it. */
export const TOOLTIP_HEADER_FONT_SIZE = 13;

/**
 * The three emphasis levels, as weights of the ONE popover ink against the popover surface.
 *
 * `value` is the number the tooltip exists to report, so it is that ink undiluted. `muted` is
 * Metabase's own dimming of its header and its shares (0.69 of the ink). `name` sits at the
 * midpoint of the two, which is what makes this a RAMP rather than a two-tone split: the eye lands
 * on the value, then the name, then the share.
 *
 * Mixed from one ink rather than picked as three tokens because egret HAS one popover ink, and
 * inventing two more would be a palette change rather than a chart change — and because a mix
 * keeps its ordering in both themes for free, where three hand-picked tokens have to be re-checked
 * per theme. `base.tooltip.test.ts` pins every level's contrast against the real `--popover` of
 * both themes rather than against these numbers.
 */
export const TOOLTIP_INK_WEIGHT = { value: 1, name: 0.85, muted: 0.69 } as const;

/**
 * The row weights — the OTHER half of the three-level ramp: colour separates the value from the
 * share, weight separates the value from the name (both of Metabase's own, 600 against 400).
 * `focused` is the row the pointer is on, which goes bold throughout — pre-existing pie behaviour,
 * unchanged here.
 */
export const TOOLTIP_WEIGHT = { value: 600, name: 400, header: 400, focused: 700 } as const;

/** The tooltip's surface and its three ink levels, resolved against the current theme. */
export function tooltipInk() {
  const surface = resolveColor("var(--popover)", COLOR_FALLBACK.popover);
  const ink = resolveColor("var(--popover-foreground)", COLOR_FALLBACK.popoverForeground);
  return {
    surface,
    value: mixColor(ink, surface, TOOLTIP_INK_WEIGHT.value),
    name: mixColor(ink, surface, TOOLTIP_INK_WEIGHT.name),
    muted: mixColor(ink, surface, TOOLTIP_INK_WEIGHT.muted),
  };
}

/**
 * The tooltip's own box: the app's floating surface, no border, and the body size and ink that
 * every chart's plain-text formatter inherits. Spread into EVERY tooltip in this directory.
 */
export function tooltipSurface() {
  const ink = tooltipInk();
  return {
    backgroundColor: ink.surface,
    // Zero, not a colour: ECharts' default is a 1px border in the hovered item's own colour, and
    // there is no border here to give a better colour TO. The surface and the shadow already say
    // "this floats above the chart".
    borderWidth: 0,
    textStyle: { ...baseTextStyle(), fontSize: TOOLTIP_BODY_FONT_SIZE, color: ink.value },
  };
}

/**
 * Per-item tooltip: `name: value` with an optional share, used by every chart whose marks
 * are discrete items (pie, funnel, sankey, map, boxplot).
 */
export function itemTooltip(format: (v: unknown) => string, opts: { percent?: boolean } = {}) {
  return {
    trigger: "item" as const,
    ...tooltipSurface(),
    formatter: (p: { name?: string; value?: unknown; percent?: number; data?: { slicePercent?: number } }) => {
      const name = String(p.name ?? "");
      if (p.value == null || (typeof p.value === "number" && Number.isNaN(p.value))) return name;
      const value = format(p.value);
      // A datum that states its own share wins over ECharts'. Required, not merely preferred, for
      // the pie: a `sunburst` series has no `percent` at all — only `PieSeriesModel.getDataParams`
      // adds one (and only it honours `percentPrecision`), and the pie is a sunburst now, so every
      // slice supplies `slicePercent` itself (share of the whole chart at ring 0, of its own parent
      // below that — see options/pie.ts). The `?? p.percent` fallback is what the other item-tooltip
      // users (funnel, map, boxplot, sankey) still ride on.
      const percent = p.data?.slicePercent ?? p.percent;
      return opts.percent && percent != null ? `${name}: ${value} (${percent}%)` : `${name}: ${value}`;
    },
  };
}

/** Axis tooltip with the shadow pointer, for charts whose marks share a category axis. */
export function axisTooltip() {
  return {
    trigger: "axis" as const,
    axisPointer: { type: "shadow" as const },
    ...tooltipSurface(),
  };
}

// NO LEGEND BUILDERS HERE ANY MORE. `bottomLegend`/`sideLegend` composed an ECharts `legend`
// component, and pie was their only caller — no cartesian chart here has ever set one. The pie's
// legend is now React markup rendered beside the canvas (buildPieLegend + PieLegend.tsx), because
// ECharts' legend does not work with a `sunburst` series and that is what the pie is.
// Leaving the helpers behind would advertise a shared answer to "what does a legend look like in
// egret" that nothing implements.

/**
 * A screen-reader description of the chart, replacing the one ECharts generates.
 *
 * The generated one enumerates every dimension of each datum, so a bar chart reads "the
 * data for Books is 0, 29825" — the category index and then the value. This says what a
 * sighted user sees, with the chart's own number formatting.
 */
export function ariaDescription(d: {
  kind: string;
  metric?: string;
  dimension?: string;
  points: { name: string; value: unknown }[];
  format: (v: unknown) => string;
  /** More than one plotted series; the points then belong to the first. */
  seriesCount?: number;
}) {
  const of = d.metric ? ` of ${d.metric}` : "";
  const by = d.dimension ? ` by ${d.dimension}` : "";
  const series = d.seriesCount && d.seriesCount > 1 ? ` ${d.seriesCount} series;` : "";
  const shown = d.points.slice(0, 20);
  const items = shown.map((p) => `${p.name} ${d.format(p.value)}`).join(", ");
  const more = d.points.length > shown.length ? `, and ${d.points.length - shown.length} more` : "";
  const body = d.points.length ? `${d.points.length} values:${series} ${items}${more}.` : "No data.";
  return { ...ARIA_A11Y, label: { description: `${d.kind}${of}${by}. ${body}` } };
}
