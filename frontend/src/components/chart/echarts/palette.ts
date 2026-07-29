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
 *    non-visual "data-table" fallback). This is the part that earns its keep.
 *  - `decal.show` would overlay patterns (dots/hatches) on filled series so colour
 *    could be told apart without relying on hue. **Off since chart 05**, for three
 *    reasons: neither reference tool does it (`grep -r decal` finds nothing in either
 *    Metabase or Superset); egret's palette IS Metabase's, so the colours were never
 *    the weak point; and on a nested pie the patterns actively fought the design —
 *    lightness carries ring depth and hue carries family, and a third orthogonal
 *    channel obscured both rather than adding anything.
 *
 *  This was never a deliberate choice. Sub-project B2 found the `aria` component had
 *  never been registered, so every aria and decal setting was dead code; registering
 *  it brought the patterns to life and nobody looked at whether they should be.
 *
 *  Line charts still cycle dash style and marker shape per series (`lineType` /
 *  `symbolShape`), which is the same affordance without covering the fill. */
export const ARIA_A11Y = { enabled: true, decal: { show: false } } as const;

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

// Per-ring OKLab lightness delta for `lightenOklch`. With up to three rings (task 5's pie),
// the outermost ring is at most 2 steps out from its parent, so +0.24 L total — enough to read
// as a clearly distinct, lighter band next to its parent's ring without washing a mid-tone
// slice out to near-white.
//
// Checked against Metabase's OWN numbers, not eyeballed: Metabase's ring colouring
// (getRingColorAlias, ui/colors/accents.ts) shifts HSL lightness by a fixed
// CHART_TINT_SHADE_FACTOR = 0.125 (12.5 points) for its "light" (outer) variant and -0.125 for
// its "dark" (inner) variant — a total spread of 0.25 across its two non-base rings. This
// codebase's 3 accumulated OKLab-L steps of 0.12 span 0 -> 0.24 from ring 0 to ring 2 — within
// 0.01 of Metabase's own total spread, in a MORE perceptually-uniform space (OKLab) than
// Metabase's HSL. That comparison does not warrant a change, so 0.12 stays.
const OKLCH_STEP_L = 0.12;

// Cap the TARGET lightness at 0.9 rather than 1.0. OKLab's gamut boundary curves inward as L
// approaches 1 (physically: not every hue can reach pure white at full saturation), so
// `linearToSrgb`'s per-channel clamp increasingly has to clip individual channels rather than
// scale them together — which is exactly what erodes chroma. Stopping short of that edge at
// 0.9 keeps every SERIES_PALETTE colour's chroma comfortably above a "still visibly tinted"
// floor even after 3 lightening steps (see the palette.test.ts sweep over the whole palette).
const OKLCH_MAX_L = 0.9;

function srgbToLinear(c: number): number {
  return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}
function linearToSrgb(c: number): number {
  const x = Math.min(1, Math.max(0, c));
  return x <= 0.0031308 ? x * 12.92 : 1.055 * Math.pow(x, 1 / 2.4) - 0.055;
}

/** Parses a `#rgb` / `#rrggbb` hex colour to 0-255 channels; anything else is unparseable. */
function parseHexColor(color: string): [number, number, number] | undefined {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(color.trim());
  if (!m) return undefined;
  const hex = m[1].length === 3 ? m[1].split("").map((c) => c + c).join("") : m[1];
  const n = parseInt(hex, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/**
 * Lightens a hex colour by `steps` in OKLab space, keeping chroma (C) and hue (H) fixed —
 * i.e. moves straight up the L axis of OKLCH. Used to give an outer pie ring's slice a
 * lighter variant of its parent slice's colour (Metabase's `getRingColorAlias`), rather than
 * an unrelated palette colour.
 *
 * Conversion matrices are Björn Ottosson's OKLab (https://bottosson.github.io/posts/oklab/),
 * §"Converting from linear sRGB to OKLab" and its inverse.
 *
 * `steps <= 0` and unparseable input both return `color` unchanged (verbatim, not
 * round-tripped) — the former so "no lightening" is exact, the latter so a caller passing
 * something this function can't handle (e.g. an unresolved `var(--x)`) degrades to "leave it
 * alone" rather than throwing.
 */
export function lightenOklch(color: string, steps: number): string {
  if (!Number.isFinite(steps) || steps <= 0) return color;
  const rgb = parseHexColor(color);
  if (!rgb) return color;

  const [r, g, b] = rgb.map((c) => srgbToLinear(c / 255));

  // linear sRGB -> LMS -> LMS' (cube root)
  const l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
  const m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
  const s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
  const l_ = Math.cbrt(l);
  const m_ = Math.cbrt(m);
  const s_ = Math.cbrt(s);

  // LMS' -> OKLab
  const L = 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_;
  const a = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_;
  const bb = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_;

  // Raise L only; a/b (so chroma + hue) are untouched — the whole point of doing this in
  // OKLab rather than a naive sRGB blend toward white, which desaturates and shifts hue.
  // Capped at OKLCH_MAX_L (see above), but never DARKENED below the input: a colour that
  // starts out brighter than the cap already (rare for this codebase's palette, but possible
  // for an arbitrary sliceColors override) is left at its own lightness rather than dimmed.
  const target = L + OKLCH_STEP_L * steps;
  const L2 = Math.max(L, Math.min(target, OKLCH_MAX_L));

  // OKLab -> LMS' (inverse of the forward matrix above)
  const l2_ = L2 + 0.3963377774 * a + 0.2158037573 * bb;
  const m2_ = L2 - 0.1055613458 * a - 0.0638541728 * bb;
  const s2_ = L2 - 0.0894841775 * a - 1.2914855480 * bb;
  const l2 = l2_ * l2_ * l2_;
  const m2 = m2_ * m2_ * m2_;
  const s2 = s2_ * s2_ * s2_;

  // LMS -> linear sRGB
  const r2 = 4.0767416621 * l2 - 3.3077115913 * m2 + 0.2309699292 * s2;
  const g2 = -1.2684380046 * l2 + 2.6097574011 * m2 - 0.3413193965 * s2;
  const b2 = -0.0041960863 * l2 - 0.7034186147 * m2 + 1.7076147010 * s2;

  const toHex = (c: number) => Math.round(linearToSrgb(c) * 255).toString(16).padStart(2, "0");
  return `#${toHex(r2)}${toHex(g2)}${toHex(b2)}`;
}

/** Concrete fallbacks (≈ egret light-theme tokens) for non-DOM contexts. */
export const COLOR_FALLBACK = {
  gridLine: "#e5e7eb",     // ≈ --border (very light gray)
  axisLabel: "#6b7280",    // ≈ --muted-foreground (slate)
  text: "#111827",         // ≈ --foreground (near-black)
  card: "#ffffff",         // ≈ --card (charts render inside a Card, not the page background)
} as const;
