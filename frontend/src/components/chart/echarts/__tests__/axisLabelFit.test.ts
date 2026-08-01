import { describe, it, expect } from "vitest";
import { fitXLabels, type XLabelFit } from "../axisLabelFit";
import type { TextMeasurer } from "@/components/chart/measure-text";

/** Every label is `perChar` wide per character and `fontSize` tall. Injected so the ladder can be
 *  driven to each of its branches exactly; jsdom's real measurer returns zeroes. */
const stub = (perChar: number): TextMeasurer =>
  (text, style) => ({ width: text.length * perChar, height: Number(style.size) });

const fit = (labels: string[], plotWidth: number, plotHeight = 400, perChar = 8): XLabelFit =>
  fitXLabels({ labels, plotWidth, plotHeight, fontSize: 13, measure: stub(perChar) });

describe("fitXLabels", () => {
  it("draws every label upright when they all fit", () => {
    // 4 labels x 80px each = 320 of 800: no adjacent pair collides.
    expect(fit(["Widget", "Gadget", "Gizmo", "Doohickey"], 800)).toEqual({
      kind: "show", rotate: 0, bottomPad: 0,
    });
  });

  /**
   * The defect this module exists for. Same four labels on a one-third-width card: upright they
   * collide, so today's `interval: "auto"` drew ONE of the four. Metabase rotates instead.
   */
  it("rotates 45° when labels collide but there is still room per label", () => {
    const r = fit(["Widget", "Gadget", "Gizmo", "Doohickey"], 240);
    expect(r.kind).toBe("rotate");
    expect(r).toMatchObject({ rotate: 45 });
  });

  it("rotates 90° when there is not even room for 45°", () => {
    // dimensionWidth = 60/4 = 15px. 15 >= 13*2.1 (27.3) is false; 15 >= 13*1.2 (15.6) is false too,
    // so widen slightly to land on the 90 branch rather than the hide branch.
    const r = fit(["Widget", "Gadget", "Gizmo", "Doohickey"], 4 * 13 * 1.3);
    expect(r).toMatchObject({ kind: "rotate", rotate: 90 });
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
   * An even sample, not a prefix: on a sorted axis the widest label is as likely to be last.
   *
   * Written so a prefix sample would give a DIFFERENT answer, which the plan's original fixture
   * (200 narrow labels + one wide one at 400px) did not — at that width `dimensionWidth` is ~2px
   * and even a one-character label overlaps, so it left the upright branch whether or not the wide
   * label was ever measured. Here 60 labels over 1200px puts `dimensionWidth` at 20px: wide enough
   * that the 90° branch is reached, narrow enough that it is reached, and then only the last
   * label's width decides between rotating and hiding.
   */
  it("measures the widest label even when it is last", () => {
    const labels = [...Array(59).fill("a"), "Enterprise Widget Assembly, North-West"];
    // 38 chars padded x 8px = 320px > 0.7 x 400px, so the height guard fires — but only if the last
    // label was in the sample at all. A prefix of 50 sees nothing but "a" and rotates 90°.
    expect(fit(labels, 1200)).toEqual({ kind: "hide" });
    expect(fit([...Array(60).fill("a")], 1200)).toMatchObject({ kind: "rotate", rotate: 90 });
  });

  it("reserves bottom padding proportional to the rotation", () => {
    const upright = fit(["a", "b"], 800);
    const rotated = fit(["Widget", "Gadget", "Gizmo", "Doohickey"], 240);
    expect(upright.kind === "show" && upright.bottomPad).toBe(0);
    expect(rotated.kind === "rotate" && rotated.bottomPad).toBeGreaterThan(0);
  });

  it("draws upright when there are no labels rather than dividing by zero", () => {
    expect(fit([], 800)).toEqual({ kind: "show", rotate: 0, bottomPad: 0 });
  });
});
