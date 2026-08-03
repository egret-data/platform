import { describe, it, expect } from "vitest";
import * as echarts from "echarts/core";
import { registerEChartsModules } from "../register";
import { buildCartesianOption } from "../options/cartesian";
import { buildWaterfallOption } from "../options/waterfall";
import { buildBoxplotOption } from "../options/boxplot";

/**
 * THE VALUE-LABEL DENSITY SETTINGS, RENDERED — what "Fit" and "All" actually put on a card.
 *
 * Three panels offer a density control the moment a user turns values on: `fit` / `all` on the
 * cartesian and waterfall builders, and "Values to display: Median only / All" on the box plot.
 * All three are implemented by one line of ECharts option, `labelLayout: { hideOverlap: true }`,
 * and until `register.ts` listed `LabelLayout` that option WAS DEAD IN THE SHIPPED APP — echarts
 * installs the component from `lib/export/core.js`, which is not in the package's `sideEffects`
 * allowlist, so Rollup deleted the call and every setting above rendered identically in the build.
 *
 * WHAT THIS FILE DOES NOT PROVE, said plainly. It cannot fail for that defect. VITEST DOES NOT
 * TREE-SHAKE: the `use()` echarts performs on its own survives here whether or not `register.ts`
 * lists the component, so every case below passed before the fix as well as after. `import
 * { LabelLayout }` could be deleted from `register.ts` tomorrow and this file would stay green
 * while the app went back to drawing overlapping numbers. The only check that can fail for that
 * reason reads the production bundle and therefore cannot live in a unit suite — it is
 * `frontend/scripts/assert-label-layout-bundled.mjs`, run in CI straight after `npm run build`,
 * and its header explains which symbols survive minification.
 *
 * `pieLabelRoom.echarts.test.ts` is the precedent for a header that states its own limits, and it
 * takes the opposite route for the opposite reason: it STRIPS `labelLayout` before rendering,
 * because what it is measuring is the pie's own geometric fit in isolation.
 *
 * WHAT IT DOES PROVE, which is the other half of the change and is not covered anywhere else:
 * given a working LabelManager, the three settings select genuinely different pictures, and none
 * of them is a synonym for "draw fewer labels always". Each case pins BOTH directions —
 * the crowded size where the two settings must differ, and the roomy size where the thinning one
 * must still write every number — so neither "hide always" nor "hide never" passes.
 *
 * NO PIXEL IS ASSERTED. Every case reads the text nodes ECharts emitted and asserts WHICH LABELS
 * SURVIVED; placement is ECharts' own layout. Value labels are told apart from axis ticks and
 * category names by `font-weight:700` (`CHART_THEME.labelWeight`), which is the only thing the
 * three builders put on a series label and nothing puts on an axis.
 */

/** The value labels the chart DREW, in the order ECharts emitted them. Axis ticks and category
 *  names share the label font SIZE, so weight is what separates them — see the header. */
function valueLabels(option: unknown, size: { width: number; height: number }): string[] {
  registerEChartsModules();
  const chart = echarts.init(null, null, { renderer: "svg", ssr: true, ...size });
  chart.setOption({ ...(option as object), animation: false });
  const svg = chart.renderToSVGString();
  chart.dispose();
  return [...svg.matchAll(/<text\b([^>]*)>([^<]*)<\/text>/g)]
    .filter(([, attrs]) => /font-weight:700/.test(attrs))
    .map(([, , text]) => text)
    .filter((t) => t.trim() !== "");
}

/** The harness's own dashboard card. Eight labels have nowhere near the room here. */
const CARD = { width: 166, height: 129 };
/** Roomy enough that nothing collides, which is what stops "hide everything" passing. */
const ROOMY = { width: 640, height: 400 };

const CATS = ["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel"];
/** Deliberately alternating high/low so consecutive labels sit at different heights: a monotone
 *  series would let a naive "keep every other one" rule look like collision handling. */
const VALUES = [11234, 22345, 13456, 24567, 15678, 26789, 17890, 28901];
const ALL_EIGHT = ["11,234", "22,345", "13,456", "24,567", "15,678", "26,789", "17,890", "28,901"];

const cartesian = (labelDensity: "fit" | "all", size: { width: number; height: number }) =>
  buildCartesianOption(
    [{ name: "c", type: "varchar" }, { name: "rev", type: "bigint" }],
    CATS.map((c, i) => [c, VALUES[i]]),
    {
      kind: "bar", xAxis: "c", yMetrics: ["rev"], metricTypes: { rev: "bigint" },
      showLabels: true, labelDensity, showXLabel: false, showYLabel: false,
    },
    size,
  );

/** The waterfall's labels are SIGNED STEPS, not the bar values, so its expected set is its own —
 *  see `buildWaterfallOption`'s formatter. Nine of them: eight steps plus the total. */
const waterfall = (labelDensity: "fit" | "all", size: { width: number; height: number }) =>
  buildWaterfallOption(
    [{ name: "c", type: "varchar" }, { name: "rev", type: "bigint" }],
    CATS.map((c, i) => [c, VALUES[i] - 10000]),
    { xAxis: "c", metric: "rev", showLabels: true, labelDensity, showXLabel: false, showYLabel: false },
    size,
  );
const ALL_NINE = ["1.2K", "12.3K", "3.5K", "14.6K", "5.7K", "16.8K", "7.9K", "18.9K", "80.9K"];

