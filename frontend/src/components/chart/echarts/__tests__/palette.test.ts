import { describe, it, expect, vi, afterEach } from "vitest";
import { SERIES_PALETTE, CHART_THEME, seriesColor, resolveColor, resolveFontFamily, CHART_FONT_FALLBACK, chartShade, chartTint, ringVariantColor, getTextColorForBackground, getTextHaloForBackground, LABEL_HALO_WIDTH, parseColor, COLOR_FALLBACK, onThemeChange, toHexColor } from "../palette";

// egret's REAL theme tokens, copied verbatim from src/index.css (`:root` and `.dark`), together
// with the rgb a browser actually resolves each one to. Every number below was computed from the
// OKLCH spec and cross-checked against the values measured live in a running browser on a 3-ring
// sunburst — they are the ground truth these tests assert against, not values re-derived from the
// implementation under test.
const TOKENS = {
  light: {
    foreground: "oklch(0.21 0.11 263.16)", // rgb(0, 15, 73)
    background: "oklch(1 0 0)",            // rgb(255, 255, 255)
    foregroundHex: "#000f49",
    backgroundHex: "#ffffff",
  },
  dark: {
    foreground: "oklch(0.99 0 0)",         // rgb(252, 252, 252)
    background: "oklch(0.14 0 285.86)",    // rgb(9, 9, 9)
    card: "oklch(0.21 0.01 285.93)",       // rgb(24, 24, 29)
    foregroundHex: "#fcfcfc",
    backgroundHex: "#090909",
  },
} as const;

// Independent (deliberately duplicated) sRGB -> OKLab conversion, used ONLY to verify
// a colour under test from the outside — asserting on the hue/lightness the test itself
// computes, rather than trusting the implementation's own intermediate values.
function hexToRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.replace("#", ""), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
function srgbToLinear(c: number): number {
  const x = c / 255;
  return x <= 0.04045 ? x / 12.92 : Math.pow((x + 0.055) / 1.055, 2.4);
}
function toOklab(hex: string): { L: number; a: number; b: number } {
  const [r, g, b] = hexToRgb(hex).map(srgbToLinear);
  const l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
  const m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
  const s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
  const l_ = Math.cbrt(l), m_ = Math.cbrt(m), s_ = Math.cbrt(s);
  return {
    L: 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
    a: 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
    b: 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
  };
}
function oklch(hex: string): { L: number; C: number; H: number } {
  const { L, a, b } = toOklab(hex);
  return { L, C: Math.sqrt(a * a + b * b), H: Math.atan2(b, a) };
}
function hueDegrees(hex: string): number {
  const deg = (oklch(hex).H * 180) / Math.PI;
  return deg < 0 ? deg + 360 : deg;
}

/** Independent (deliberately duplicated) sRGB -> HSL, in the 0-360 / 0-100 / 0-100 units the
 * `color` package Metabase derives its accent variants with uses. Used only to check
 * chartShade/chartTint from the outside. */
function hsl(hex: string): { h: number; s: number; l: number } {
  const [r, g, b] = hexToRgb(hex).map((c) => c / 255);
  const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
  const l = (max + min) / 2;
  if (d === 0) return { h: 0, s: 0, l: l * 100 };
  const s = d / (1 - Math.abs(2 * l - 1));
  const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return { h: ((h * 60) % 360 + 360) % 360, s: s * 100, l: l * 100 };
}

/**
 * Every fill a sunburst can draw: each palette colour at each of Metabase's three ring levels
 * (inner = dark, middle = base, outer = light). This IS the set of label backgrounds the contrast
 * sweeps below have to cover — it replaced "the base plus 1 and 2 OKLCH lightening steps", which
 * was the shape of the ramp egret drew before adopting Metabase's rule.
 */
const RING_VARIANTS = [-1, 0, 1] as const;
const variantName = (v: number) => (v < 0 ? "dark" : v > 0 ? "light" : "base");
const sunburstFills = () =>
  SERIES_PALETTE.flatMap((base) =>
    RING_VARIANTS.map((v) => ({ base, variant: variantName(v), fill: ringVariantColor(base, v) })),
  );

