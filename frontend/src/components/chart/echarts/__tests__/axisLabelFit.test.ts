import { describe, it, expect } from "vitest";
import { fitXLabels, type XLabelFit } from "../axisLabelFit";
import type { TextMeasurer } from "@/components/chart/measure-text";

/** Every label is `perChar` wide per character and `fontSize` tall. Injected so the ladder can be
 *  driven to each of its branches exactly; jsdom's real measurer returns zeroes. */
const stub = (perChar: number): TextMeasurer =>
  (text, style) => ({ width: text.length * perChar, height: Number(style.size) });

const fit = (labels: string[], plotWidth: number, outerHeight = 400, perChar = 8): XLabelFit =>
  fitXLabels({ labels, plotWidth, outerHeight, fontSize: 13, measure: stub(perChar) });

/** The four the live comparison was run on, and every width below is arithmetic about these:
 *  padded they measure 64, 64, 56 and 88px, and "Doohickey" is 72px bare. */
const CATEGORIES = ["Widget", "Gadget", "Gizmo", "Doohickey"];

describe("fitXLabels", () => {
  it("draws every label upright when they all fit", () => {
    // dimensionWidth = 800/4 = 200px. The worst adjacent pair is Gizmo/Doohickey at
    // 56/2 + 88/2 + 6 = 78, and the widest label alone is 88 + 3 = 91 — both far under 200.
    expect(fit(CATEGORIES, 800)).toEqual({ kind: "show", rotate: 0 });
  });

  /**
   * The defect this module exists for. Same four labels on a one-third-width card: upright they
   * collide, so today's `interval: "auto"` drew ONE of the four. Metabase rotates instead.
   */
  it("rotates 45° when labels collide but there is still room per label", () => {
    // dimensionWidth = 240/4 = 60px. Widget/Gadget need 64/2 + 64/2 + 6 = 70 > 60, so they collide;
    // 60 >= 13*2.1 (27.3) keeps it on the top rung, and 72/√2 = 51px is well under 70% of 400.
    // `toEqual`, not `toMatchObject`: the fit is a DECISION and carries nothing else. It used to
    // carry a `bottomPad` that the builders added to `grid.bottom` on top of what `containLabel`
    // had already reserved, which crushed the plot — so an exact shape here is what stops the pad
    // coming back (`cartesianXLabels.test.ts` guards the builders' end of the same contract).
    expect(fit(CATEGORIES, 240)).toEqual({ kind: "rotate", rotate: 45 });
  });

  it("rotates 90° when there is not even room for 45°", () => {
    // dimensionWidth = 60/4 = 15px. 15 >= 13*2.1 (27.3) is false; 15 >= 13*1.2 (15.6) is false too,
    // so widen slightly to land on the 90 branch rather than the hide branch.
    expect(fit(CATEGORIES, 4 * 13 * 1.3)).toEqual({ kind: "rotate", rotate: 90 });
  });

  /**
   * The branch most easily left untested, and the one that changes what a user sees most: when even
   * a rotated label would eat 70% of the chart's height, Metabase hides the axis rather than
   * drawing a wall of text. Asserted with a label long enough that its rotated height exceeds the
   * ratio, so this is the height guard firing and not the width ladder.
   */
  it("hides the axis when a rotated label would be taller than 70% of the chart", () => {
    const long = "Enterprise Widget Assembly, North-Western Distribution Region";
    expect(fit([long, long, long, long], 4 * 13 * 1.3, 120)).toEqual({ kind: "hide" });
  });

  /**
   * Only 50 of an axis's labels are ever measured, so every one of these asks the same question:
   * does the label that actually decides the answer get measured, wherever it happens to sit?
   *
   * All three use 60 labels over 1200px — `dimensionWidth` 20px, above `fontSize`, so the ladder
   * reaches the 90° rung and only the widest label's height decides rotate-vs-hide. That framing
   * is the point: the plan's original fixture (200 narrow labels + one wide at 400px) could not
   * detect anything, because at ~2px per dimension even a one-character label overlaps and the
   * answer never depended on the wide label being measured at all. Verified — it passes against a
   * prefix-sampling implementation.
   */
  describe("measures the label that decides the answer", () => {
    const narrow = Array<string>(60).fill("a");
    const wide = "Enterprise Widget Assembly, North-Western Region"; // 48 chars = 384px > 0.7 x 400
    const withWideAt = (i: number) => narrow.map((l, j) => (j === i ? wide : l));

    it("rotates when every label is narrow", () => {
      expect(fit(narrow, 1200)).toMatchObject({ kind: "rotate", rotate: 90 });
    });

    it("finds the widest label when it is last", () => {
      expect(fit(withWideAt(59), 1200)).toEqual({ kind: "hide" });
    });

    /**
     * Index 3 is NOT in the even sample of 60 (which runs 0, 1, 2, 4, 5, …), so this is caught only
     * by also measuring the longest label by character count — the same proxy Metabase's own
     * category branch uses. Without it egret rotates a 48-character label to 90° where Metabase
     * hides the axis.
     */
    it("finds the widest label when it falls in a sampling gap", () => {
      expect(fit(withWideAt(3), 1200)).toEqual({ kind: "hide" });
    });

    /**
     * And character count alone is not enough either, which is why the even sample stays: "WWWW" is
     * wider than "iiiiiiiiiiii" while being a third of its length. The wide-in-pixels label sits at
     * index 59 — inside the even sample, outside a 50-long prefix — and the longest-by-characters
     * label at index 10 is inside both, so only even spacing can reach the one that matters.
     */
    it("finds the widest label when it is not the longest one", () => {
      const perChar = (c: string) => (c === "W" ? 30 : c === "i" ? 3 : 8);
      const proportional: TextMeasurer = (text, style) => ({
        width: [...text].reduce((w, c) => w + perChar(c), 0),
        height: Number(style.size),
      });
      const labels = narrow.map((l, i) => (i === 10 ? "iiiiiiiiiiii" : i === 59 ? "WWWW" : l));
      // "WWWW" is 120px; 120 / 150 exceeds the 70% guard, while the 36px "iiiiiiiiiiii" does not.
      expect(fitXLabels({
        labels, plotWidth: 1200, outerHeight: 150, fontSize: 13, measure: proportional,
      })).toEqual({ kind: "hide" });
    });
  });

  /**
   * The two spaces Metabase pads a label with are there to make the OVERLAP question pessimistic
   * (`getPaddedAxisLabel`); `getMaxXTickWidth` measures the bare formatted value. Sized so the
   * distinction is the whole answer: "Doohickeys" is 80px bare and 96px padded, and 0.7 x 120px
   * falls between them — so padding the height guard would hide an axis that fits, over two spaces
   * that are never drawn. Exact widths are safe to reason about because the measurer is the stub;
   * the real one returns zeroes under jsdom.
   */
  it("measures the height guard on the bare label, not the padded one", () => {
    expect(fit(["Doohickeys", "Doohickeys", "Doohickeys", "Doohickeys"], 80, 120)).toEqual({
      kind: "rotate", rotate: 90,
    });
  });

  /**
   * The √2, which is the module's only non-trivial arithmetic and is the easiest thing here to get
   * wrong: a mutant projecting a 45° label at its FULL width passed every other test in this file.
   *
   * It used to be pinned through `bottomPad`, which was the only place the factor showed through.
   * `bottomPad` is gone — `containLabel` reserves that room and adding it twice crushed the plot,
   * see `XLabelFit` — so the projection is now internal, and its one remaining observable is the
   * 70% height guard: a rotation is taken iff `projected(rotate) / outerHeight < 0.7`.
   *
   * So bracket it there. Same four labels (72px bare for "Doohickey"), each rung reached by plot
   * width alone, and `outerHeight` walked across the guard's boundary. The boundary sits at
   * `projected / 0.7`, so pinning the exact height at which each rung flips pins the projection to
   * within a pixel:
   *
   *   - 45°: flips between 72 and 73  ->  projected(45) is in (50.4, 51.1]  -> 72/√2 = 50.91  ✓
   *   - 90°: flips between 102 and 103 -> projected(90) is in (71.4, 72.1]  -> 72        ✓
   *
   * An unprojected 45° (72px, the mutant) would need an outerHeight of 103 to flip, so it fails on
   * the very first assertion below. A projection applied to the 90° rung too (72/√2) would flip at
   * 73 and fail the third.
   */
  it("projects a 45° label at width/√2 and a 90° one at its full width", () => {
    const at45 = (outerHeight: number) => fit(CATEGORIES, 240, outerHeight).kind;
    // dimensionWidth 16.9: below the 45 rung's 27.3, above the 90 rung's 15.6.
    const at90 = (outerHeight: number) => fit(CATEGORIES, 4 * 13 * 1.3, outerHeight).kind;

    expect(at45(73)).toBe("rotate");
    expect(at45(72)).toBe("hide");
    expect(at90(103)).toBe("rotate");
    expect(at90(102)).toBe("hide");
  });

  it("draws upright when there are no labels rather than dividing by zero", () => {
    expect(fit([], 800)).toEqual({ kind: "show", rotate: 0 });
  });
});
