import { describe, it, expect } from "vitest";
import { fitXLabels, type XLabelFit } from "../axisLabelFit";
import { boundarySteps } from "@/components/chart/temporal-period";
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

  /**
   * THE FOURTH OUTCOME: a bucketed date axis draws every Nth label upright rather than none.
   *
   * Twelve months at "September 2026" on a one-third-width card is ~22px per category against a
   * ~95px label, so every adjacent pair collides and PR 02's ladder rotates or hides — on the ZZ
   * Chart Parity harness, line, area and combo drew NO x labels at all. Metabase draws two:
   * "January 2026" and "July 2026". See the `thin` arm of `XLabelFit` for why a regular stride over
   * period boundaries is not the arbitrary subset PR 02 banned.
   *
   * The arithmetic every case below is stated in, at the stub's 8px per character:
   *   - the widest name the MONTH unit can produce is "September 2026", 14 chars, padded 16 -> 128px,
   *     and a stride fits when 128 + 6 (the ink-free gap) <= every x dimensionWidth. So 134px.
   *   - the widest name in this DATASET is the same string, so the overlap test above fires at any
   *     dimensionWidth under 128 + 3 = 131px, i.e. any plot narrower than 1572px.
   * `MONTH_STEPS` is the real ladder rather than a literal, so "every 6th" here is the same rung a
   * month-bucketed chart gets — `temporal-period.test.ts` pins its contents.
   */
  describe("thins a bucketed temporal axis to period boundaries", () => {
    const MONTH_STEPS = boundarySteps("month");
    const MONTHS = [
      "January", "February", "March", "April", "May", "June",
      "July", "August", "September", "October", "November", "December",
    ].map((m) => `${m} 2026`);
    const MONTH_BOUNDARY = { widestLabel: "September 2026", steps: MONTH_STEPS };
    /** `boundary` is REQUIRED here and passed explicitly at every call, including the negative
     *  cases. It had a default of `MONTH_BOUNDARY`, and `thin(..., undefined)` — the case that
     *  proves an unbucketed axis is never thinned — quietly took the default and asserted the
     *  opposite of its name. It failed, which is the only reason this comment exists rather than a
     *  green test that checked nothing. */
    const thin = (
      labels: string[],
      plotWidth: number,
      boundary: { widestLabel: string; steps: number[] } | undefined,
      outerHeight = 400,
    ) => fitXLabels({ labels, plotWidth, outerHeight, boundary, fontSize: 13, measure: stub(8) });

    it("draws every label when they all fit, without consulting the ladder", () => {
      // dimensionWidth 150 >= 131, so nothing overlaps and thinning is never reached.
      expect(thin(MONTHS, 1800, MONTH_BOUNDARY)).toEqual({ kind: "show", rotate: 0 });
    });

    it("takes the CHEAPEST rung that fits rather than the first one that helps", () => {
      // 900 -> dimensionWidth 75: 2 x 75 = 150 >= 134, so every 2nd, not every 6th.
      expect(thin(MONTHS, 900, MONTH_BOUNDARY)).toEqual({ kind: "thin", rotate: 0, every: 2 });
      // 600 -> 50: 2 x 50 = 100 is short, 3 x 50 = 150 clears. Every quarter, which is the rung
      // Metabase's ladder has above `month` x 2 and is why the steps are not just powers of two.
      expect(thin(MONTHS, 600, MONTH_BOUNDARY)).toEqual({ kind: "thin", rotate: 0, every: 3 });
    });

    /**
     * Brackets the two constants the budget is built from, neither of which any case above can
     * detect on its own. 780 -> dimensionWidth 65, so a rung of 2 offers 130px against a budget of
     * 128 (padded) + 6 (gap) = 134 and loses; drop EITHER the padding Metabase measures with
     * (`getPaddedAxisLabel(formatter(date))`, `timeseries.ts:290-291`) or the ink-free gap and 2 wins.
     */
    it("budgets the padded label plus a gap, not the bare one", () => {
      expect(thin(MONTHS, 780, MONTH_BOUNDARY)).toEqual({ kind: "thin", rotate: 0, every: 3 });
    });

    /**
     * THE UNIT'S WORST CASE, NOT THIS DATASET'S — Metabase's reason for measuring
     * `getRepresentativeDates` rather than its data, and the one claim every other case here is
     * blind to because their widest label happens to be "September 2026" either way.
     *
     * Three months of May–July: the widest label PRESENT is "July 2026" at 11 padded chars (88px,
     * budget 94), while the unit can still produce "September 2026" (128px, budget 134). At
     * dimensionWidth 60 a rung of 2 offers 120px — enough for the data, short for the unit. Sizing
     * on the data draws every 2nd and clips the moment the range slides to September; sizing on the
     * unit draws every 3rd and keeps drawing it.
     */
    it("sizes on what the unit can produce, not on what this dataset happens to contain", () => {
      const summer = ["May 2026", "June 2026", "July 2026"];
      expect(thin(summer, 180, MONTH_BOUNDARY)).toEqual({ kind: "thin", rotate: 0, every: 3 });
    });

    /** The parity case, and the one the harness is judged on: a one-third-width card draws
     *  "January 2026" and "July 2026" — indices 0 and 6 of twelve — where PR 02 drew nothing. */
    it("draws January and July on a card too narrow for twelve", () => {
      // 300 -> 25: 2, 3 give 50 and 75 against 134; 6 gives 150.
      expect(thin(MONTHS, 300, MONTH_BOUNDARY)).toEqual({ kind: "thin", rotate: 0, every: 6 });
    });

    /**
     * One label is a rung, not a degenerate case — Metabase's `minTickCount = 2` is deliberately
     * not ported (see `strideThatFits`), because stepping back to two labels at this width draws
     * them overlapping, and "January 2026" alone is a truthful axis where two smeared names are not.
     */
    it("draws a single period rather than two that would collide", () => {
      // 150 -> 12.5: 12 x 12.5 = 150 >= 134, and every 12th of twelve is index 0 alone.
      expect(thin(MONTHS, 150, MONTH_BOUNDARY)).toEqual({ kind: "thin", rotate: 0, every: 12 });
    });

    /**
     * THE SINGLE LABEL IS NOT CHARGED CLEARANCE FROM A NEIGHBOUR IT DOES NOT HAVE — the gap arm of
     * the last rung's budget in `strideThatFits`, and the half that decided the live defect.
     *
     * Both cases are the same axis at a plot four pixels apart, bracketing the padded label's own
     * 128px from each side. Reinstating `HORIZONTAL_TICKS_GAP` on this rung turns the first into
     * `hide`, because 130 sits inside the 128–134 window the phantom gap opens; without the
     * bracket's second half the rule could be "never budget anything at the last rung", which the
     * 126px case refuses.
     *
     * The window is not academic. Measured in Chrome at 400 13px in the app's own `--font-sans`
     * stack, on the ZZ Chart Parity card: " September 2026 " is 109.3px against a 108.4px plot, so
     * the axis was refused its last rung by 6.9px, of which 6 were clearance from a neighbour that
     * is not drawn.
     */
    it("does not charge a between-labels gap to the rung that draws one label", () => {
      expect(thin(MONTHS, 130, MONTH_BOUNDARY)).toEqual({ kind: "thin", rotate: 0, every: 12 });
      // 126 -> 10.5: the label itself does not fit the plot, so there is nothing to draw upright.
      expect(thin(MONTHS, 126, MONTH_BOUNDARY)).toEqual({ kind: "hide" });
    });

    /**
     * A LABEL CANNOT OWN MORE AXIS THAN THERE IS — the `Math.min` arm of the same budget.
     *
     * Thirteen categories, because that is what a waterfall of twelve months has: it appends a
     * "Total". Rung 12 is then no longer the last, and rung 24 was budgeted at `24 x dimensionWidth`
     * — 184px of an axis 100px wide — so a label needing 128px "fitted" a plot that could not hold
     * it. Clamped to the plot it does not, and the axis falls through to PR 02's ladder as it should.
     */
    it("budgets a stride past the bucket count against the plot, not against the stride", () => {
      const withTotal = [...MONTHS, "Total"];
      expect(thin(withTotal, 100, MONTH_BOUNDARY)).toEqual({ kind: "hide" });
    });

    /**
     * THE REGRESSION THIS PAIR WAS FOUND THROUGH, stated as the property rather than as two widths:
     * appending one non-period category must not decide whether the axis is thinned at all.
     *
     * On the live dashboard it did. Twelve monthly buckets on a 193x214 card: the line chart's last
     * rung was 12, its budget the plot exactly, and the padded name plus a phantom gap lost by a
     * third of a pixel — no stride, rotate ladder, axis HIDDEN. The waterfall of the same twelve
     * months had thirteen categories, so rung 24 was reachable, and its 208px budget against a 113px
     * plot "fitted" — the axis drew "January 2026", correctly, for a reason that was not true. Same
     * unit, same card, same period names, opposite answers.
     *
     * 130px is the width where the old code reproduced exactly that split (12 -> hide, 13 -> thin at
     * 24). The two still choose different RUNGS, which is not the defect and is not asserted: 12 of
     * twelve and 24 of thirteen both draw index 0 and nothing else. What has to agree is that the
     * axis is thinned rather than hidden.
     */
    it("thins or does not thin regardless of one extra non-period category", () => {
      const bare = thin(MONTHS, 130, MONTH_BOUNDARY);
      const withTotal = thin([...MONTHS, "Total"], 130, MONTH_BOUNDARY);
      expect(bare.kind).toBe("thin");
      expect(withTotal.kind).toBe("thin");
    });

    /**
     * PR 02'S LADDER, UNCHANGED, when no stride fits — this is the half that makes the fourth
     * outcome an addition rather than a replacement. Both rungs are exercised: a plot too narrow
     * for even one label falls to `hide`, and one merely too narrow for a stride falls to `rotate`,
     * exactly as it did before this arm existed.
     */
    it("falls through to rotation when even the widest stride is too tight", () => {
      // Two labels: the only rung at or above the count is 2, and 2 x 50 = 100 < 134. Then the
      // ladder below runs untouched — dimensionWidth 50 >= 13 x 2.1, and 112/√2 is under 70% of 400.
      expect(thin(["January 2026", "September 2026"], 100, MONTH_BOUNDARY)).toEqual({ kind: "rotate", rotate: 45 });
    });

    it("falls through to hiding when the plot is narrower than one label", () => {
      // 100 -> 8.33: the last rung gives 100px for a label needing 134, and 8.33 clears neither
      // rotation threshold.
      expect(thin(MONTHS, 100, MONTH_BOUNDARY)).toEqual({ kind: "hide" });
    });

    /**
     * THE NEGATIVE, and the property that keeps this from being `interval: "auto"` under another
     * name: the stride is reachable only through `boundary`, which only a bucketed temporal axis
     * has. Same labels, same width, same everything — an ordinal axis of twelve product names gets
     * PR 02's answer, because there is no such thing as every second product.
     *
     * All three shapes are asserted because they are three different guards. `undefined` is what
     * `options/cartesian.ts` passes when `namesAPeriod(cfg.xAxisUnit)` is false; the empty pair is
     * what `boundarySteps`/`widestPeriodName` return for a unit that names no period, which holds
     * even if that call site is ever loosened; and the empty-steps-with-a-label case is the one a
     * partial loosening would produce.
     */
    it("never thins an axis that is not bucketed, at the width that would have thinned it", () => {
      const ordinal = { kind: "rotate", rotate: 90 };
      expect(thin(MONTHS, 300, undefined)).toEqual(ordinal);
      expect(thin(MONTHS, 300, { widestLabel: "", steps: [] })).toEqual(ordinal);
      expect(thin(MONTHS, 300, { widestLabel: "September 2026", steps: [] })).toEqual(ordinal);
      // A zero-width candidate must not "fit" every stride on the strength of measuring nothing.
      expect(thin(MONTHS, 300, { widestLabel: "", steps: MONTH_STEPS })).toEqual(ordinal);
    });
  });
});