describe("chart palette", () => {
  it("exposes Metabase's 8 accent colors in order", () => {
    expect(SERIES_PALETTE).toEqual([
      "#509EE3", "#88BF4D", "#A989C5", "#EF8C8C", "#F9D45C", "#F2A86F", "#98D9D9", "#7172AD",
    ]);
  });
  it("carries Metabase style constants", () => {
    expect(CHART_THEME.barWidth).toBe("80%");
    expect(CHART_THEME.areaOpacity).toBe(0.3);
    expect(CHART_THEME.symbolSize).toBe(6);
    expect(CHART_THEME.labelFontSize).toBe(13);
  });
  it("seriesColor cycles through the palette", () => {
    expect(seriesColor(0)).toBe("#509EE3");
    expect(seriesColor(8)).toBe("#509EE3"); // wraps
    expect(seriesColor(9)).toBe("#88BF4D");
  });
  describe("resolveColor", () => {
    it("returns concrete colors unchanged (no var)", () => {
      expect(resolveColor("#509EE3", "#000")).toBe("#509EE3");
      expect(resolveColor("rgb(1,2,3)", "#000")).toBe("rgb(1,2,3)");
    });
    it("falls back when a var() cannot be resolved (jsdom has no computed custom props)", () => {
      // jsdom's getComputedStyle does not resolve CSS custom properties, so the
      // fallback must be used — guaranteeing ECharts never receives a raw var().
      expect(resolveColor("var(--border)", "#e5e7eb")).toBe("#e5e7eb");
    });
    it("falls back for a value that isn't a colour at all, rather than painting nothing", () => {
      expect(resolveColor("not-a-real-color", "#000")).toBe("#000");
      expect(resolveColor("banana", "#000")).toBe("#000");
    });

    // The missing link that made the OKLCH bug invisible to every existing test: a real browser
    // does NOT normalise a resolved custom property to `rgb()`. `getComputedStyle().color`
    // serialises a non-legacy colour space in its OWN space (CSS Color 4 §serialization), so
    // egret's oklch() tokens come back as `oklch(...)` — which is exactly what every downstream
    // consumer then has to be able to read. jsdom resolves no custom properties at all, so under
    // vitest this path is only ever reached with the mock below; that gap is the reason 900+
    // green tests never saw a single oklch string.
    it("hands back the oklch() string a real browser returns for an oklch token, not rgb()", () => {
      const computed = vi.spyOn(window, "getComputedStyle");
      computed.mockReturnValue({ color: TOKENS.dark.foreground } as CSSStyleDeclaration);
      expect(resolveColor("var(--browser-oklch-probe)", "#fallback")).toBe(TOKENS.dark.foreground);
      computed.mockRestore();
    });
  });

  // parseColor is the shared "colour string -> channels" reader behind getTextColorForBackground's
  // WCAG maths and the ring variants' shading. It understood only hex and rgb(), so every egret
  // design token — all of which are oklch() (Tailwind v4) — was unparseable, and both callers
  // silently took their "can't read it" branch.
  describe("toHexColor", () => {
    // The waterfall's Total swatch (KI-057) hands a resolved token to a native colour input.
    it("writes a resolved oklch token as the hex a colour input accepts", () => {
      expect(toHexColor(TOKENS.light.foreground)).toBe("#000f49");
      expect(toHexColor(TOKENS.dark.foreground)).toBe("#fcfcfc");
      expect(toHexColor("#4C5773")).toBe("#4c5773");
    });
    it("returns what it cannot read unchanged", () => {
      expect(toHexColor("var(--foreground)")).toBe("var(--foreground)");
    });
  });

  describe("parseColor", () => {
    it("reads the live theme tokens to the rgb a browser resolves them to", () => {
      expect(parseColor(TOKENS.dark.foreground)).toEqual([252, 252, 252]);
      expect(parseColor(TOKENS.dark.background)).toEqual([9, 9, 9]);
      expect(parseColor(TOKENS.dark.card)).toEqual([24, 24, 29]);
      expect(parseColor(TOKENS.light.foreground)).toEqual([0, 15, 73]);
      expect(parseColor(TOKENS.light.background)).toEqual([255, 255, 255]);
    });

    it("reads percent lightness the same as 0-1 lightness", () => {
      // Both forms are legal CSS and both appear in the wild (hand-written tokens vs. browser
      // serialisation), so neither may be the only one that works.
      expect(parseColor("oklch(99% 0 0)")).toEqual([252, 252, 252]);
      expect(parseColor("oklch(14% 0 285.86)")).toEqual([9, 9, 9]);
      expect(parseColor("oklch(99% 0 0)")).toEqual(parseColor("oklch(0.99 0 0)"));
      expect(parseColor("oklch(21% 0.11 263.16)")).toEqual(parseColor(TOKENS.light.foreground));
    });

    it("treats an omitted, `none`, or zero chroma/hue as an achromatic grey", () => {
      const grey = parseColor("oklch(0.99 0 0)");
      expect(parseColor("oklch(0.99)")).toEqual(grey);
      expect(parseColor("oklch(0.99 0)")).toEqual(grey);
      expect(parseColor("oklch(0.99 none none)")).toEqual(grey);
      expect(parseColor("oklch(99%)")).toEqual(grey);
      // a hue with no chroma is still grey — the hue has nothing to tint
      expect(parseColor("oklch(0.99 0 285.86)")).toEqual(grey);
    });

    it("ignores the alpha component and a `deg` hue unit", () => {
      expect(parseColor("oklch(0.14 0 285.86 / 0.5)")).toEqual([9, 9, 9]);
      expect(parseColor("oklch(0.14 0 285.86 / 50%)")).toEqual([9, 9, 9]);
      expect(parseColor("oklch(0.21 0.11 263.16deg)")).toEqual([0, 15, 73]);
    });

    it("clamps an out-of-gamut oklch into sRGB rather than overflowing a channel", () => {
      // Chroma far beyond what sRGB can represent at this lightness.
      const rgb = parseColor("oklch(0.6 0.4 29.23)");
      expect(rgb).toBeDefined();
      for (const c of rgb!) {
        expect(c).toBeGreaterThanOrEqual(0);
        expect(c).toBeLessThanOrEqual(255);
        expect(Number.isInteger(c)).toBe(true);
      }
    });

    // Independent check of the new inverse conversion, using this file's OWN separately-written
    // forward sRGB -> OKLab transform (above): a round trip through both must land back on the
    // lightness it started from. A transposed or mistyped constant in the inverse matrix would
    // move L and fail here even though every hard-coded expectation above was also re-derived
    // from the same (wrong) code.
    it("round-trips through the independently-written forward OKLab transform", () => {
      const toHex = (rgb: [number, number, number]) =>
        `#${rgb.map((c) => c.toString(16).padStart(2, "0")).join("")}`;
      for (const L of [0.14, 0.35, 0.6, 0.85, 0.99]) {
        const rgb = parseColor(`oklch(${L} 0 0)`)!;
        expect(oklch(toHex(rgb)).L).toBeCloseTo(L, 2);
      }
      // and a chromatic one keeps its hue as well as its lightness
      const red = parseColor("oklch(0.628 0.2577 29.23)")!;
      expect(oklch(toHex(red)).L).toBeCloseTo(0.628, 2);
      expect(hueDegrees(toHex(red))).toBeCloseTo(29.23, 0);
    });

    it("still reads hex and rgb()/rgba(), which it always could", () => {
      expect(parseColor("#509EE3")).toEqual([80, 158, 227]);
      expect(parseColor("#fff")).toEqual([255, 255, 255]);
      expect(parseColor("rgb(80, 158, 227)")).toEqual([80, 158, 227]);
      expect(parseColor("rgba(80,158,227,0.5)")).toEqual([80, 158, 227]);
    });

    it("reports anything it genuinely cannot read as unparseable", () => {
      expect(parseColor("var(--foreground)")).toBeUndefined();
      expect(parseColor("rebeccapurple")).toBeUndefined();
      expect(parseColor("not-a-color")).toBeUndefined();
      expect(parseColor("oklch()")).toBeUndefined();
      expect(parseColor("oklch(banana 0 0)")).toBeUndefined();
    });
  });

  // The resolved-colour cache is module-level and lives as long as the page does, so without
  // invalidation every var()-backed colour (label text, ring border, donut total, axis labels —
  // cartesian, waterfall, boxplot and gauge all go through the same function) keeps whichever
  // theme's value happened to be resolved first until a full reload.
  describe("theme changes", () => {
    afterEach(() => {
      vi.restoreAllMocks();
      document.documentElement.classList.remove("dark");
    });

    // ThemeProvider swaps the `light`/`dark` class on <html>, which is what the `.dark` variant
    // in index.css keys off — so that class attribute IS the theme signal.
    const flipTheme = async () => {
      document.documentElement.classList.toggle("dark");
      // MutationObserver records are delivered in a microtask, not synchronously.
      await new Promise((r) => setTimeout(r, 0));
    };

    it("re-resolves a var() colour after the theme class on <html> changes", async () => {
      const computed = vi.spyOn(window, "getComputedStyle");
      computed.mockReturnValue({ color: "rgb(17, 24, 39)" } as CSSStyleDeclaration);
      expect(resolveColor("var(--test-fg)", "#fallback")).toBe("rgb(17, 24, 39)");

      computed.mockReturnValue({ color: "rgb(250, 250, 250)" } as CSSStyleDeclaration);
      // Still the light-theme value: that's the cache doing its job, not the bug.
      expect(resolveColor("var(--test-fg)", "#fallback")).toBe("rgb(17, 24, 39)");

      await flipTheme();
      expect(resolveColor("var(--test-fg)", "#fallback")).toBe("rgb(250, 250, 250)");
    });

    // The font is read from `--font-sans` exactly the way the colours are read from their tokens,
    // so it needs the same invalidation: a theme is free to restate the token, and a cached answer
    // would otherwise outlive the theme that produced it. This is also the only place the
    // token-reading branch runs at all — jsdom resolves no custom property, so every other test in
    // the suite (and every chart rendered under vitest) takes the fallback.
    it("reads the app's font token, caches it, and re-reads it after a theme change", async () => {
      const computed = vi.spyOn(window, "getComputedStyle");
      computed.mockReturnValue({ fontFamily: "Inter, sans-serif" } as CSSStyleDeclaration);
      await flipTheme(); // drop whatever earlier tests resolved
      expect(resolveFontFamily()).toBe("Inter, sans-serif");

      computed.mockReturnValue({ fontFamily: "Some Other Face, serif" } as CSSStyleDeclaration);
      // Still the previous answer: the cache doing its job, not the bug.
      expect(resolveFontFamily()).toBe("Inter, sans-serif");

      await flipTheme();
      expect(resolveFontFamily()).toBe("Some Other Face, serif");
    });

    it("falls back to the product's font when the token cannot be resolved", async () => {
      const computed = vi.spyOn(window, "getComputedStyle");
      // What jsdom actually does with a custom property: hands the literal straight back.
      computed.mockReturnValue({ fontFamily: "var(--font-sans)" } as CSSStyleDeclaration);
      await flipTheme();
      expect(resolveFontFamily()).toBe(CHART_FONT_FALLBACK);
    });

    it("notifies subscribers so a live chart can repaint, and stops after unsubscribe", async () => {
      const listener = vi.fn();
      const off = onThemeChange(listener);
      await flipTheme();
      expect(listener).toHaveBeenCalledTimes(1);
      off();
      await flipTheme();
      expect(listener).toHaveBeenCalledTimes(1);
    });
  });

  // These three are NOT derived from toOklab/oklch above — they're hard-coded from an external
  // source, specifically so a shared transcription bug in the matrix constants (present in BOTH
  // palette.ts and this file's "independent" toOklab) can still be caught. Values from Björn
  // Ottosson's own OKLab reference (https://bottosson.github.io/posts/oklab/, "the sRGB color
  // #ff0000 is converted to..." worked example + the same computation for pure green/blue),
  // cross-checked against the oklch.com converter.
  it("the (deliberately duplicated) reference OKLab conversion matches an external, hard-coded source", () => {
    const check = (hex: string, L: number, C: number, hDeg: number) => {
      const got = oklch(hex);
      expect(got.L).toBeCloseTo(L, 3);
      expect(got.C).toBeCloseTo(C, 3);
      expect(hueDegrees(hex)).toBeCloseTo(hDeg, 1);
    };
    check("#FF0000", 0.628, 0.2577, 29.23);
    check("#00FF00", 0.8664, 0.2948, 142.5);
    check("#0000FF", 0.452, 0.3132, 264.05);
  });

  // ------------------------------------------------------------------------------------------
  // The ring variants. These replaced `lightenOklch`, which lightened OUTWARD from the base in
  // OKLCH (deliberately preserving chroma) and so made egret's sunburst read as base -> lighter
  // -> lightest where Metabase reads dark -> base -> light. Measured side by side: egret's ring-1
  // blue was #76C4FF against Metabase's #87BCEC, and its innermost ring was the medium base where
  // Metabase's is the deep accent0-dark.
  //
  // The port is checked against Metabase's OWN numbers rather than described in prose: the table
  // below is every accent variant read out of a running Metabase, and `chartShade`/`chartTint`
  // have to reproduce all 24 of them from the eight bases alone. That is what makes this a port
  // and not an approximation — an OKLCH-space or a differently-rounded HSL implementation lands
  // near these values but not ON them.
  describe("chartShade / chartTint — Metabase's accent variants", () => {
    /** [base, dark (shade), light (tint)] — measured from a running Metabase's CSS variables. */
    const METABASE_ACCENTS: [string, string, string][] = [
      ["#509EE3", "#227FD2", "#87BCEC"],
      ["#88BF4D", "#689636", "#A7D07C"],
      ["#A989C5", "#8A5EB0", "#C8B4DA"],
      ["#EF8C8C", "#E75454", "#F7C4C4"],
      ["#F9D45C", "#F7C41F", "#FBE499"],
      ["#F2A86F", "#ED8535", "#F7CBA9"],
      ["#98D9D9", "#69C8C8", "#C7EAEA"],
      ["#7172AD", "#51528D", "#999AC4"],
    ];

    it("the eight bases under test really are egret's own palette, in order", () => {
      // Otherwise the table below would be checking a derivation against colours egret doesn't use.
      expect(METABASE_ACCENTS.map(([base]) => base)).toEqual(SERIES_PALETTE);
    });

    it("reproduces every one of Metabase's 24 accent hexes", () => {
      for (const [base, dark, light] of METABASE_ACCENTS) {
        expect(chartShade(base).toUpperCase(), `${base} -> dark`).toBe(dark);
        expect(chartTint(base).toUpperCase(), `${base} -> light`).toBe(light);
      }
    });

    // The rule behind the table, so a future colour that isn't in it is still covered. Tolerances
    // are the cost of the 8-bit round trip through a hex string (the derived colour is re-read
    // from 24-bit sRGB, not from the exact HSL triple), not slack in the rule.
    it("moves HSL lightness by Metabase's ±12.5 points, leaving hue and saturation alone", () => {
      for (const [base] of METABASE_ACCENTS) {
        const b = hsl(base);
        for (const [variant, delta] of [[chartShade(base), -12.5], [chartTint(base), 12.5]] as const) {
          const v = hsl(variant);
          expect(Math.abs(v.l - (b.l + delta)), `${base} -> ${variant} lightness`).toBeLessThan(1);
          expect(Math.abs(v.h - b.h), `${base} -> ${variant} hue`).toBeLessThan(1);
          expect(Math.abs(v.s - b.s), `${base} -> ${variant} saturation`).toBeLessThan(2);
        }
      }
    });

    it("returns a concrete colour string, never a var() the SVG renderer can't resolve", () => {
      expect(chartTint(SERIES_PALETTE[0])).toMatch(/^#[0-9a-f]{6}$/i);
      expect(chartShade(SERIES_PALETTE[0])).toMatch(/^#[0-9a-f]{6}$/i);
    });

    it("returns an unparseable input unchanged rather than throwing", () => {
      for (const fn of [chartShade, chartTint]) {
        expect(() => fn("var(--foreground)")).not.toThrow();
        expect(fn("var(--foreground)")).toBe("var(--foreground)");
        expect(fn("not-a-color")).toBe("not-a-color");
      }
    });

    // Inherited from lightenOklch, and a live requirement rather than a theoretical one: the
    // "Other" slice's colour is `var(--muted-foreground)`, which a real browser resolves to an
    // oklch() string. A variant function that only understood hex would hand every ring nested
    // under Other the same flat grey — the exact bug that shipped once already.
    it("reads a browser-resolved oklch()/rgb() colour, not just a hex literal", () => {
      const mutedDark = "oklch(0.71 0.01 286.09)"; // --muted-foreground, dark theme
      expect(chartTint(mutedDark)).toMatch(/^#[0-9a-f]{6}$/i);
      expect(hsl(chartTint(mutedDark)).l).toBeGreaterThan(hsl(chartShade(mutedDark)).l);
      // identical input, two spellings -> identical output
      expect(chartTint("rgb(107, 114, 128)")).toBe(chartTint("#6b7280"));
    });

    it("clamps at the ends of the lightness range instead of wrapping past them", () => {
      expect(chartTint("#ffffff")).toBe("#ffffff");
      expect(chartShade("#000000")).toBe("#000000");
      // ...and a near-white still moves, so the clamp isn't just freezing everything pale
      expect(chartTint("#f0f0f0")).not.toBe("#f0f0f0");
    });

    // Every fill a sunburst can now produce has to stay a distinct, visible colour: three levels
    // of the same hue that a reader can tell apart at a glance.
    it("keeps each ring's fill visibly distinct from its neighbours", () => {
      for (const base of SERIES_PALETTE) {
        const [dark, light] = [chartShade(base), chartTint(base)];
        expect(new Set([dark, base, light]).size, base).toBe(3);
        expect(hsl(dark).l, base).toBeLessThan(hsl(base).l);
        expect(hsl(light).l, base).toBeGreaterThan(hsl(base).l);
      }
    });
  });

  // getTextColorForBackground (fix 2 — WCAG AA label contrast): ported from Metabase's
  // ui/colors/palette.ts, but NOT with Metabase's 3x white-text bias — see the correction at
  // the top of palette.ts's getTextColorForBackground for why. Verification here is
  // INDEPENDENT of the implementation's own contrast maths — a second, separately-written WCAG
  // relative-luminance/contrast formula, so a bug shared between the "real" code and its own
  // self-check couldn't hide.
  describe("getTextColorForBackground", () => {
    const DARK_TEXT = "#000000";
    const LIGHT_TEXT = "#ffffff";

    function hexToRgb(hex: string): [number, number, number] {
      const n = parseInt(hex.replace("#", ""), 16);
      return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
    }
    function channelToLinear(c: number): number {
      const x = c / 255;
      return x <= 0.03928 ? x / 12.92 : Math.pow((x + 0.055) / 1.055, 2.4);
    }
    function relativeLuminance(hex: string): number {
      const [r, g, b] = hexToRgb(hex).map(channelToLinear);
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    }
    function wcagContrast(hexA: string, hexB: string): number {
      const l1 = relativeLuminance(hexA);
      const l2 = relativeLuminance(hexB);
      const lighter = Math.max(l1, l2);
      const darker = Math.min(l1, l2);
      return (lighter + 0.05) / (darker + 0.05);
    }

    // The two real failing colours measured live on a 3-ring sunburst (task write-up). Under
    // the OLD ported 3x-bias, both got LIGHT text whose contrast is capped by the background's
    // OWN luminance (~0.30/~0.32) at ~2.9-3.0 — a regression from egret's PRE-port fixed dark
    // label (rgb(51,51,51) ≈ 4.42/4.26, itself only a hair over AA). The corrected rule picks
    // whichever candidate actually clears 4.5, so both now get dark text, verified independently
    // below at ~7.34/7.07 (against plain black) — comfortably clear, not a borderline pass.
    it("a saturated purple fill (#A989C5) gets dark text that actually clears 4.5:1", () => {
      const chosen = getTextColorForBackground("#A989C5", DARK_TEXT, LIGHT_TEXT);
      expect(chosen).toBe(DARK_TEXT);
      const ratio = wcagContrast("#A989C5", chosen);
      expect(ratio).toBeGreaterThanOrEqual(4.5);
      expect(ratio).toBeCloseTo(7.072, 2);
      expect(wcagContrast("#A989C5", LIGHT_TEXT)).toBeLessThan(4.5); // confirms light really was the worse pick

      // Also check egret's REAL production dark token (COLOR_FALLBACK.text, "#111827" — what
      // pie.ts actually resolves `var(--foreground)` to), not just the test's plain "#000000"
      // stand-in: it clears 4.5 too (~5.97, per the task write-up), so the token this ships
      // with is dark enough for the job, not merely "darker than white".
      expect(wcagContrast("#A989C5", COLOR_FALLBACK.text)).toBeGreaterThanOrEqual(4.5);
      expect(wcagContrast("#A989C5", COLOR_FALLBACK.text)).toBeCloseTo(5.974, 2);
    });

    it("a mid blue fill (#509EE3) gets dark text that actually clears 4.5:1", () => {
      const chosen = getTextColorForBackground("#509EE3", DARK_TEXT, LIGHT_TEXT);
      expect(chosen).toBe(DARK_TEXT);
      const ratio = wcagContrast("#509EE3", chosen);
      expect(ratio).toBeGreaterThanOrEqual(4.5);
      expect(ratio).toBeCloseTo(7.341, 2);
      expect(wcagContrast("#509EE3", LIGHT_TEXT)).toBeLessThan(4.5); // confirms light really was the worse pick

      // Same real-token check as above, for this fill.
      expect(wcagContrast("#509EE3", COLOR_FALLBACK.text)).toBeGreaterThanOrEqual(4.5);
      expect(wcagContrast("#509EE3", COLOR_FALLBACK.text)).toBeCloseTo(6.201, 2);
    });

    it("a light, already-passing fill keeps dark text", () => {
      // #B9F381 — the outer-ring green from the write-up, measured 9.75 against dark text.
      const chosen = getTextColorForBackground("#B9F381", DARK_TEXT, LIGHT_TEXT);
      expect(chosen).toBe(DARK_TEXT);
      expect(wcagContrast("#B9F381", chosen)).toBeGreaterThanOrEqual(4.5);
    });

    // Proves the fix didn't just become "always pick dark": a genuinely dark, saturated fill
    // (midnight blue) where white's raw contrast is overwhelmingly better still gets white.
    // #191970: contrast to white ≈14.85, contrast to plain black ≈1.41 — dark doesn't even come
    // close, so this isn't a tie-break case at all, just "pick whichever actually reads" doing
    // its job on the (still very real) side of the palette where Metabase's white-leaning look
    // was always correct.
    it("a genuinely dark, saturated fill still gets white text", () => {
      const hex = "#191970"; // midnight blue
      const rawWhite = wcagContrast(hex, LIGHT_TEXT);
      const rawDark = wcagContrast(hex, DARK_TEXT);
      expect(rawWhite).toBeGreaterThan(rawDark); // white genuinely wins on raw contrast
      expect(rawWhite).toBeGreaterThanOrEqual(4.5);

      const chosen = getTextColorForBackground(hex, DARK_TEXT, LIGHT_TEXT);
      expect(chosen).toBe(LIGHT_TEXT);
    });

    // The white tie-break is load-bearing: find (by computing, not guessing) a fill where BOTH
    // candidates already clear 4.5:1 AND raw contrast alone would favour dark — so if the
    // tie-break weren't wired in, "pick the higher-contrast candidate" would choose dark here.
    // #767676: contrast to black ≈4.623, contrast to white ≈4.542 — both clear, dark is
    // numerically ahead, so this is exactly the "both safe, prefer the safer-look one" case the
    // tie-break exists for.
    it("the white tie-break wins when both candidates already clear 4.5:1, even though dark has the higher raw contrast", () => {
      const hex = "#767676";
      const rawDark = wcagContrast(hex, DARK_TEXT);
      const rawWhite = wcagContrast(hex, LIGHT_TEXT);
      expect(rawDark).toBeGreaterThanOrEqual(4.5);
      expect(rawWhite).toBeGreaterThanOrEqual(4.5);
      expect(rawDark).toBeGreaterThan(rawWhite); // raw contrast alone would pick dark here

      const chosen = getTextColorForBackground(hex, DARK_TEXT, LIGHT_TEXT);
      expect(chosen).toBe(LIGHT_TEXT); // tie-break overrides the raw-contrast winner
    });

    it("falls back to the dark candidate for a background it cannot parse, rather than throwing", () => {
      expect(() => getTextColorForBackground("not-a-color", DARK_TEXT, LIGHT_TEXT)).not.toThrow();
      expect(getTextColorForBackground("not-a-color", DARK_TEXT, LIGHT_TEXT)).toBe(DARK_TEXT);
    });

    // The real set of backgrounds a sunburst pie produces: every SERIES_PALETTE colour at each of
    // Metabase's three ring levels — dark (inner), base (middle), light (outer). Every one of
    // these must get a label colour that actually clears 4.5:1 against SOME candidate — if one
    // can't, that's a gap in SERIES_PALETTE itself, not something this function can paper over by
    // picking harder.
    it("every SERIES_PALETTE fill a sunburst can produce (dark, base, light) gets a label clearing 4.5:1 — or is reported by name", () => {
      const failures = sunburstFills()
        .map(({ base, variant, fill }) => {
          const dark = wcagContrast(fill, COLOR_FALLBACK.text);
          const white = wcagContrast(fill, COLOR_FALLBACK.textInverse);
          return { base, variant, fill, dark, white, best: Math.max(dark, white) };
        })
        .filter((r) => r.best < 4.5);

      // Ground rule: report a real gap by name and ratio rather than loosening this assertion.
      // TWO now, and the first one is NEW — introduced by adopting Metabase's dark inner ring:
      //   #227FD2 (accent0-dark, the inner blue): best ≈4.253 against these non-DOM fallback
      //     candidates, near-black. It was #509EE3 (≈6.2) while egret coloured its inner ring
      //     with the base. Reported rather than quietly shipped; see the both-themes sweep below
      //     for what it measures against the REAL tokens (≈4.32 light, and it clears AA in dark).
      //   #7172AD (indigo) at the base level: best ≈4.456, white — pre-existing, and inherent to
      //     the 8th accent colour rather than to anything this function or the ring rule does.
      // For scale: Metabase's own sunburst fails AA on 27 of its 53 labels, worst 2.70:1.
      const summary = failures
        .map((f) => `${f.base} ${f.variant} (${f.fill}): dark=${f.dark.toFixed(3)} white=${f.white.toFixed(3)} best=${f.best.toFixed(3)}`)
        .join("; ");
      expect(failures.length, `unexpected AA gap(s): ${summary}`).toBe(2);
      expect(failures.map((f) => f.fill), summary).toEqual(["#227fd2", "#7172AD"]);
      expect(failures[0].best, summary).toBeCloseTo(4.253, 2);
      expect(failures[1].best, summary).toBeCloseTo(4.456, 2);
    });

    // ---- The theme half of the story -------------------------------------------------------
    //
    // Everything above this line hands the function two HEX candidates. Production never does:
    // pie.ts passes `resolveColor("var(--foreground)")` / `var(--background)`, which in a real
    // browser are oklch() strings. Measured live on a 3-ring sunburst before the fix: light mode
    // 0/51 labels failing (worst 6.07) but dark mode 51/51 failing (worst 1.26) — and in BOTH
    // themes all 51 labels came back the SAME colour, because neither candidate parsed and the
    // comparison silently ran against hard-coded black/white defaults instead. Light mode only
    // looked right because that accidental black happened to suit a light palette.
    //
    // The fills themselves do NOT change with the theme (they're the fixed Metabase palette), so
    // the correct answer in dark mode is still near-BLACK ink on most slices — i.e. the
    // `--background` candidate, not the `--foreground` one.

    const themeCandidates = {
      light: { text: TOKENS.light.foreground, inverse: TOKENS.light.background, textHex: TOKENS.light.foregroundHex, inverseHex: TOKENS.light.backgroundHex },
      dark: { text: TOKENS.dark.foreground, inverse: TOKENS.dark.background, textHex: TOKENS.dark.foregroundHex, inverseHex: TOKENS.dark.backgroundHex },
    } as const;

    it("picks the near-black token for a mid-blue slice in DARK mode, where it used to pick near-white", () => {
      const { text, inverse } = themeCandidates.dark;
      const chosen = getTextColorForBackground("#509EE3", text, inverse);
      expect(chosen).toBe(inverse); // --background, rgb(9,9,9)
      expect(wcagContrast("#509EE3", TOKENS.dark.backgroundHex)).toBeGreaterThanOrEqual(4.5);
      // the token it used to return, and why that was the defect: 2.79 against this fill
      expect(wcagContrast("#509EE3", TOKENS.dark.foregroundHex)).toBeLessThan(4.5);
    });

    // Assertion 3 — the one whose absence let "one colour for every slice" through. A per-slice
    // picker that returns an identical colour for a light fill and a dark fill is not picking.
    it("returns DIFFERENT colours for a light fill and a dark fill, in either theme", () => {
      for (const theme of ["light", "dark"] as const) {
        const { text, inverse } = themeCandidates[theme];
        const onPaleYellow = getTextColorForBackground("#F9D45C", text, inverse);
        const onMidnightBlue = getTextColorForBackground("#191970", text, inverse);
        expect(onPaleYellow, theme).not.toBe(onMidnightBlue);
        // and specifically: dark ink on the pale fill, light ink on the dark fill — in BOTH
        // themes, because the fills don't change with the theme
        const paleHex = onPaleYellow === text ? themeCandidates[theme].textHex : themeCandidates[theme].inverseHex;
        const midnightHex = onMidnightBlue === text ? themeCandidates[theme].textHex : themeCandidates[theme].inverseHex;
        expect(wcagContrast("#F9D45C", paleHex), theme).toBeGreaterThanOrEqual(4.5);
        expect(wcagContrast("#191970", midnightHex), theme).toBeGreaterThanOrEqual(4.5);
      }
    });

    // The live symptom, restated as an assertion: on a real 3-ring sunburst all 51 labels came
    // back the SAME colour. The pick has to follow the FILL, not the theme — so a given fill gets
    // the same ink in light mode and in dark mode, since the fills themselves don't change with
    // the theme.
    //
    // RE-BASELINED BY THE RING RULE. While every ring lightened outward from the base, all 24
    // fills were mid-to-light and all 24 took the near-black candidate; the only interesting case
    // had to be imported (#191970). Metabase's rule paints the INNER ring dark, so three of the
    // fills are now genuinely dark and legitimately take light ink — which makes the exception
    // list itself the assertion: exactly the dark variants deep enough to need it, and nothing
    // else moved.
    it("picks by the FILL's lightness, not by the theme's — the same fill gets the same ink in both", () => {
      const fills = sunburstFills();
      for (const theme of ["light", "dark"] as const) {
        const { text, inverse, textHex, inverseHex } = themeCandidates[theme];
        // the near-black candidate in this theme
        const nearBlack = relativeLuminance(textHex) < relativeLuminance(inverseHex) ? text : inverse;
        const exceptions = fills
          .filter((f) => getTextColorForBackground(f.fill, text, inverse) !== nearBlack)
          .map((f) => f.fill);
        // #8A5EB0 (accent2-dark) and #51528D (accent7-dark) are dark enough that white genuinely
        // reads better on them — the picker doing its job, in both themes. #7172AD (the indigo
        // base) is the fill NEITHER candidate can serve: in light mode white is its marginally
        // better option (4.456 vs 4.047), so it joins them there; in dark mode the near-black
        // `--background` wins it outright (4.468 vs 4.344) and it does not.
        expect(exceptions, theme).toEqual(
          theme === "light" ? ["#8a5eb0", "#51528d", "#7172AD"] : ["#8a5eb0", "#51528d"],
        );
        // ...and every other fill takes near-black, so this is a picker, not a constant
        expect(new Set(fills.map((f) => getTextColorForBackground(f.fill, text, inverse))).size, theme).toBe(2);
      }
    });

    // Assertion 2 — the dark-mode sweep, mirroring the light-mode one above but with the REAL
    // oklch tokens on both sides instead of the non-DOM hex fallbacks. Runs both themes so the
    // light half can never again be the only one under test.
    it("every sunburst fill (dark, base, light) gets a label clearing 4.5:1 in BOTH themes — or is reported by name", () => {
      const gaps: string[] = [];
      let worst = { ratio: Infinity, where: "" };
      for (const theme of ["light", "dark"] as const) {
        const { text, inverse, textHex, inverseHex } = themeCandidates[theme];
        for (const { base, variant, fill } of sunburstFills()) {
          const chosen = getTextColorForBackground(fill, text, inverse);
          const chosenHex = chosen === text ? textHex : inverseHex;
          const achieved = wcagContrast(fill, chosenHex);
          const best = Math.max(wcagContrast(fill, textHex), wcagContrast(fill, inverseHex));

          // Whatever the palette can achieve, the picker must actually achieve it — a fill that
          // COULD clear AA but got the other candidate is a picker bug, not a palette gap.
          expect(achieved, `${theme} ${base} ${variant} (${fill}): picked ${chosen}`).toBeCloseTo(best, 6);
          if (achieved < worst.ratio) worst = { ratio: achieved, where: `${theme} ${base} ${variant} (${fill})` };
          if (best < 4.5) {
            gaps.push(`${theme} ${base} ${variant} (${fill}): best=${best.toFixed(3)}`);
          }
        }
      }

      // Same ground rule as the light-mode sweep: report a real palette gap by name and ratio
      // rather than loosening the assertion. THREE entries, two fills:
      //   - #227FD2 (accent0-dark), light mode only, ≈4.323 — NEW, and the cost of adopting
      //     Metabase's dark inner ring: this fill did not exist while egret coloured ring 0 with
      //     the base (#509EE3, ≈6.20). In dark mode its near-black `--background` clears AA.
      //   - #7172AD (the indigo base), both themes, ≈4.456 / ≈4.468 — pre-existing, inherent to
      //     the 8th accent (only reached at 8+ categories).
      // Nothing else in the 24 fills × 2 themes falls short, and the worst ratio anywhere is
      // ≈4.32:1 — against Metabase's own sunburst, which fails 27 of 53 labels at worst 2.70:1.
      const summary = gaps.join("; ");
      expect(gaps.length, `unexpected AA gap(s): ${summary}`).toBe(3);
      expect(gaps[0], summary).toContain("light #509EE3 dark (#227fd2)");
      expect(gaps[1], summary).toContain("light #7172AD base");
      expect(gaps[2], summary).toContain("dark #7172AD base");
      expect(wcagContrast("#227fd2", TOKENS.light.foregroundHex), summary).toBeCloseTo(4.323, 2);
      expect(wcagContrast("#7172AD", TOKENS.light.backgroundHex), summary).toBeCloseTo(4.456, 2);
      expect(wcagContrast("#7172AD", TOKENS.dark.backgroundHex), summary).toBeCloseTo(4.468, 2);
      // The headline number for the whole palette, stated once so a regression moves it.
      expect(worst.ratio, worst.where).toBeCloseTo(4.323, 2);
    });

    // Assertion 4 — the direct regression guard. If parseColor ever loses oklch(), BOTH candidates
    // become unparseable again and the function silently compares against hard-coded black/white,
    // returning `textColor` for every fill in the palette. These two assertions fail the moment
    // that happens: the first because a near-white fill would get near-white ink, the second
    // because an oklch FILL would take the "unparseable background" early return.
    it("regression: loses WCAG entirely if oklch() support is dropped", () => {
      const { text, inverse } = themeCandidates.dark;
      // near-white fill, dark theme: must get the near-black candidate, not the near-white one
      expect(getTextColorForBackground("#f5f5f5", text, inverse)).toBe(inverse);
      // an oklch FILL (a sliceColors override, or the muted "Other" slice a browser resolves to
      // oklch) must be read, not treated as unparseable
      expect(getTextColorForBackground("oklch(0.95 0.03 100)", text, inverse)).toBe(inverse);
      expect(getTextColorForBackground("oklch(0.2 0.05 264)", text, inverse)).toBe(text);
    });

    // ---- The two fills NO ink can serve ----------------------------------------------------
    //
    // The sweeps above end by REPORTING two fills rather than asserting them away: #7172AD
    // (accent7, best 4.456 light / 4.468 dark) and #227FD2 (accent0-dark, 4.323 light). The
    // picker does its job on both — it returns the better of the two inks — but "better" there
    // means the least-bad of two illegible options, and the label ships marginally unreadable.
    //
    // The palette is deliberately NOT the fix: those are Metabase's accent hexes, matched on
    // purpose. A 2px outline in the opposite ink is, and it must engage ONLY on the fills that
    // actually fail — a halo everywhere would be a look change to 45 labels that never needed one.
    describe("getTextHaloForBackground", () => {
      /** The best either ink can do, measured with THIS file's independently-written WCAG maths
       * rather than the implementation's — so "does this fill need a halo?" is answered from the
       * outside, and a bug shared between the code and its own self-check cannot hide. */
      const bestInk = (fill: string, textHex: string, inverseHex: string) =>
        Math.max(wcagContrast(fill, textHex), wcagContrast(fill, inverseHex));

      it("gives no halo at all to a fill one of the inks already clears", () => {
        const { text, inverse, textHex, inverseHex } = themeCandidates.light;
        // accent0 itself: dark ink reads at ~6.2 against it, so there is nothing to rescue.
        expect(bestInk("#509EE3", textHex, inverseHex)).toBeGreaterThanOrEqual(4.5);
        expect(getTextHaloForBackground("#509EE3", text, inverse)).toBeUndefined();
        // ...and with flat hex candidates too, so this is not an artefact of the oklch tokens:
        // a pale fill (dark ink wins) and a near-black one (light ink wins) both stay bare.
        expect(getTextHaloForBackground("#F9D45C", DARK_TEXT, LIGHT_TEXT)).toBeUndefined();
        expect(getTextHaloForBackground("#191970", DARK_TEXT, LIGHT_TEXT)).toBeUndefined();
      });

      it("outlines #7172AD in the ink the label did NOT take — in both themes, since it fails in both", () => {
        for (const theme of ["light", "dark"] as const) {
          const { text, inverse, textHex, inverseHex } = themeCandidates[theme];
          expect(bestInk("#7172AD", textHex, inverseHex), theme).toBeLessThan(4.5);
          const chosen = getTextColorForBackground("#7172AD", text, inverse);
          const halo = getTextHaloForBackground("#7172AD", text, inverse);
          expect(halo, theme).toEqual({
            textBorderColor: chosen === inverse ? text : inverse,
            textBorderWidth: LABEL_HALO_WIDTH,
          });
          expect(halo!.textBorderColor, theme).not.toBe(chosen); // the OPPOSITE ink, not the same one
        }
      });

      it("outlines #227FD2 in light mode, where it falls to 4.32 — and leaves it bare in dark, where it clears", () => {
        const light = themeCandidates.light;
        expect(bestInk("#227fd2", light.textHex, light.inverseHex)).toBeCloseTo(4.323, 2);
        const chosen = getTextColorForBackground("#227fd2", light.text, light.inverse);
        expect(chosen).toBe(light.text); // near-black is the better of the two, at 4.323
        expect(getTextHaloForBackground("#227fd2", light.text, light.inverse))
          .toEqual({ textBorderColor: light.inverse, textBorderWidth: LABEL_HALO_WIDTH });

        // In dark mode the near-black `--background` clears AA against this fill outright, so
        // there is nothing to outline — the same fill, a different answer, which is the whole
        // point of keying the halo off the measurement rather than off a list of hexes.
        const dark = themeCandidates.dark;
        expect(bestInk("#227fd2", dark.textHex, dark.inverseHex)).toBeGreaterThanOrEqual(4.5);
        expect(getTextHaloForBackground("#227fd2", dark.text, dark.inverse)).toBeUndefined();
      });

      it("flips the halo colour with the theme", () => {
        const l = getTextHaloForBackground("#7172AD", themeCandidates.light.text, themeCandidates.light.inverse);
        const d = getTextHaloForBackground("#7172AD", themeCandidates.dark.text, themeCandidates.dark.inverse);
        expect(l!.textBorderColor).toBe(TOKENS.light.foreground);
        expect(d!.textBorderColor).toBe(TOKENS.dark.foreground);
        // ...and that is a real flip in lightness, not merely a different token string: the same
        // fill gets a near-BLACK outline in light mode and a near-WHITE one in dark.
        expect(relativeLuminance(TOKENS.light.foregroundHex)).toBeLessThan(0.05);
        expect(relativeLuminance(TOKENS.dark.foregroundHex)).toBeGreaterThan(0.95);
      });

      // The whole surface, stated once: 8 accents × 3 ring levels × 2 themes = 48 label
      // backgrounds. The halo must appear on exactly the three that fail and on nothing else —
      // asserted as an IF-AND-ONLY-IF against the independent measurement, so both directions
      // are covered (a halo that spread everywhere fails, and so does one that stopped engaging).
      it("over SERIES_PALETTE × 3 ring levels × both themes: exactly the three failing fills take a halo", () => {
        const haloed: string[] = [];
        for (const theme of ["light", "dark"] as const) {
          const { text, inverse, textHex, inverseHex } = themeCandidates[theme];
          for (const { base, variant, fill } of sunburstFills()) {
            const best = bestInk(fill, textHex, inverseHex);
            const halo = getTextHaloForBackground(fill, text, inverse);
            expect(halo !== undefined, `${theme} ${base} ${variant} (${fill}): best=${best.toFixed(3)}`)
              .toBe(best < 4.5);
            if (halo) haloed.push(`${theme} ${fill}`);
          }
        }
        // #227FD2 is accent0's DARK variant (the sunburst's inner blue); #7172AD is accent7's own
        // base. Named in full so a palette or ring-rule change that moves the set has to say so.
        expect(haloed).toEqual(["light #227fd2", "light #7172AD", "dark #7172AD"]);
      });
    });
  });
});
