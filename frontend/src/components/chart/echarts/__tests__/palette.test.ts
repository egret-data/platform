import { describe, it, expect } from "vitest";
import { SERIES_PALETTE, CHART_THEME, seriesColor, resolveColor, lightenOklch } from "../palette";

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
});
