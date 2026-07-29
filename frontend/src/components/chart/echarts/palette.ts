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

// The OKLab <-> sRGB pair below is the single copy of the colour-space maths in this file: it
// backs BOTH `lightenOklch` (which moves a colour up the L axis) and `parseColor`'s `oklch()`
// branch (which reads egret's design tokens). Conversion matrices are Björn Ottosson's OKLab
// (https://bottosson.github.io/posts/oklab/), §"Converting from linear sRGB to OKLab" and its
// inverse.

/** sRGB 0-255 channels -> OKLab `[L, a, b]`. */
function srgbToOklab(rgb: [number, number, number]): [number, number, number] {
  const [r, g, b] = rgb.map((c) => srgbToLinear(c / 255));

  // linear sRGB -> LMS -> LMS' (cube root)
  const l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
  const m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
  const s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
  const l_ = Math.cbrt(l);
  const m_ = Math.cbrt(m);
  const s_ = Math.cbrt(s);

  // LMS' -> OKLab
  return [
    0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
    1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
    0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
  ];
}

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

/**
 * Lightens a colour by `steps` in OKLab space, keeping chroma (C) and hue (H) fixed —
 * i.e. moves straight up the L axis of OKLCH. Used to give an outer pie ring's slice a
 * lighter variant of its parent slice's colour (Metabase's `getRingColorAlias`), rather than
 * an unrelated palette colour.
 *
 * Input goes through the shared `parseColor`, so a colour that arrived as a browser-resolved
 * token (`oklch(...)`) or as `rgb()` lightens exactly like a hex literal does. That matters
 * live rather than theoretically: the "Other" slice's colour is `var(--muted-foreground)`, which
 * `resolveColor` turns into an `oklch()` string in a real browser — while this only read hex,
 * every ring nested under Other came back unchanged and the whole sub-tree rendered one flat grey.
 *
 * `steps <= 0` and unparseable input both return `color` unchanged (verbatim, not
 * round-tripped) — the former so "no lightening" is exact, the latter so a caller passing
 * something this function can't handle (e.g. an unresolved `var(--x)`) degrades to "leave it
 * alone" rather than throwing.
 */
export function lightenOklch(color: string, steps: number): string {
  if (!Number.isFinite(steps) || steps <= 0) return color;
  const rgb = parseColor(color);
  if (!rgb) return color;

  const [L, a, b] = srgbToOklab(rgb);

  // Raise L only; a/b (so chroma + hue) are untouched — the whole point of doing this in
  // OKLab rather than a naive sRGB blend toward white, which desaturates and shifts hue.
  // Capped at OKLCH_MAX_L (see above), but never DARKENED below the input: a colour that
  // starts out brighter than the cap already (rare for this codebase's palette, but possible
  // for an arbitrary sliceColors override) is left at its own lightness rather than dimmed.
  const target = L + OKLCH_STEP_L * steps;
  const L2 = Math.max(L, Math.min(target, OKLCH_MAX_L));

  const toHex = (c: number) => c.toString(16).padStart(2, "0");
  return `#${oklabToSrgb(L2, a, b).map(toHex).join("")}`;
}

/** Concrete fallbacks (≈ egret light-theme tokens) for non-DOM contexts. */
export const COLOR_FALLBACK = {
  gridLine: "#e5e7eb",     // ≈ --border (very light gray)
  axisLabel: "#6b7280",    // ≈ --muted-foreground (slate)
  text: "#111827",         // ≈ --foreground (near-black)
  textInverse: "#ffffff",  // ≈ --background in light mode — the "opposite ink" for getTextColorForBackground
  card: "#ffffff",         // ≈ --card (charts render inside a Card, not the page background)
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
 * file's OWN outputs (SERIES_PALETTE, lightenOklch, resolveColor) can ever produce, plus what a
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
 * Unparseable input comes back verbatim — the same "leave it alone" degradation `lightenOklch`
 * uses rather than throwing.
 */
export function fadeColor(color: string, alpha: number): string {
  const rgb = parseColor(color);
  if (!rgb) return color;
  return `rgba(${rgb[0]}, ${rgb[1]}, ${rgb[2]}, ${alpha})`;
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
 * sRGB electro-optical transfer function, also used by `lightenOklch`) rather than duplicating
 * the gamma maths for the WCAG relative-luminance formula.
 *
 * `backgroundColor` failing to parse (a CSS colour keyword, an unresolved `var()`, ...) falls
 * back to `textColor` — the same "leave it alone / pick the safe default" degradation this
 * file already uses elsewhere (e.g. `lightenOklch` on unparseable input) rather than throwing.
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
