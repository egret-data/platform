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
    formatter: (p: { name?: string; value?: unknown; percent?: number; data?: { parentPercent?: number } }) => {
      const name = String(p.name ?? "");
      if (p.value == null || (typeof p.value === "number" && Number.isNaN(p.value))) return name;
      const value = format(p.value);
      // ECharts' own `percent` is always share-of-that-series (share of the ring); a datum that
      // carries a parent-relative override (pie's outer rings — see pie.ts buildPieOption) wins.
      const percent = p.data?.parentPercent ?? p.percent;
      return opts.percent && percent != null ? `${name}: ${value} (${percent}%)` : `${name}: ${value}`;
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

/** The bottom, scrollable legend — or nothing, when the chart hides it. `data`, when given,
 * restricts the legend to exactly those names rather than ECharts' default of every series'
 * own data — needed by pie's nested rings, where only the first ring's slices have a single,
 * unambiguous colour (see buildPieOption).
 *
 * `formatter` rewrites each entry's text from its name. ECharts accepts either a template
 * string or a callback here; a callback is what pie needs, since the text it wants ("Doohickey
 * 23.88%") depends on a per-slice number the legend itself doesn't know about. */
export function bottomLegend(
  show: boolean | undefined,
  data?: string[],
  formatter?: (name: string) => string,
) {
  if (!show) return undefined;
  return {
    bottom: 0, type: "scroll" as const, textStyle: baseTextStyle,
    ...(data ? { data } : {}),
    ...(formatter ? { formatter } : {}),
  };
}

/**
 * The same legend as a left-hand COLUMN rather than a bottom row — for a chart whose host is
 * wide and short, where a column costs less of the shape the chart actually needs than a row
 * does (pie is the only caller; see options/pie.ts, which decides between the two from the
 * chart's real measured size and shifts its own centre to clear this column).
 *
 * `data` and `formatter` mean exactly what they mean on `bottomLegend`. `rich` is the one
 * addition a column can use and a row cannot: ECharts parses a legend formatter's output as
 * rich text once the legend's own textStyle declares rich styles, and each fragment takes its
 * own `width`/`align` — which is how a column gets Metabase's name-left, percentage-right pair
 * ("Doohickey    23.88%") instead of one indivisible run of text.
 */
export function sideLegend(
  show: boolean | undefined,
  data?: string[],
  formatter?: (name: string) => string,
  rich?: Record<string, Record<string, unknown>>,
) {
  if (!show) return undefined;
  return {
    orient: "vertical" as const, left: 0, top: "middle" as const, type: "scroll" as const,
    textStyle: rich ? { ...baseTextStyle, rich } : baseTextStyle,
    ...(data ? { data } : {}),
    ...(formatter ? { formatter } : {}),
  };
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
