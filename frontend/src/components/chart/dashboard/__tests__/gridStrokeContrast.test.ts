import { describe, it, expect } from "vitest";
import {
  GRID_STROKE_CSS, GRID_STROKE_MAX_CONTRAST, GRID_STROKE_MIN_CONTRAST,
} from "../grid-stroke";
import {
  contrastRatio, oklchToSrgb, over, token, tokenOklch, type Paint,
} from "@/__tests__/support/wcag";

/** The surface the guide grid is painted on: `DashboardView.tsx` wraps the whole route in
 *  `bg-dashboard`, and the grid background sits directly on it. NOT `--background`: the first
 *  measurement taken for the design used that and read 2.36:1 where the truth is 2.42:1. */
const CANVAS = "dashboard";

/**
 * Resolve the stroke expression the component ACTUALLY uses, in the given theme.
 *
 * Two forms are supported — a bare `var(--token)` and CSS Color 5's `color-mix(in oklch, …)` —
 * so that restoring the old blend makes this test recompute ITS contrast and fail on the number,
 * rather than failing on a changed string. A test that asserted `GRID_STROKE_CSS === "var(--border)"`
 * would pin the string and measure nothing.
 */
function resolveStroke(expr: string, theme: "light" | "dark"): Paint {
  const plain = expr.match(/^var\(--([\w-]+)\)$/);
  if (plain) return token(theme, plain[1]);

  const mix = expr.match(/^color-mix\(in oklch,\s*var\(--([\w-]+)\)\s+(\d+)%,\s*var\(--([\w-]+)\)\)$/);
  if (!mix) throw new Error(`unsupported stroke expression: ${expr}`);
  const p1 = Number(mix[2]) / 100;
  const a = tokenOklch(theme, mix[1]);
  const b = tokenOklch(theme, mix[3]);
  // CSS Color 5 §3.2: each percentage is scaled by that colour's alpha, the components are mixed
  // alpha-premultiplied, and the result is un-premultiplied by the summed alpha. This is why an
  // 80/20 blend of a 10%-alpha --border with an opaque --foreground lands at alpha 0.28, not 0.10 —
  // and it is exactly the arithmetic that made the dark grid twice as loud as Metabase's.
  const alpha = p1 * a.alpha + (1 - p1) * b.alpha;
  const w1 = (p1 * a.alpha) / alpha;
  const w2 = ((1 - p1) * b.alpha) / alpha;
  return {
    rgb: oklchToSrgb(a.L * w1 + b.L * w2, a.C * w1 + b.C * w2, a.h * w1 + b.h * w2),
    alpha,
  };
}

function paintedContrast(expr: string, theme: "light" | "dark"): number {
  const canvas = token(theme, CANVAS).rgb;
  return contrastRatio(over(resolveStroke(expr, theme), canvas), canvas);
}

describe("the edit-mode grid guide stays inside Metabase's contrast band", () => {
  for (const theme of ["light", "dark"] as const) {
    it(`${theme}: the stroke composites into [${GRID_STROKE_MIN_CONTRAST}, ${GRID_STROKE_MAX_CONTRAST}] against --dashboard`, () => {
      const ratio = paintedContrast(GRID_STROKE_CSS, theme);
      const label = `grid stroke on --dashboard in ${theme}: ${ratio.toFixed(2)}:1`;
      // The ceiling is the defect: a guide grid is a hint under the cards, not a table of boxes,
      // and Metabase's measures 1.27:1. The floor exists only so nobody "fixes" it to nothing.
      expect(ratio, label).toBeLessThanOrEqual(GRID_STROKE_MAX_CONTRAST);
      expect(ratio, label).toBeGreaterThanOrEqual(GRID_STROKE_MIN_CONTRAST);
    });
  }

  it("the pre-fix blend is what this band rules out, in both themes", () => {
    // Guards the evaluator itself. If `resolveStroke` returned the same Paint for both expressions
    // the assertions above would pass no matter what the component painted — the precise failure
    // mode the chart programme hit six times.
    const blend = "color-mix(in oklch, var(--border) 80%, var(--foreground))";
    expect(paintedContrast(blend, "dark")).toBeGreaterThan(GRID_STROKE_MAX_CONTRAST);
    expect(paintedContrast(blend, "light")).toBeGreaterThan(GRID_STROKE_MAX_CONTRAST);
  });
});
