import { ARIA_A11Y, CHART_THEME, SERIES_PALETTE, resolveFontFamily } from "./palette";

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

/**
 * Per-item tooltip: `name: value` with an optional share, used by every chart whose marks
 * are discrete items (pie, funnel, sankey, map, boxplot).
 */
export function itemTooltip(format: (v: unknown) => string, opts: { percent?: boolean } = {}) {
  return {
    trigger: "item" as const,
    textStyle: baseTextStyle(),
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
    textStyle: baseTextStyle(),
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
