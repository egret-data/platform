/**
 * Canvas text measurement, ported from Metabase's `frontend/src/metabase/utils/measure-text.ts`.
 *
 * Used by the scalar's auto-sizer to pick the largest font that still fits its card. The option
 * builders must never GUESS a size — chart 05 invented a 300px reference container against a real
 * ~530px card and truncated all 51 of its labels — so everything here measures what is actually
 * on screen.
 */

export interface FontStyle {
  /** A CSS length. `rem` is supported; see the note on the shared canvas below. */
  size: string | number;
  family: string;
  weight: string | number;
}

export interface TextSize {
  width: number;
  height: number;
}

export type TextMeasurer = (text: string, style: FontStyle) => TextSize;

let canvas: HTMLCanvasElement | null = null;

/** Reset the shared canvas. Tests only — a suite that changes the root font size needs the next
 * measurement to pick it up rather than reuse a canvas stamped with the old one. */
export function __resetMeasureCanvas(): void {
  canvas?.remove();
  canvas = null;
}

/**
 * The text's width and glyph height at a given font.
 *
 * Two details are load-bearing and both are copied rather than invented:
 *
 * 1. The canvas is APPENDED TO THE DOCUMENT and its own `font-size` is set from the document root.
 *    `context.font` resolves `rem` against the canvas element's computed font size, so a detached
 *    canvas silently measures every `rem` against 16px — correct only until a user changes their
 *    base font size, and then wrong everywhere with no visible cause.
 * 2. Height is `actualBoundingBoxAscent + actualBoundingBoxDescent` — the GLYPH height, not the
 *    line height. Fitting a number against line height leaves a visible gap under every card.
 */
export const measureText: TextMeasurer = (text, style) => {
  if (!canvas) {
    canvas = document.createElement("canvas");
    document.body.appendChild(canvas);
    canvas.style.display = "none";
    canvas.style.fontSize = window.getComputedStyle(document.documentElement).fontSize;
  }

  // No 2D context means no measurement is possible — jsdom without `node-canvas` is the case that
  // matters, and it is exactly the blind spot the parity rules call out: **jsdom runs no layout**.
  // Reporting a zero size makes every candidate font "fit", so `findSize` returns its maximum.
  // That is deliberate: it keeps the component rendering (its text, roles and handlers stay
  // testable) while making it impossible for a jsdom test to accidentally assert a CHOSEN size and
  // look like it verified the fit. Size claims belong in a browser.
  const context = canvas.getContext("2d");
  if (!context) return { width: 0, height: 0 };

  const fontSize = typeof style.size === "number" ? `${style.size}px` : style.size;
  context.font = `${style.weight} ${fontSize} ${style.family}`;
  const m = context.measureText(text);

  return {
    width: m.width,
    height: m.actualBoundingBoxAscent + m.actualBoundingBoxDescent,
  };
};

/** Three decimals — far finer than any step this is used with, so it only removes accumulated
 * binary-float error and never quantises a size the caller actually asked for. */
function roundToStepPrecision(n: number): number {
  return Math.round(n * 1000) / 1000;
}

export interface FindSizeInput {
  text: string;
  targetHeight: number;
  targetWidth: number;
  unit: "rem";
  fontFamily: string;
  fontWeight: string | number;
  step: number;
  min: number;
  max: number;
  /** Injectable for tests: jsdom's `measureText` returns zeroes, so a test that asserts a chosen
   * size against the real measurer is only asserting where the loop bottoms out. */
  measurer?: TextMeasurer;
}

/**
 * The largest size in `[min, max]`, stepping down by `step`, at which `text` fits both dimensions.
 * Copied verbatim from Metabase's `findSize`, including that it returns `min` when nothing fits —
 * a number too big for its card shrinks to the floor and is then handled by compaction, rather
 * than disappearing.
 */
export function findSize({
  text, targetHeight, targetWidth, unit, fontFamily, fontWeight, step, min, max,
  measurer = measureText,
}: FindSizeInput): string {
  let size = max;
  let metrics = measurer(text, { size: `${size}${unit}`, family: fontFamily, weight: fontWeight });

  while ((metrics.width > targetWidth || metrics.height > targetHeight) && size > min) {
    // Rounded at each step. Metabase writes the bare `size - step`, and with its 0.2 step the
    // float error accumulates: stepping 7 down to what should be 2.4 yields 2.399999999999996,
    // which then goes into the DOM as a font-size with sixteen decimal places and never lands on
    // the step grid the constant describes. Deliberate divergence, caught by this module's tests.
    size = Math.max(roundToStepPrecision(size - step), min);
    metrics = measurer(text, { size: `${size}${unit}`, family: fontFamily, weight: fontWeight });
  }

  return `${size}${unit}`;
}
