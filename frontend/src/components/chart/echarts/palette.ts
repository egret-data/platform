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
  // Hover states, measured on a live Metabase rather than read off its source: pointing at one
  // series lifts it and pushes the others back. An area's fill carries both halves of the effect
  // (0.3 -> 0.6 hovered, 0.3 -> 0.2 blurred); a bar or a line has no fill opacity to raise, so
  // only the blur applies to it.
  areaOpacityEmphasis: 0.6,
  areaOpacityBlur: 0.2,
  blurOpacity: 0.3,
  scatterOpacity: 0.8,  // CHART_STYLE.opacity.scatter
  /** Point diameter when no bubble metric sizes them. Metabase's own floor for a sized bubble. */
  scatterSymbolSize: 15,
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

// Metabase's "summarize" green (palm/50 = hsl(89 48% 40%)) used to live here as SUMMARIZE_COLOR,
// described as the colour of a lone series. Measured on identical data on the parity dashboards it
// is not: a single-metric card takes the colour its column name hashes to, like every other series.
// What the green actually is, is Metabase's `success` theme colour, handed by `getPreferredColor` to
// a series NAMED for a good outcome ("profit", "completed") — and that rule already lives in
// `keyColors.ts` as `PREFERRED_COLORS.success`, byte for byte. Its last caller was the chart
// settings panel's default swatch, which now asks `colorsForKeys` like the renderer does, so the
// second spelling is gone rather than left about to be reached for again.

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
//
// The cache below is module-level, so it outlives every chart on the page — which means it has
// to be dropped when the theme changes, or `var(--foreground)` / `var(--background)` /
// `var(--card)` keep whichever theme's value was resolved first until a full page reload. See
// `onThemeChange` for the signal used and who listens to it.
const colorCache = new Map<string, string>();

// ThemeProvider (components/theme-provider.tsx) switches themes by swapping the `light`/`dark`
// class on <html>, which is exactly what index.css's `.dark` variant keys off — so that class
// attribute IS the theme signal, and watching it directly keeps this file free of any dependency
// on React or on the provider itself. (A DOM watcher rather than a context subscription also
// matters because ThemeProvider's `children` element is referentially stable, so a theme toggle
// does NOT re-render the chart tree on its own — hence the listeners below.)
const themeListeners = new Set<() => void>();

/** Runs `listener` after every theme change, once the colour cache has already been dropped —
 * so anything it rebuilds resolves against the new theme. Returns an unsubscribe. */
export function onThemeChange(listener: () => void): () => void {
  themeListeners.add(listener);
  return () => {
    themeListeners.delete(listener);
  };
}

function handleThemeChange(): void {
  colorCache.clear();
  fontCache = undefined;
  for (const listener of themeListeners) listener();
}

if (typeof document !== "undefined" && typeof MutationObserver !== "undefined") {
  // Deliberately never disconnected: it lives as long as the module (and so the page) does,
  // exactly like the cache it guards.
  new MutationObserver(handleThemeChange).observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class"],
  });
}

// ---------------------------------------------------------------------------------------------
// The typeface.
//
// Measured live: egret's UI is Inter and its charts were drawn in `sans-serif` — the browser's
// default — because no option ever named a font. Metabase sets one at the option root
// (`textStyle: { fontFamily: renderingContext.fontFamily }`), which is why its charts are in its
// own product font rather than in a browser default. The rule is what's worth copying; the FONT is
// egret's own, read from the same `--font-sans` token the rest of the app is set in (index.css).
// Copying Metabase's Lato instead would have put the charts in a typeface the surrounding UI
// doesn't use.

/** The token the app's typeface lives in. */
const CHART_FONT_TOKEN = "var(--font-sans)";

/** What to draw in when the token can't be read (non-DOM tests, SSR) — the head of the same stack
 * `--font-sans` holds, so the answer is still "the product's font" rather than the browser's. Kept
 * short deliberately: the full stack belongs to the token, and duplicating it here would be a
 * second place to maintain it. */
export const CHART_FONT_FALLBACK = "Inter, sans-serif";

// Cached like the colours are, and dropped by the same theme-change signal (see
// handleThemeChange): a theme is free to restate `--font-sans`, and a chart drawn before the
// switch must not keep the previous theme's answer for the life of the page.
let fontCache: string | undefined;

