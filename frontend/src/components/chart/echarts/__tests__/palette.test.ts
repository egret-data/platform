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
      const nearWhite = "#fefefe";
      const lightened = lightenOklch(nearWhite, 5);
      expect(lightened).toMatch(/^#[0-9a-f]{6}$/i);
      // still a valid, opaque-white-or-lighter-than-input colour, not NaN/garbage
      const { L } = oklch(lightened);
      expect(L).toBeLessThanOrEqual(1.001);
      expect(L).toBeGreaterThanOrEqual(oklch(nearWhite).L);
    });
  });
});
