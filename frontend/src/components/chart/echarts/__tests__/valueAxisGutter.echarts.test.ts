import { describe, it, expect, vi } from "vitest";
import * as echarts from "echarts/core";
import { registerEChartsModules } from "../register";
import { buildCartesianOption } from "../options/cartesian";
import { buildWaterfallOption } from "../options/waterfall";
import { buildBoxplotOption } from "../options/boxplot";
import { widestTickLabel, type estimatePlotWidth as EstimatePlotWidth } from "../axisLabelFit";

/**
 * WHAT THE VALUE AXIS ACTUALLY DRAWS, against what the x-label fitter is CHARGED for it.
 *
 * `estimatePlotWidth` subtracts a gutter for the value axis, and every stride and rotation decision
 * downstream is made against what is left. Until PR 03 the three builders handed it their widest
 * DATUM formatted, and a value axis does not draw data — it draws round ticks. On the ZZ Chart
 * Parity line card that cost `"24.7K"` (34.6px) where ECharts drew `"25K"` (24.2px): ten pixels of a
 * ~119px plot reserved and never used, enough to miss the last stride by 0.86px and hide the x axis
 * entirely. `widestTickLabel` now samples the fifths of a round ceiling instead.
 *
 * WHY THIS FILE BOOTS REAL ECHARTS. The claim being made is about a string ECharts generates from
 * its own `nice()` and `intervalScale`, which no assertion written from the builder's side can check
 * — a test that compared `widestTickLabel` against a hand-written "25K" would pin this file's
 * prediction of ECharts rather than ECharts. So each case builds the real option, renders it through
 * the SVG renderer in SSR mode (no canvas, no DOM needed — the same route
 * `containLabelRotation.echarts.test.ts` takes), and reads the tick labels back out of the markup.
 *
 * NO PIXELS ARE ASSERTED, for the reason that file states: SSR text advance widths are not a
 * browser's. What is asserted is CHARACTER COUNT, which is the same proxy `widestTickLabel` chooses
 * its candidate by, and the two directions are not symmetric:
 *
 *   - charged SHORTER than drawn -> the gutter under-reserves, `plotWidth` comes out too big, and x
 *     labels are left upright and overlapping. Every case here forbids it.
 *   - charged LONGER than drawn -> the defect this fixed. Allowed at zero characters, i.e. also
 *     forbidden, which is what makes these cases bite: before the change the first one charged five
 *     characters against three.
 *
 * So each case asserts EQUALITY of character count, and the pair of bounds is written out rather
 * than folded into one `toBe` so a failure says which way it went.
 */

type EstimateArgs = Parameters<typeof EstimatePlotWidth>;

const { gutterCalls } = vi.hoisted(() => ({
  gutterCalls: [] as { widest: number; format: (v: number) => string }[],
}));

/** A pass-through spy: the real estimate still runs, and all this adds is a view of the two
 *  arguments the gutter is computed from, so the charged candidate can be reconstructed with the
 *  same function production uses rather than re-derived here. */
vi.mock("../axisLabelFit", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../axisLabelFit")>();
  return {
    ...actual,
    estimatePlotWidth: (...args: EstimateArgs) => {
      gutterCalls.push({ widest: args[1], format: args[2] });
      return actual.estimatePlotWidth(...args);
    },
  };
});

/** Wide enough that no x label is thinned or hidden, so every remaining text node in the SVG is a
 *  value-axis tick and the filter below needs no geometry. Tall enough that ECharts keeps its
 *  default five splits rather than compressing them. */
const SIZE = { width: 640, height: 480 };

/** The tick labels the value axis DREW: every text in the rendered SVG that is not one of the
 *  category labels on the other axis. Both axis titles are switched off per case, so nothing else
 *  is in there — and if that ever stops being true the extra string shows up as a longer "widest"
 *  and fails loudly rather than quietly widening the bound. */
