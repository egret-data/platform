import { readFileSync } from "node:fs";
import path from "node:path";

/**
 * OKLCH → sRGB, WCAG relative luminance, contrast, source-over compositing, and `index.css` token
 * lookup, for tests that have to assert a colour RELATIONSHIP rather than a colour string.
 *
 * Why this is arithmetic and not a rendered measurement: jsdom runs no layout and no paint, and
 * `canvas` is not a dependency of this project, so `getContext("2d")` returns null and a pixel
 * readback in vitest measures nothing. The conversion is closed-form, `index.css` is the single
 * source of every token involved, and the same method is already used by
 * `src/__tests__/focusIndicatorContrast.test.ts` — including the trap it documents: nothing here
 * regexes a colour into three numbers, because `oklch(L C H)` read as `rgb(R G B)` produces
 * confident garbage.
 *
 * That file keeps its own inline copies of these functions and is deliberately NOT refactored to
 * import them: it is ~900 lines of load-bearing commentary about a published accessibility number,
 * and a mechanical extraction across it is a larger risk than the duplication. New contrast tests
 * import from here.
 *
 * This file is `support/`, not a spec file, so vitest's default `include` does not collect it.
 */

export type Rgb = [number, number, number];
/** A colour plus the alpha it will be painted at. */
export interface Paint { rgb: Rgb; alpha: number }
export interface Oklch { L: number; C: number; h: number; alpha: number }

const CSS = readFileSync(path.resolve(__dirname, "../../index.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** The published oklab→LMS→linear-sRGB matrices (Ottosson). Gamut-clipped the way a browser clips
 *  when it paints, so the result is the colour that reaches the screen, not an out-of-gamut ideal. */
export function oklchToSrgb(L: number, C: number, hDeg: number): Rgb {
  const h = (hDeg * Math.PI) / 180;
  const a = C * Math.cos(h);
  const b = C * Math.sin(h);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const linear = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  const clamp = (x: number) => Math.min(1, Math.max(0, x));
  const encode = (u: number) => (u <= 0.0031308 ? 12.92 * u : 1.055 * Math.pow(u, 1 / 2.4) - 0.055);
  return linear.map((u) => clamp(encode(clamp(u)))) as Rgb;
}

export function relativeLuminance([r, g, b]: Rgb): number {
  const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

export function contrastRatio(a: Rgb, b: Rgb): number {
  const [x, y] = [relativeLuminance(a), relativeLuminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** Source-over composite of `fg` onto an opaque `bg`, in gamma space — which is where a browser
 *  composites an SVG stroke painted over a background colour. */
export function over(fg: Paint, bg: Rgb): Rgb {
  return fg.rgb.map((c, i) => c * fg.alpha + bg[i] * (1 - fg.alpha)) as Rgb;
}

/** The `:root { … }` and `.dark { … }` declaration blocks of index.css, by theme. */
export function themeBlock(theme: "light" | "dark"): string {
  const selector = theme === "light" ? ":root" : "\\.dark";
  const match = CSS.match(new RegExp(`(?:^|\\n)${selector}\\s*\\{([\\s\\S]*?)\\n\\}`));
  if (!match) throw new Error(`no ${theme} theme block found in index.css`);
  return match[1];
}

/** Resolve one custom property to OKLCH components plus alpha. Throws rather than guessing —
 *  a token that silently resolved to a default would make every assertion below meaningless.
 *  Handles the translucent form: `--border` is `oklch(1 0 0 / 10%)` in the dark theme. */
export function tokenOklch(theme: "light" | "dark", name: string): Oklch {
  const decl = themeBlock(theme).match(new RegExp(`--${name}:\\s*([^;]+);`));
  if (!decl) throw new Error(`--${name} is not defined in the ${theme} theme`);
  const raw = decl[1].trim();
  const m = raw.match(/^oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*(?:\/\s*([\d.]+)(%?)\s*)?\)$/);
  if (!m) throw new Error(`--${name} in ${theme} is not a plain oklch(): ${raw}`);
  const alpha = m[4] === undefined ? 1 : m[5] === "%" ? Number(m[4]) / 100 : Number(m[4]);
  return { L: Number(m[1]), C: Number(m[2]), h: Number(m[3]), alpha };
}

export function token(theme: "light" | "dark", name: string): Paint {
  const { L, C, h, alpha } = tokenOklch(theme, name);
  return { rgb: oklchToSrgb(L, C, h), alpha };
}
