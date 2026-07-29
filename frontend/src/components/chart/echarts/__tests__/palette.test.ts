import { describe, it, expect } from "vitest";
import { SERIES_PALETTE, CHART_THEME, seriesColor, resolveColor, lightenOklch, getTextColorForBackground } from "../palette";

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
  // ui/colors/palette.ts. Verification here is INDEPENDENT of the implementation's own
  // contrast maths — a second, separately-written WCAG relative-luminance/contrast formula,
  // so a bug shared between the "real" code and its own self-check couldn't hide.
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

    // The two real failing colours measured live on a 3-ring sunburst (task write-up): both
    // fail WCAG AA (4.5:1) against the OLD fixed dark label colour egret used before this fix
    // (rgb(51,51,51), ECharts' own default — reproduced here as a sanity check that this test's
    // independent contrast maths agrees with the write-up's own measurements).
    //
    // IMPORTANT / found while writing this test: for BOTH of these colours, the ported 3x-bias
    // formula picks LIGHT text, but light text's contrast against them is capped by the
    // background's OWN luminance (~0.30/~0.32) — (Lwhite+0.05)/(Lbg+0.05) with Lwhite=1 tops out
    // around 2.9-3.0 for a background this bright, and no shade of "light" text can exceed that
    // ceiling. Dark text, by contrast, WOULD clear 4.5 here (verified below) — the 3x bias
    // overrides that in favour of the (objectively worse, still-failing) light option. This is
    // not a bug in this port: it reproduces bit-for-bit against Metabase's own real default theme
    // tokens (ui/colors/constants/base-colors.ts orion[80] for text-primary, "white" for
    // text-primary-inverse — verified by hand against Metabase's source for this write-up), so
    // Metabase's real pie chart has this same property for these same accent colours. Filed as a
    // finding rather than "fixed", since the task's instruction was to port the bias as-is, not
    // invent a corrective on top of it.
    it("a saturated purple fill (#A989C5): the bias still picks light even though dark would have cleared 4.5:1", () => {
      const chosen = getTextColorForBackground("#A989C5", DARK_TEXT, LIGHT_TEXT);
      expect(chosen).toBe(LIGHT_TEXT);
      expect(wcagContrast("#A989C5", DARK_TEXT)).toBeGreaterThanOrEqual(4.5); // the road not taken
      expect(wcagContrast("#A989C5", LIGHT_TEXT)).toBeLessThan(4.5); // what the bias actually picks
    });

    it("a mid blue fill (#509EE3): the bias still picks light even though dark would have cleared 4.5:1", () => {
      const chosen = getTextColorForBackground("#509EE3", DARK_TEXT, LIGHT_TEXT);
      expect(chosen).toBe(LIGHT_TEXT);
      expect(wcagContrast("#509EE3", DARK_TEXT)).toBeGreaterThanOrEqual(4.5);
      expect(wcagContrast("#509EE3", LIGHT_TEXT)).toBeLessThan(4.5);
    });

    it("a light, already-passing fill keeps dark text", () => {
      // #B9F381 — the outer-ring green from the write-up, measured 9.75 against dark text.
      const chosen = getTextColorForBackground("#B9F381", DARK_TEXT, LIGHT_TEXT);
      expect(chosen).toBe(DARK_TEXT);
      expect(wcagContrast("#B9F381", chosen)).toBeGreaterThanOrEqual(4.5);
    });

    // The 3x white-bias is load-bearing: find (by computing, not guessing) a fill whose RAW
    // contrast picks dark, but where the biased comparison flips to white. If no such colour
    // existed, that would mean the bias never actually changes an outcome — so this searches
    // for one and fails loudly, rather than asserting on a colour picked by eye, if none exists.
    it("the 3x white bias flips a borderline call that raw contrast would have given to dark text", () => {
      let found: { hex: string; rawDark: number; rawWhite: number } | undefined;
      for (let v = 0; v < 256 && !found; v++) {
        const hex = `#${[v, v, v].map((c) => c.toString(16).padStart(2, "0")).join("")}`;
        const rawDark = wcagContrast(hex, DARK_TEXT);
        const rawWhite = wcagContrast(hex, LIGHT_TEXT);
        if (rawDark > rawWhite && rawWhite * 3 > rawDark) {
          found = { hex, rawDark, rawWhite };
        }
      }
      expect(found, "expected some grey where raw contrast favours dark but the 3x bias favours light").toBeDefined();
      const chosen = getTextColorForBackground(found!.hex, DARK_TEXT, LIGHT_TEXT);
      expect(chosen).toBe(LIGHT_TEXT);
    });

    it("falls back to the dark candidate for a background it cannot parse, rather than throwing", () => {
      expect(() => getTextColorForBackground("not-a-color", DARK_TEXT, LIGHT_TEXT)).not.toThrow();
      expect(getTextColorForBackground("not-a-color", DARK_TEXT, LIGHT_TEXT)).toBe(DARK_TEXT);
    });
  });
});