function drawnValueTicks(option: unknown, categories: string[]): string[] {
  registerEChartsModules();
  const chart = echarts.init(null, null, {
    renderer: "svg", ssr: true, width: SIZE.width, height: SIZE.height,
  });
  chart.setOption({ ...(option as object), animation: false });
  const svg = chart.renderToSVGString();
  chart.dispose();
  return [...svg.matchAll(/>([^<>]+)</g)]
    .map((m) => m[1])
    .filter((t) => t.trim() !== "" && !categories.includes(t));
}

const longest = (labels: string[]) =>
  labels.reduce((best, l) => (l.length > best.length ? l : best), "");

interface Case {
  what: string;
  /** Built with both axis titles off; see `drawnValueTicks`. */
  option: () => unknown;
  categories: string[];
}

const CATS = ["A", "B", "C", "D"];
const cartesian = (values: number[]) => () =>
  buildCartesianOption(
    [{ name: "c", type: "varchar" }, { name: "rev", type: "bigint" }],
    CATS.map((c, i) => [c, values[i]]),
    {
      kind: "bar", xAxis: "c", yMetrics: ["rev"], metricTypes: { rev: "bigint" },
      showXLabel: false, showYLabel: false,
    },
    SIZE,
  );

const CASES: Case[] = [
  {
    // THE LIVE CASE, at the values the ZZ Chart Parity revenue card carries. Compact engages at
    // 10,000, so the datum formatted "24.7K" where the axis draws "25K" — five characters charged
    // against three, and the whole reason this file exists.
    what: "a compact axis whose top datum carries a decimal the tick does not",
    option: cartesian([12000, 18000, 9000, 24730]),
    categories: CATS,
  },
  {
    // The mirror, and the case `estimatePlotWidth`'s doc used to list as an over-estimate: under
    // 10,000 the numbers are grouped rather than compacted, and ECharts' round ceiling is WIDER than
    // the datum — "10,000" against "9,700". Charging the datum under-reserved here.
    what: "a grouped axis whose round ceiling is wider than its widest datum",
    option: cartesian([3100, 5200, 8000, 9700]),
    categories: CATS,
  },
  {
    // The formatter changes magnitude part way up the axis: the ceiling formats "2M" at two
    // characters while the ticks below it reach "500K" at four. A candidate taken from the ceiling
    // alone would under-reserve by two characters; the fifths are what catch it.
    what: "an axis whose ticks change magnitude suffix below the ceiling",
    option: cartesian([400000, 900000, 1100000, 1250000]),
    categories: CATS,
  },
  {
    what: "a waterfall, whose axis is the running-total envelope",
    option: () => buildWaterfallOption(
      [{ name: "x", type: "varchar" }, { name: "m", type: "double" }],
      CATS.map((c, i) => [c, [64000, 51000, 47000, 43256][i]]),
      { xAxis: "x", metric: "m", showXLabel: false, showYLabel: false },
      SIZE,
    ),
    categories: [...CATS, "Total"],
  },
  {
    // The box plot's axis is `axisWidest` — the outlier, not the box. Small numbers, so nothing is
    // compacted and the ceiling is the widest tick outright.
    what: "a box plot, whose axis is dragged out by an outlier",
    option: () => buildBoxplotOption(
      [{ name: "c", type: "varchar" }, { name: "v", type: "double" }],
      [["A", 1], ["A", 1], ["A", 1], ["A", 1], ["A", 1000]],
      { category: "c", value: "v", showXLabel: false, showYLabel: false },
      SIZE,
    ),
    categories: ["A"],
  },
];

describe("the value-axis gutter is charged for a tick the axis draws", () => {
  it.each(CASES)("$what", ({ option, categories }) => {
    gutterCalls.length = 0;
    const built = option();
    expect(gutterCalls).toHaveLength(1);

    const charged = widestTickLabel(gutterCalls[0].widest, gutterCalls[0].format);
    const drawn = longest(drawnValueTicks(built, categories));

    // Not vacuous: an empty `drawn` would clear the first bound and fail the second, but saying so
    // here means a broken extraction reads as a broken extraction.
    expect(drawn).not.toBe("");
    expect(charged.length).toBeGreaterThanOrEqual(drawn.length);
    expect(charged.length).toBeLessThanOrEqual(drawn.length);
  });
});
