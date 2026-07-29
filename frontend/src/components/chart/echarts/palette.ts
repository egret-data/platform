// Metabase chart styling, lifted from metabase frontend
// (ui/colors/constants/accent-colors.ts + echarts/cartesian/constants/style.ts).

/** Categorical series colors, cycled in order. */
export const SERIES_PALETTE = [
  "#509EE3", // blue
  "#88BF4D", // green
  "#A989C5", // purple
  "#EF8C8C", // red
  "#F9D45C", // yellow
  "#F2A86F", // orange
  "#98D9D9", // cyan
  "#7172AD", // indigo
];

export const CHART_THEME = {
  barWidth: "80%",      // CHART_STYLE.series.barWidth 0.8
  areaOpacity: 0.3,     // CHART_STYLE.opacity.area
  scatterOpacity: 0.8,  // CHART_STYLE.opacity.scatter
  symbolSize: 6,        // CHART_STYLE.symbolSize
  lineWidth: 2,         // LINE_SIZE.M
  labelFontSize: 13,    // FONT_SIZES.label
  labelWeight: 700,     // CHART_STYLE.seriesLabels.weight
  axisFontSize: 13,       // FONT_SIZES.label
  axisWeight: 400,      // CHART_STYLE.axisTicks/axisName.weight
  // gridline / axis line color — light hairline, like Metabase's
  // --mb-color-cartesian-grid-line. Uses the egret CSS border token.
  gridLine: "var(--border)",
  axisLabel: "var(--muted-foreground)",
  grid: { left: 8, right: 16, top: 16, bottom: 12, containLabel: true },
} as const;

export const seriesColor = (i: number) => SERIES_PALETTE[i % SERIES_PALETTE.length];

/** Line dash styles cycled per series so line charts stay distinguishable without
 * relying on colour alone (WCAG "color-not-only"; colour-blind users). ECharts
 * `lineStyle.type` accepts "solid" | "dashed" | "dotted" or a dash-array. */
export const LINE_TYPES: (string | number[])[] = ["solid", "dashed", "dotted", [9, 3, 2, 3]];
export const lineType = (i: number) => LINE_TYPES[i % LINE_TYPES.length];

/** Marker shapes cycled per series (line points + scatter) — shape distinguishes
 * a series without relying on colour. */
export const SYMBOLS = ["circle", "rect", "triangle", "diamond", "pin", "arrow"];
export const symbolShape = (i: number) => SYMBOLS[i % SYMBOLS.length];

/** ECharts accessibility:
 *  - `enabled` generates a screen-reader description of the chart's data (the
 *    non-visual "data-table" fallback).
 *  - `decal.show` overlays distinct patterns (dots/hatches) on filled series
 *    (bars, pie slices, areas) so colour-blind users can tell them apart without
 *    relying on the palette colour alone (WCAG "color-not-only"). Line charts add
 *    dash/marker cycling on top of this (see `lineType`/`symbolShape`). */
export const ARIA_A11Y = { enabled: true, decal: { show: true } } as const;

// Metabase colours a single (un-broken-out) series with its "summarize" green
// (palm/50 = hsl(89 48% 40%)) rather than the first accent.
export const SUMMARIZE_COLOR = "#689735";

/** Muted color for the aggregated "Other" pie slice (Metabase uses text-tertiary). */
export const OTHER_SLICE_COLOR = "var(--muted-foreground)";

// ECharts' SVG renderer writes colors as SVG presentation attributes, where CSS
// `var()` does NOT resolve (and egret's tokens are oklch(), so a wrapping hsl()
// would be invalid anyway). Resolve a `var(--x)` expression to a concrete computed
// color via a probe element; fall back to a fixed color in non-DOM (vitest/SSR).
//
// Also doubles as the one place that validates a colour that came from somewhere less
// trusted than this file's own constants (e.g. chart settings JSONB, which is never
// whitelisted server-side): the same probe element that resolves `var()` also tells us,
// via whether the CSSOM setter accepted the assignment at all, whether `expr` is a colour
// in the first place. A concrete, valid colour is returned verbatim — not run through
// getComputedStyle — because that would normalize it (e.g. "#fff" -> "rgb(255, 255, 255)",
// "rgb(1,2,3)" -> "rgb(1, 2, 3)") and callers that pass a literal expect it back unchanged.
const colorCache = new Map<string, string>();
export function resolveColor(expr: string, fallback: string): string {
  const cached = colorCache.get(expr);
  if (cached) return cached;

  const isVar = /var\((--[\w-]+)\)/.test(expr);
  if (typeof document === "undefined" || !document.body) {
    // No DOM to probe with (SSR/non-browser tests): a var() can't be resolved at all here,
    // so fall back rather than risk shipping the literal "var(...)"; a plain literal is
    // passed through, same as always — there's simply no probe available to check it.
    const out = isVar ? fallback : expr;
    colorCache.set(expr, out);
    return out;
  }

  const probe = document.createElement("span");
  probe.style.color = "";
  probe.style.color = expr;
  if (probe.style.color === "") {
    // The CSSOM setter rejected it outright: not a colour the renderer can draw at all.
    colorCache.set(expr, fallback);
    return fallback;
  }
  if (!isVar) {
    colorCache.set(expr, expr);
    return expr;
  }

  probe.style.display = "none";
  document.body.appendChild(probe);
  const resolved = getComputedStyle(probe).color;
  probe.remove();
  // jsdom returns the literal "var(--x)" (no resolution); a real browser returns rgb().
  const out = resolved && !resolved.includes("var(") ? resolved : fallback;
  colorCache.set(expr, out);
  return out;
}

/** Concrete fallbacks (≈ egret light-theme tokens) for non-DOM contexts. */
export const COLOR_FALLBACK = {
  gridLine: "#e5e7eb",     // ≈ --border (very light gray)
  axisLabel: "#6b7280",    // ≈ --muted-foreground (slate)
  text: "#111827",         // ≈ --foreground (near-black)
} as const;