/**
 * The font family every chart draws in, resolved from egret's own token.
 *
 * Resolved through a probe element for the same reason `resolveColor` is: ECharts writes the font
 * into the SVG as a presentation attribute, where a CSS `var()` does not resolve. Unlike a colour
 * there is nothing to validate — any string is a legal font list — so this is the simpler of the
 * two: read it, or fall back.
 */
export function resolveFontFamily(): string {
  if (fontCache) return fontCache;
  if (typeof document === "undefined" || !document.body) return (fontCache = CHART_FONT_FALLBACK);

  const probe = document.createElement("span");
  probe.style.fontFamily = CHART_FONT_TOKEN;
  probe.style.display = "none";
  document.body.appendChild(probe);
  const resolved = getComputedStyle(probe).fontFamily;
  probe.remove();

  // jsdom resolves no custom property at all and hands back the literal (or nothing); a real
  // browser returns the token's own stack.
  fontCache = resolved && !resolved.includes("var(") ? resolved : CHART_FONT_FALLBACK;
  return fontCache;
}

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
  // jsdom returns the literal "var(--x)" (no resolution); a real browser returns a resolved
  // colour — and for egret's tokens that is an `oklch(...)` string, NOT `rgb()`: computed-value
  // serialisation only collapses to rgb() for legacy sRGB colours (CSS Color 4). Assuming rgb()
  // here is what left `parseColor` unable to read a single one of this app's own tokens, so every
  // downstream consumer of this return value has to handle oklch too.
  const out = resolved && !resolved.includes("var(") ? resolved : fallback;
  colorCache.set(expr, out);
  return out;
}

/**
 * Metabase's `CHART_TINT_SHADE_FACTOR` (0.125), as the HSL lightness POINTS its own accent
 * variants are derived with — `deriveChartTintColor`/`deriveChartShadeColor` (ui/colors/accents.ts)
 * are `Color(hex).lightness(l ± 12.5).hex()`, nothing more.
 *
 * THIS REPLACED AN OKLCH RAMP, and the space matters. egret used to lighten each ring outward
 * from the base by a fixed OKLab-L step, deliberately holding chroma constant so the outer rings
 * kept their saturation — which is exactly why the result read as more vivid than Metabase's, and
 * why the two disagreed on every ring: egret drew base -> lighter -> lightest (ring-1 blue
 * #76C4FF), Metabase draws dark -> base -> light (ring-1 blue #87BCEC, i.e. the base itself).
 * Matching the LOOK therefore means matching the derivation, in Metabase's own (less
 * perceptually-uniform) HSL — an OKLCH equivalent lands near these hexes but not on them, and
 * palette.test.ts asserts all 24 of Metabase's own accent variants exactly.
 */
const CHART_TINT_SHADE_POINTS = 12.5;

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

// OKLab -> sRGB, used by `parseColor`'s `oklch()` branch (which reads egret's design tokens).
// Conversion matrix is the inverse of Björn Ottosson's OKLab
// (https://bottosson.github.io/posts/oklab/), §"Converting from linear sRGB to OKLab".
//
// The forward direction went with `lightenOklch`: nothing in this file moves a colour along an
// OKLab axis any more (see CHART_TINT_SHADE_POINTS), so only the reader is left.

/** OKLab `[L, a, b]` -> sRGB 0-255 channels, each clamped into gamut and rounded. */
function oklabToSrgb(L: number, a: number, b: number): [number, number, number] {
  // OKLab -> LMS' (inverse of the forward matrix above) -> LMS
  const l_ = L + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = L - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = L - 0.0894841775 * a - 1.2914855480 * b;
  const l = l_ * l_ * l_;
  const m = m_ * m_ * m_;
  const s = s_ * s_ * s_;

  // LMS -> linear sRGB -> sRGB
  const r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
  const g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
  const bl = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;
  return [r, g, bl].map((c) => Math.round(linearToSrgb(c) * 255)) as [number, number, number];
}

/** sRGB 0-255 channels -> HSL, in the `color` package's own units (h 0-360, s/l 0-100) — the
 * space Metabase derives its accent variants in. */
function srgbToHsl([r255, g255, b255]: [number, number, number]): [number, number, number] {
  const [r, g, b] = [r255 / 255, g255 / 255, b255 / 255];
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  const l = (max + min) / 2;
  if (d === 0) return [0, 0, l * 100]; // achromatic: hue and saturation are meaningless
  const s = d / (1 - Math.abs(2 * l - 1));
  const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return [((h * 60) % 360 + 360) % 360, s * 100, l * 100];
}

