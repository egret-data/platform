import { describe, it, expect, vi, afterEach } from "vitest";
import { SERIES_PALETTE, CHART_THEME, seriesColor, resolveColor, lightenOklch, getTextColorForBackground, parseColor, COLOR_FALLBACK, onThemeChange } from "../palette";

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
// lightenOklch's output from the outside — asserting on the hue/lightness the test itself
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
  // WCAG maths and lightenOklch's ring shading. It understood only hex and rgb(), so every egret
  // design token — all of which are oklch() (Tailwind v4) — was unparseable, and both callers
  // silently took their "can't read it" branch.
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

  describe("lightenOklch", () => {
    const BLUE = SERIES_PALETTE[0]; // "#509EE3"
    const RED = "#CC3333";
    const GREEN = "#33AA33";

    it("raises perceived (OKLab) lightness", () => {
      const lightened = lightenOklch(BLUE, 1);
      expect(oklch(lightened).L).toBeGreaterThan(oklch(BLUE).L);
    });

    it("preserves hue — a red stays red, a green stays green", () => {
      const redHue = oklch(RED).H;
      const lightenedRedHue = oklch(lightenOklch(RED, 1)).H;
      expect(lightenedRedHue).toBeCloseTo(redHue, 2);

      const greenHue = oklch(GREEN).H;
      const lightenedGreenHue = oklch(lightenOklch(GREEN, 1)).H;
      expect(lightenedGreenHue).toBeCloseTo(greenHue, 2);

      // and the two stay distinct hues from each other
      expect(lightenedRedHue).not.toBeCloseTo(lightenedGreenHue, 1);
    });

    it("lightening by 0 returns the input unchanged", () => {
      expect(lightenOklch(BLUE, 0)).toBe(BLUE);
    });

    it("is monotonic — two steps are lighter than one", () => {
      const one = oklch(lightenOklch(BLUE, 1)).L;
      const two = oklch(lightenOklch(BLUE, 2)).L;
      expect(two).toBeGreaterThan(one);
    });

    it("returns a concrete colour string, never a var() the SVG renderer can't resolve", () => {
      const lightened = lightenOklch(BLUE, 1);
      expect(lightened).not.toContain("var(");
      expect(lightened).toMatch(/^#[0-9a-f]{6}$/i);
    });

    it("returns an unparseable input unchanged rather than throwing", () => {
      expect(() => lightenOklch("var(--foreground)", 1)).not.toThrow();
      expect(lightenOklch("var(--foreground)", 1)).toBe("var(--foreground)");
      expect(lightenOklch("not-a-color", 1)).toBe("not-a-color");
    });

    // The second casualty of the same OKLCH blindness, and a live one: the "Other" slice's colour
    // is `var(--muted-foreground)`, which a real browser resolves to an oklch() string — so every
    // ring nested under Other came back UNCHANGED from lightenOklch and the whole Other sub-tree
    // rendered as one flat grey. Under jsdom the same call gets COLOR_FALLBACK's hex instead and
    // lightens fine, which is precisely why no test ever saw it.
    it("lightens a browser-resolved oklch()/rgb() colour, not just a hex literal", () => {
      const mutedDark = "oklch(0.71 0.01 286.09)"; // --muted-foreground, dark theme
      const lightened = lightenOklch(mutedDark, 1);
      expect(lightened).toMatch(/^#[0-9a-f]{6}$/i);
      expect(oklch(lightened).L).toBeGreaterThan(0.71);

      const fromRgb = lightenOklch("rgb(107, 114, 128)", 1);
      expect(fromRgb).toMatch(/^#[0-9a-f]{6}$/i);
      expect(oklch(fromRgb).L).toBeGreaterThan(oklch("#6b7280").L);
      // identical input, two spellings -> identical output
      expect(fromRgb).toBe(lightenOklch("#6b7280", 1));
    });

    it("clamps rather than overflowing or wrapping when already near-white", () => {
      // A light grey whose OWN lightness (~0.845) is comfortably below the 0.9 cap, so 5 steps
      // (target ~1.45) unambiguously overshoots it — this proves the clamp actually ENGAGES,
      // rather than merely passing because the input was already near the cap (a no-op would
      // have satisfied a looser "didn't get darker" assertion without the clamp firing at all).
      const lightGrey = "#cccccc";
      expect(oklch(lightGrey).L).toBeLessThan(0.9);
      const lightened = lightenOklch(lightGrey, 5);
      expect(lightened).toMatch(/^#[0-9a-f]{6}$/i);
      const { L } = oklch(lightened);
      expect(L).toBeCloseTo(0.9, 1); // reaches the cap, doesn't overflow past it or wrap
    });

    // The bug this guards: chaining lightenOklch ring-over-ring (each hop re-parsing the
    // previous hop's 8-bit-rounded hex output) let per-channel gamut clipping compound, so a
    // mid-tone slice's colour collapsed to indistinguishable-from-white by the 3rd ring. pie.ts
    // no longer chains — it lightens each ring directly off a stable base colour in one call —
    // but this test guards the PRIMITIVE itself: every palette colour must still carry visible
    // chroma after 3 steps, however it gets called.
    it("every SERIES_PALETTE colour still has meaningful chroma after 3 lightening steps", () => {
      // Floor picked well above what the old chained bug produced (it collapsed as low as
      // ~0.0000373 and ~0.0034 for two palette colours — literally indistinguishable from
      // white) and comfortably below every colour's actual post-fix chroma (measured minimum
      // ~0.055, for the indigo "#7172AD") — so this fails on a real regression, not on noise.
      const CHROMA_FLOOR = 0.04;
      for (const color of SERIES_PALETTE) {
        const lightened = lightenOklch(color, 3);
        expect(oklch(lightened).C, `${color} -> ${lightened}`).toBeGreaterThan(CHROMA_FLOOR);
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

    // The real set of backgrounds a sunburst pie produces: every SERIES_PALETTE colour at ring
    // 0 (as shipped, unlightened), plus each one lightened 1 and 2 OKLCH steps for rings 1 and
    // 2 (pie.ts's actual max ring depth — see OKLCH_STEP_L's own comment: "at most 2 steps out
    // from its parent" for a 3-ring pie). Every one of these must get a label colour that
    // actually clears 4.5:1 against SOME candidate — if one can't, that's a gap in
    // SERIES_PALETTE itself, not something this function can paper over by picking harder.
    it("every SERIES_PALETTE fill a sunburst can produce (ring 0, +1 step, +2 steps) gets a label clearing 4.5:1 — or is reported by name", () => {
      const fills: { base: string; steps: number; fill: string }[] = [];
      for (const base of SERIES_PALETTE) {
        for (let steps = 0; steps <= 2; steps++) {
          fills.push({ base, steps, fill: steps === 0 ? base : lightenOklch(base, steps) });
        }
      }

      const failures = fills
        .map(({ base, steps, fill }) => {
          const dark = wcagContrast(fill, COLOR_FALLBACK.text);
          const white = wcagContrast(fill, COLOR_FALLBACK.textInverse);
          return { base, steps, fill, dark, white, best: Math.max(dark, white) };
        })
        .filter((r) => r.best < 4.5);

      // Ground rule: report a real gap by name and ratio rather than loosening this assertion.
      // #7172AD (indigo) ring 0 — the base colour itself, unlightened — is exactly such a gap:
      // best achievable is white at ≈4.456, just under AA; near-black does worse (≈3.98). This
      // is a genuine, pre-existing SERIES_PALETTE limitation (indigo is the 8th/last accent
      // colour, only reached with 8+ categories), not something introduced or fixable by this
      // function — documented here by name and ratio rather than silently excluded.
      const summary = failures
        .map((f) => `${f.base} +${f.steps} (${f.fill}): dark=${f.dark.toFixed(3)} white=${f.white.toFixed(3)} best=${f.best.toFixed(3)}`)
        .join("; ");
      expect(failures.length, `unexpected AA gap(s): ${summary}`).toBe(1);
      expect(failures[0].base, summary).toBe("#7172AD");
      expect(failures[0].steps, summary).toBe(0);
      expect(failures[0].best, summary).toBeCloseTo(4.456, 2);
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
    // back the SAME colour. Every SERIES_PALETTE fill (and every lightened ring variant) is
    // mid-to-light, so in EITHER theme they must all take the near-black candidate — which is
    // `--foreground` in light mode but `--background` in dark mode. Only a genuinely dark fill
    // takes the other one. Pre-fix, dark mode inverted this for all 24 sunburst fills at once.
    it("picks by the FILL's lightness, not by the theme's — the same fill gets the same ink in both", () => {
      const fills = [...SERIES_PALETTE, ...SERIES_PALETTE.map((c) => lightenOklch(c, 1)), ...SERIES_PALETTE.map((c) => lightenOklch(c, 2))];
      for (const theme of ["light", "dark"] as const) {
        const { text, inverse, textHex, inverseHex } = themeCandidates[theme];
        // the near-black candidate in this theme
        const nearBlack = relativeLuminance(textHex) < relativeLuminance(inverseHex) ? text : inverse;
        const exceptions = fills.filter((f) => getTextColorForBackground(f, text, inverse) !== nearBlack);
        // #7172AD at ring 0 is the one fill NEITHER candidate can serve (the AA gap the sweep
        // below reports by name). In light mode white is its marginally better option — 4.456 vs
        // 4.047 for this theme's near-black `--foreground` — so it is the sole legitimate
        // exception there; in dark mode the near-black `--background` wins it outright (4.468 vs
        // 4.344) and there is no exception at all. Every other fill takes near-black in both.
        expect(exceptions, theme).toEqual(theme === "light" ? ["#7172AD"] : []);
        // ...and a genuinely dark fill takes the OTHER one, so this is a picker, not a constant
        const onMidnight = getTextColorForBackground("#191970", text, inverse);
        expect(onMidnight, theme).not.toBe(nearBlack);
        expect(new Set([...fills.map((f) => getTextColorForBackground(f, text, inverse)), onMidnight]).size, theme).toBe(2);
      }
    });

    // Assertion 2 — the dark-mode sweep, mirroring the light-mode one above but with the REAL
    // oklch tokens on both sides instead of the non-DOM hex fallbacks. Runs both themes so the
    // light half can never again be the only one under test.
    it("every sunburst fill (ring 0, +1, +2) gets a label clearing 4.5:1 in BOTH themes — or is reported by name", () => {
      const gaps: string[] = [];
      for (const theme of ["light", "dark"] as const) {
        const { text, inverse, textHex, inverseHex } = themeCandidates[theme];
        for (const base of SERIES_PALETTE) {
          for (let steps = 0; steps <= 2; steps++) {
            const fill = steps === 0 ? base : lightenOklch(base, steps);
            const chosen = getTextColorForBackground(fill, text, inverse);
            const chosenHex = chosen === text ? textHex : inverseHex;
            const achieved = wcagContrast(fill, chosenHex);
            const best = Math.max(wcagContrast(fill, textHex), wcagContrast(fill, inverseHex));

            // Whatever the palette can achieve, the picker must actually achieve it — a fill that
            // COULD clear AA but got the other candidate is a picker bug, not a palette gap.
            expect(achieved, `${theme} ${base} +${steps} (${fill}): picked ${chosen}`).toBeCloseTo(best, 6);
            if (best < 4.5) {
              gaps.push(`${theme} ${base} +${steps} (${fill}): best=${best.toFixed(3)}`);
            }
          }
        }
      }

      // Same ground rule as the light-mode sweep: report a real palette gap by name and ratio
      // rather than loosening the assertion. #7172AD (indigo, the 8th accent — only reached at 8+
      // categories) at ring 0 is the ONLY fill that cannot clear AA, and it can't in either
      // theme: light best ≈4.456 (white), dark best ≈4.468 (near-black). Pre-existing and inherent
      // to SERIES_PALETTE, not something this function can fix by picking harder.
      const summary = gaps.join("; ");
      expect(gaps.length, `unexpected AA gap(s): ${summary}`).toBe(2);
      expect(gaps[0], summary).toContain("light #7172AD +0");
      expect(gaps[1], summary).toContain("dark #7172AD +0");
      expect(wcagContrast("#7172AD", TOKENS.light.backgroundHex), summary).toBeCloseTo(4.456, 2);
      expect(wcagContrast("#7172AD", TOKENS.dark.backgroundHex), summary).toBeCloseTo(4.468, 2);
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
  });
});