describe("a bar chart's value-label density", () => {
  /**
   * THE SETTING THAT USED TO LIE. Before the component reached the bundle these two lists were
   * identical at every size, on a control the panel names "Fit" and "All".
   *
   * `fit` is asserted as a STRICT, NON-EMPTY SUBSET rather than as the four labels that happen to
   * survive today (`13,456 / 24,567 / 17,890 / 28,901`, measured): which of a colliding pair
   * ECharts keeps is its own resolution order, and pinning it here would make an echarts patch
   * release look like a regression in this repo. Strictness is what carries the claim — a build
   * where the backstop is dead draws all eight and fails on the length.
   */
  it("thins the labels on 'Fit' at a card's width and keeps every one on 'All'", () => {
    const fit = valueLabels(cartesian("fit", CARD), CARD);
    const all = valueLabels(cartesian("all", CARD), CARD);

    expect(all).toEqual(ALL_EIGHT);
    expect(fit.length).toBeGreaterThan(0);
    expect(fit.length).toBeLessThan(all.length);
    expect(ALL_EIGHT).toEqual(expect.arrayContaining(fit));
  });

  /** The other direction: `fit` is not "draw fewer", it is "drop what collides". Given the room it
   *  writes every number, which is what fails if the backstop is ever left on unconditionally. */
  it("writes every value on 'Fit' once the chart has the room", () => {
    expect(valueLabels(cartesian("fit", ROOMY), ROOMY)).toEqual(ALL_EIGHT);
    expect(valueLabels(cartesian("all", ROOMY), ROOMY)).toEqual(ALL_EIGHT);
  });
});

describe("a waterfall's value-label density", () => {
  it("thins the steps on 'Fit' at a card's width and keeps every one on 'All'", () => {
    const fit = valueLabels(waterfall("fit", CARD), CARD);
    const all = valueLabels(waterfall("all", CARD), CARD);

    expect(all).toEqual(ALL_NINE);
    expect(fit.length).toBeGreaterThan(0);
    expect(fit.length).toBeLessThan(all.length);
    expect(ALL_NINE).toEqual(expect.arrayContaining(fit));
  });

  it("writes every step on 'Fit' once the chart has the room", () => {
    expect(valueLabels(waterfall("fit", ROOMY), ROOMY)).toEqual(ALL_NINE);
    expect(valueLabels(waterfall("all", ROOMY), ROOMY)).toEqual(ALL_NINE);
  });
});

/**
 * TWELVE BOXES OF NINE READINGS, drifting three units apart — chosen so the five statistics of one
 * box are separated by a handful of pixels and its median sits within a few of its neighbour's.
 * That makes BOTH claims below falsifiable on one fixture: "All" has sixty labels stacked five to a
 * box, and "Median only" has twelve close enough together to collide at a narrow width and far
 * enough apart to fit at a wide one.
 *
 * `compactValues: false` so each label is a distinct four-digit string; compacted, several boxes
 * would round to the same text and a dropped label would be indistinguishable from a duplicate.
 */
const BOXES = ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L"];
const BOX_ROWS: unknown[][] = BOXES.flatMap((g, gi) =>
  Array.from({ length: 9 }, (_, i) => [g, 1000 + gi * 3 + i * 7]));

const boxplot = (labelMode: "median" | "all", size: { width: number; height: number }) =>
  buildBoxplotOption(
    [{ name: "g", type: "varchar" }, { name: "v", type: "double" }],
    BOX_ROWS,
    {
      category: "g", value: "v", showLabels: true, labelMode,
      compactValues: false, showXLabel: false, showYLabel: false,
    },
    size,
  );

/** A middling card and a full-width one. `MEDIUM` is where "Median only" thins and "All" must not;
 *  `ROOMY` is where neither does. */
const MEDIUM = { width: 400, height: 300 };

describe("a box plot's 'Values to display'", () => {
  /**
   * "ALL" MEANS ALL — the correction this change had to make rather than inherit.
   *
   * The box plot has no `labelDensity`; "Values to display" IS its density control, because
   * labelling all five statistics is the only setting that can crowd. `options/boxplot.ts` set
   * `hideOverlap` unconditionally, which was harmless while the component was tree-shaken out and
   * became a lie the moment it was not: 12 of 45 texts gone at 640x400 and 26 at a card, on a
   * setting whose entire meaning is that the user asked for every number. Now gated on
   * `labelMode !== "all"`, mirroring the cartesian rule — "All" is for someone who would rather
   * have every statistic and accept the crowding.
   */
  it("draws all five statistics of every box on 'All', at a card and at full width", () => {
    for (const size of [MEDIUM, ROOMY]) {
      expect(valueLabels(boxplot("all", size), size)).toHaveLength(BOXES.length * 5);
    }
  });

  /** And "Median only" keeps the backstop, which is the whole reason the gate is on the mode
   *  rather than removed: one label per box still collides on a crowded category axis. */
  it("still thins the medians on 'Median only' when the axis is crowded", () => {
    const medians = valueLabels(boxplot("median", MEDIUM), MEDIUM);
    expect(medians.length).toBeGreaterThan(0);
    expect(medians.length).toBeLessThan(BOXES.length);
  });

  /** The other direction for the median path, so "thins" cannot be satisfied by dropping labels
   *  unconditionally: given the width, all twelve are drawn. */
  it("writes every median once the axis has the room", () => {
    expect(valueLabels(boxplot("median", ROOMY), ROOMY)).toHaveLength(BOXES.length);
  });
});