/** HSL (h 0-360, s/l 0-100) -> sRGB 0-255 channels, rounded. */
function hslToSrgb(h: number, s100: number, l100: number): [number, number, number] {
  const s = s100 / 100;
  const l = l100 / 100;
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  const [r, g, b] =
    h < 60 ? [c, x, 0]
      : h < 120 ? [x, c, 0]
        : h < 180 ? [0, c, x]
          : h < 240 ? [0, x, c]
            : h < 300 ? [x, 0, c]
              : [c, 0, x];
  return [r, g, b].map((v) => Math.round((v + m) * 255)) as [number, number, number];
}

/** A colour with its HSL lightness moved by `points`, clamped to the 0-100 range. */
function shiftLightness(color: string, points: number): string {
  const rgb = parseColor(color);
  if (!rgb) return color; // an unresolved var(), a CSS keyword: leave it alone rather than throw
  const [h, s, l] = srgbToHsl(rgb);
  const toHex = (c: number) => c.toString(16).padStart(2, "0");
  return `#${hslToSrgb(h, s, Math.min(100, Math.max(0, l + points))).map(toHex).join("")}`;
}

/**
 * The lighter and darker variants of a chart colour — Metabase's `deriveChartTintColor` and
 * `deriveChartShadeColor` (ui/colors/accents.ts), which is how its `accentN-light` and
 * `accentN-dark` are defined: the base's HSL lightness ± CHART_TINT_SHADE_POINTS.
 *
 * DERIVED rather than looked up, which is where this deliberately parts company with Metabase.
 * Metabase maps a slice's hex back to an accent NAME and asks its theme for `accentN-dark`, so a
 * colour that isn't one of the eight accents (a custom per-slice override, egret's muted "Other"
 * grey) gets no variant at all and every ring of that family renders the same flat colour.
 * Deriving gives the same 24 hexes for the eight accents — palette.test.ts pins every one — and
 * gives an override the same three-level treatment the palette gets.
 *
 * Input goes through the shared `parseColor`, so a colour that arrived as a browser-resolved
 * token (`oklch(...)`) or as `rgb()` is handled exactly like a hex literal. That matters live
 * rather than theoretically: the "Other" slice's colour is `var(--muted-foreground)`, which
 * `resolveColor` turns into an `oklch()` string in a real browser — while an earlier version of
 * this read only hex, every ring nested under Other came back unchanged and the whole sub-tree
 * rendered one flat grey.
 */
/**
 * Scale a colour's HSL lightness by `factor` — the operation the `color` library performs for
 * `.lighten(r)` (factor `1 + r`) and `.darken(r)` (factor `1 - r`), which is what Metabase's
 * progress bar derives its track shades with.
 *
 * MULTIPLICATIVE, not additive: `shiftLightness` above moves by a fixed number of points, which is
 * right for the pie's three ring levels but wrong here — copying Metabase's numbers into an
 * additive shift would agree only for the one colour they were measured against.
 */
/**
 * The two ends of a choropleth's colour ramp, derived from ONE accent — Metabase's
 * `getColorplethColorScale`: the light end is that colour set to 92% lightness, the dark end is it
 * darkened by 0.2, and both are saturated by a further 0.1.
 *
 * egret hard-coded `["#e6f4f1", "#0f9b8e"]`, two teals belonging to no palette. They agree with
 * each other and with nothing else, so the map was the one chart drawing in a colour the design
 * system had never heard of — and a configurable accent could not have worked at all.
 */
export function choroplethScale(color: string): [string, string] {
  return [
    saturateColor(setLightness(color, 0.92), 1.1),
    saturateColor(scaleLightness(color, 0.8), 1.1),
  ];
}

/** HSL lightness set ABSOLUTELY (Metabase's `Color.lightness(n)`), not scaled. */
export function setLightness(color: string, l: number): string {
  return hslAdjust(color, (h, s) => [h, s, Math.min(1, Math.max(0, l))]);
}

/** HSL saturation scaled — Metabase's `Color.saturate(r)` is a multiply by `1 + r`. */
export function saturateColor(color: string, factor: number): string {
  return hslAdjust(color, (h, s, l) => [h, Math.min(1, s * factor), l]);
}

