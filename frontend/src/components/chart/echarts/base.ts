import { ARIA_A11Y, CHART_THEME, SERIES_PALETTE } from "./palette";

/**
 * The parts of an ECharts option that should look the same in every chart type.
 *
 * Before this, five of the eight option builders restated their own fonts, legends and
 * tooltips (and three restated none, so they silently used ECharts' defaults). Each piece
 * here is composed rather than copied, so "what a tooltip looks like in egret" is one
 * answer, in one place.
 */

/** Text sizing shared by axis labels, legends and tooltips. */
export const baseTextStyle = { fontSize: CHART_THEME.labelFontSize } as const;

/** Everything a chart gets before it adds its own series. */
export function baseOption() {
  return {
    // Screen-reader data description; decal patterns so filled series stay distinguishable
    // without relying on colour (WCAG "colour is not the only visual means").
    aria: ARIA_A11Y,
    color: SERIES_PALETTE,
    textStyle: baseTextStyle,
  };
}

/**
 * Per-item tooltip: `name: value` with an optional share, used by every chart whose marks
 * are discrete items (pie, funnel, sankey, map, boxplot).
 */
export function itemTooltip(format: (v: unknown) => string, opts: { percent?: boolean } = {}) {
  return {
    trigger: "item" as const,
    textStyle: baseTextStyle,
    formatter: (p: { name?: string; value?: unknown; percent?: number }) => {
      const name = String(p.name ?? "");
      if (p.value == null || (typeof p.value === "number" && Number.isNaN(p.value))) return name;
      const value = format(p.value);
      return opts.percent && p.percent != null ? `${name}: ${value} (${p.percent}%)` : `${name}: ${value}`;
    },
  };
}

/** Axis tooltip with the shadow pointer, for charts whose marks share a category axis. */
export function axisTooltip() {
  return {
    trigger: "axis" as const,
    axisPointer: { type: "shadow" as const },
    textStyle: baseTextStyle,
  };
}

/** The bottom, scrollable legend — or nothing, when the chart hides it. */
export function bottomLegend(show: boolean | undefined) {
  return show ? { bottom: 0, type: "scroll" as const, textStyle: baseTextStyle } : undefined;
}

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