/** Round-trip a colour through HSL, letting the caller move any channel. */
function hslAdjust(color: string, f: (h: number, s: number, l: number) => [number, number, number]): string {
  const rgb = parseColor(color);
  if (!rgb) return color;
  const [r, g, b] = rgb.map((v) => v / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l0 = (max + min) / 2;
  const d = max - min;
  const s0 = d === 0 ? 0 : d / (1 - Math.abs(2 * l0 - 1));
  let h0 = 0;
  if (d !== 0) {
    if (max === r) h0 = ((g - b) / d) % 6;
    else if (max === g) h0 = (b - r) / d + 2;
    else h0 = (r - g) / d + 4;
    h0 *= 60;
    if (h0 < 0) h0 += 360;
  }
  const [h, s, l] = f(h0, s0, l0);
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  const [r2, g2, b2] =
    h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x]
    : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
  const hex = (v: number) => Math.round((v + m) * 255).toString(16).padStart(2, "0");
  return `#${hex(r2)}${hex(g2)}${hex(b2)}`;
}

export function scaleLightness(color: string, factor: number): string {
  const rgb = parseColor(color);
  if (!rgb) return color;
  const [r, g, b] = rgb.map((v) => v / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  let h = 0;
  if (d !== 0) {
    if (max === r) h = ((g - b) / d) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h *= 60;
    if (h < 0) h += 360;
  }
  const l2 = Math.min(1, Math.max(0, l * factor));
  const c = (1 - Math.abs(2 * l2 - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l2 - c / 2;
  const [r2, g2, b2] =
    h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x]
    : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
  const hex = (v: number) => Math.round((v + m) * 255).toString(16).padStart(2, "0");
  return `#${hex(r2)}${hex(g2)}${hex(b2)}`;
}

export function chartTint(color: string): string {
  return shiftLightness(color, CHART_TINT_SHADE_POINTS);
}

export function chartShade(color: string): string {
  return shiftLightness(color, -CHART_TINT_SHADE_POINTS);
}

/**
 * A ring's own fill, given its family's BASE colour: Metabase's three levels, inner ring darkest.
 *
 *   -1 -> the dark variant   (`accentN-dark`, the innermost ring of a nested pie)
 *    0 -> the base itself    (the middle ring, and every single-ring pie)
 *   +1 -> the light variant  (`accentN-light`, the outermost ring)
 *
 * Three fixed levels rather than a per-depth ramp, because that is what Metabase's own model does
 * (`getRingColorAlias`, called with "inner"/"middle"/"outer") and a pie draws at most three rings.
 * `variant` is clamped, so a fourth ring — which the config does not offer, but nothing here
 * enforces — repeats the outermost shade rather than fading away into white.
 */
export function ringVariantColor(base: string, variant: number): string {
  if (variant < 0) return chartShade(base);
  if (variant > 0) return chartTint(base);
  return base;
}

/** Concrete fallbacks (≈ egret light-theme tokens) for non-DOM contexts. */
export const COLOR_FALLBACK = {
  gridLine: "#e5e7eb",     // ≈ --border (very light gray)
  axisLabel: "#6b7280",    // ≈ --muted-foreground (slate)
  text: "#111827",         // ≈ --foreground (near-black)
  textInverse: "#ffffff",  // ≈ --background in light mode — the "opposite ink" for getTextColorForBackground
  card: "#ffffff",         // ≈ --card (charts render inside a Card, not the page background)
  // ≈ --popover / --popover-foreground in light mode — the pair every FLOATING surface in this app
  // is tokenised with (ui/popover.tsx, ui/dropdown-menu.tsx), and so the pair a chart tooltip is
  // drawn with too. See base.ts' `tooltipSurface`.
  popover: "#f8f8f8",
  popoverForeground: "#090909",
} as const;

// An oklch() component: a number (`0.14`), a percentage (`14%`), or the `none` keyword (which
// CSS Color 4 defines as "missing", behaving as 0 in every calculation this file does).
// `percentBasis` is what 100% means for that component — 1 for lightness, 0.4 for chroma
// (CSS Color 4 §oklch()).
function parseOklchComponent(raw: string | undefined, percentBasis: number): number | undefined {
  if (raw == null || raw.toLowerCase() === "none") return 0;
  const m = /^([+-]?(?:\d+\.?\d*|\.\d+))(%?)$/.exec(raw);
  if (!m) return undefined;
  const n = Number(m[1]);
  return m[2] === "%" ? (n / 100) * percentBasis : n;
}

/** Parses `oklch(L C H)` — egret's design tokens (Tailwind v4) are all written this way, and a
 * browser hands them back in the SAME space: `getComputedStyle().color` only serialises to
 * `rgb()` for legacy-sRGB colours, so a resolved `var(--foreground)` arrives here as
 * `oklch(...)`, not `rgb(...)`.
 *
 * Accepts both spellings of lightness (`oklch(14% ...)` and `oklch(0.14 ...)`), an omitted or
 * `none` chroma/hue (achromatic grey), an optional `deg` unit on the hue, and an optional
 * `/ alpha` which is ignored — the same way `rgba()`'s alpha already is. */
function parseOklchColor(color: string): [number, number, number] | undefined {
  const m = /^oklch\(\s*([^\s/)]+)(?:\s+([^\s/)]+))?(?:\s+([^\s/)]+))?\s*(?:\/\s*[^)]*)?\)$/i.exec(color.trim());
  if (!m) return undefined;
  const L = parseOklchComponent(m[1], 1);
  const C = parseOklchComponent(m[2], 0.4);
  // `%` is not actually valid on a <hue>, so the 360 basis is only a lenient reading of something
  // CSS would reject outright — chosen over returning `undefined` because a nonsense hue on an
  // otherwise-readable colour shouldn't cost the caller its lightness.
  const H = parseOklchComponent(m[3]?.replace(/deg$/i, ""), 360);
  if (L === undefined || C === undefined || H === undefined) return undefined;

  const rad = (H * Math.PI) / 180;
  // Out-of-gamut inputs are clamped per channel by `linearToSrgb`, so this always yields drawable
  // 0-255 channels rather than an overflow.
  return oklabToSrgb(L, C * Math.cos(rad), C * Math.sin(rad));
}

/** Parses `#rgb`/`#rrggbb`, `rgb()`/`rgba()` (alpha ignored) or `oklch()` to 0-255 channels;
 * anything else (a named CSS colour, an unresolved `var()`, ...) is unparseable. Covers what this
 * file's OWN outputs (SERIES_PALETTE, chartTint/chartShade, resolveColor) can ever produce, plus what a
 * real browser's `getComputedStyle` returns for a resolved `var()` — which for egret's tokens is
 * `oklch(...)`, and only for a legacy colour `rgb(r, g, b)`. Not a general CSS colour parser.
 *
 * Exported so its contract can be asserted directly: the `oklch()` branch is the whole reason
 * `getTextColorForBackground` works at all in dark mode, and testing it only through that
 * function's chosen-candidate output is what let it stay broken. */
export function parseColor(color: string): [number, number, number] | undefined {
  const hex = parseHexColor(color);
  if (hex) return hex;
  const m = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*[\d.]+\s*)?\)$/i.exec(color.trim());
  if (m) return [Number(m[1]), Number(m[2]), Number(m[3])];
  return parseOklchColor(color);
}

/**
 * The same colour at `alpha` opacity, written as `rgba(...)` — Metabase's
 * `Color(c).fade(f).rgb().string()` (pie/option.ts), used to de-emphasise a sunburst slice.
 *
 * The alpha has to live in the FILL COLOUR rather than in ECharts' `itemStyle.opacity`, which
 * is the whole reason this exists: `opacity` is applied to the sector's stroke as well, so the
 * background-coloured border that separates neighbouring slices goes translucent too and the
 * colour underneath leaks through it. Fading only the fill leaves the border opaque.
 *
 * Unparseable input comes back verbatim — the same "leave it alone" degradation `chartTint`
 * uses rather than throwing.
 */
export function fadeColor(color: string, alpha: number): string {
  const rgb = parseColor(color);
  if (!rgb) return color;
  return `rgba(${rgb[0]}, ${rgb[1]}, ${rgb[2]}, ${alpha})`;
}

/**
 * `color` at `weight`, the rest `backdrop` — a straight per-channel sRGB mix.
 *
 * For an OPAQUE backdrop this is exactly what painting `color` at `weight` opacity on top of it
 * produces, which is the point: it turns "the same ink, dimmer" into a CONCRETE colour. A
 * translucent `rgba(...)` has no contrast ratio at all until you say what is behind it, and every
 * caller here does know — so resolving it once, here, is what lets the result be checked (see
 * `colorContrast`) instead of taken on trust.
 *
 * `weight` is clamped to 0..1. Unparseable input comes back verbatim — the same "leave it alone"
 * degradation `chartTint` and `fadeColor` already use rather than throwing.
 */
/**
 * The colour as `#rrggbb`, for the one consumer that cannot take anything else: a native
 * `<input type="color">`, whose `value` must be a hex. A browser-resolved token comes back as
 * `oklch(...)`, which that input rejects (and then shows black). Unparseable input comes back
 * verbatim, the same degradation the other helpers here use.
 */
export function toHexColor(color: string): string {
  const rgb = parseColor(color);
  if (!rgb) return color;
  return `#${rgb.map((c) => Math.round(Math.min(255, Math.max(0, c))).toString(16).padStart(2, "0")).join("")}`;
}

export function mixColor(color: string, backdrop: string, weight: number): string {
  const fg = parseColor(color);
  const bg = parseColor(backdrop);
  if (!fg || !bg) return color;
  const t = Math.min(1, Math.max(0, weight));
  const [r, g, b] = fg.map((c, i) => Math.round(c * t + bg[i] * (1 - t)));
  return `rgb(${r}, ${g}, ${b})`;
}

function relativeLuminance(rgb: [number, number, number]): number {
  const [r, g, b] = rgb.map((c) => srgbToLinear(c / 255));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Standard WCAG contrast ratio between two colours' relative luminance (1:1 to 21:1). */
function contrastRatio(a: [number, number, number], b: [number, number, number]): number {
  const l1 = relativeLuminance(a);
  const l2 = relativeLuminance(b);
  const lighter = Math.max(l1, l2);
  const darker = Math.min(l1, l2);
  return (lighter + 0.05) / (darker + 0.05);
}

/**
 * The WCAG contrast ratio between two colours, written any way `parseColor` reads them (`#rgb`,
 * `rgb()`, `oklch()` — i.e. this file's own outputs and a browser's resolved tokens);
 * `undefined` if either is unreadable.
 *
 * Exported so that a design decision which RESTS on a contrast can be asserted as one. Pinning the
 * hexes such a decision happened to produce tests the arithmetic, not the property — and the
 * property is the whole reason the colours were chosen. `base.tooltip.test.ts` uses it to check the
 * tooltip's three ink levels against the real `--popover` of BOTH themes.
 */
export function colorContrast(a: string, b: string): number | undefined {
  const ca = parseColor(a);
  const cb = parseColor(b);
  return ca && cb ? contrastRatio(ca, cb) : undefined;
}

// Metabase's ui/colors/palette.ts getTextColorForBackground biases its pick toward white by a
// flat 3x factor on top of raw contrast ("We intentionally want to return white text color more
// frequently", per its own internal design notes) — the dark candidate loses even when it's the
// objectively better read, as long as light's contrast times 3 still edges it out.
//
// This port does NOT carry that bias over. Porting it faithfully was tried first and measurably
// made egret's own palette worse: for both `#509EE3` and `#A989C5`, dark text clears WCAG AA
// (4.5:1) comfortably — 6.20 and 5.97 against egret's real dark-text token — while the 3x bias
// picked white anyway, whose contrast against these fills is capped by the fill's OWN luminance
// (~0.32/0.30) at ~2.86/2.97, nowhere near 4.5. That's not a borderline call the bias nudged in
// a debatable direction; it moved a passing label to a failing one. The bias is a look
// preference Metabase made deliberately, not a legibility law, and 4.5:1 is egret's higher-
// priority rule, so where they conflict the bias loses.
//
// The rule implemented instead: pick whichever candidate actually has the higher raw contrast
// against the fill. Apply the white preference ONLY as a tie-break between two candidates that
// BOTH already clear 4.5:1 — at that point neither reading is at risk, so Metabase's white-
// leaning look is free to win. This keeps Metabase's look everywhere it was always safe
// (saturated dark fills, where white genuinely has the better raw contrast too — no tie-break
// even needed) and stops it exactly where it would have picked a fill-out label over a legible
// one.
const WCAG_AA_CONTRAST = 4.5;

/**
 * Picks whichever of `textColor` / `textColorInverse` reads better against `backgroundColor`,
 * with `textColorInverse` preferred only as a tie-break once both already clear WCAG AA (see the
 * comment above `WCAG_AA_CONTRAST`) — adapted from Metabase's `getTextColorForBackground`
 * (ui/colors/palette.ts), minus its 3x white bias. Reuses this file's own `srgbToLinear` (the
 * sRGB electro-optical transfer function, also used by `parseColor`'s oklch branch) rather than duplicating
 * the gamma maths for the WCAG relative-luminance formula.
 *
 * `backgroundColor` failing to parse (a CSS colour keyword, an unresolved `var()`, ...) falls
 * back to `textColor` — the same "leave it alone / pick the safe default" degradation this
 * file already uses elsewhere (e.g. `chartTint` on unparseable input) rather than throwing.
 */
export function getTextColorForBackground(
  backgroundColor: string,
  textColor: string,
  textColorInverse: string,
): string {
  const bg = parseColor(backgroundColor);
  if (!bg) return textColor;

  const fg = parseColor(textColor) ?? [0, 0, 0];
  const fgInverse = parseColor(textColorInverse) ?? [255, 255, 255];

  const darkContrast = contrastRatio(bg, fg);
  const whiteContrast = contrastRatio(bg, fgInverse);

  if (darkContrast >= WCAG_AA_CONTRAST && whiteContrast >= WCAG_AA_CONTRAST) {
    return textColorInverse; // both safe — Metabase's white-leaning look wins the tie-break
  }
  return whiteContrast > darkContrast ? textColorInverse : textColor;
}

/**
 * A thin outline in the OPPOSITE ink, in px. Two is the cartographic default for text over an
 * arbitrary fill: one is invisible at label sizes, three starts closing up the counters of the
 * glyphs it is meant to rescue.
 */
export const LABEL_HALO_WIDTH = 2;

/**
 * The halo a label needs when NEITHER candidate ink clears WCAG AA against its fill — ECharts'
 * `textBorderColor`/`textBorderWidth`, ready to spread into a `label`. `undefined` (no halo at
 * all) whenever the better ink already clears 4.5:1, which is every fill but two.
 *
 * `getTextColorForBackground` always returns SOMETHING, and on a fill both inks fail against, that
 * something is merely the least-bad of two illegible options. Measured across the whole
 * SERIES_PALETTE × three ring levels × both themes, exactly two fills are in that position:
 *
 *   - `#7172AD` (accent7, the indigo base) — best achievable 4.456 light / 4.468 dark. Fails in
 *     BOTH themes. Reached by any chart with 8+ series, not just a sunburst.
 *   - `#227FD2` (accent0-dark, the sunburst's inner blue) — 4.323 in light mode; clears in dark.
 *
 * THE PALETTE IS DELIBERATELY NOT THE FIX. Those hexes are Metabase's accents, matched on purpose
 * and asked for twice; nudging either one to buy contrast would break the match everywhere else it
 * is drawn. An outline leaves the fills untouched, engages ONLY where the contrast rule actually
 * fails, and reads the same way a map label over arbitrary terrain does. Metabase does not do this
 * — it fails AA on 27 of its own 53 sunburst labels, at worst 2.70:1 — so this is a place egret is
 * deliberately better rather than a parity gap.
 *
 * The outline is the ink the label did NOT take, so it is always the maximum available separation
 * from the glyph itself, and it flips with the theme exactly as the two candidates do.
 *
 * An unparseable `backgroundColor` gets no halo: `getTextColorForBackground` degrades to
 * `textColor` there without having measured anything, and an outline drawn on an unmeasured guess
 * would be decoration, not legibility.
 */
export function getTextHaloForBackground(
  backgroundColor: string,
  textColor: string,
  textColorInverse: string,
): { textBorderColor: string; textBorderWidth: number } | undefined {
  const bg = parseColor(backgroundColor);
  if (!bg) return undefined;

  const fg = parseColor(textColor) ?? [0, 0, 0];
  const fgInverse = parseColor(textColorInverse) ?? [255, 255, 255];

  // The BEST either ink can do — the same two ratios the picker compares, so the two functions
  // can never disagree about whether this fill was served.
  if (Math.max(contrastRatio(bg, fg), contrastRatio(bg, fgInverse)) >= WCAG_AA_CONTRAST) {
    return undefined;
  }
  const chosen = getTextColorForBackground(backgroundColor, textColor, textColorInverse);
  return {
    textBorderColor: chosen === textColorInverse ? textColor : textColorInverse,
    textBorderWidth: LABEL_HALO_WIDTH,
  };
}
