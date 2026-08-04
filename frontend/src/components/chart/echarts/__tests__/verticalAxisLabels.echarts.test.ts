import { describe, it, expect, vi } from "vitest";
import * as echarts from "echarts/core";
import { platformApi } from "zrender/lib/core/platform.js";
import { registerEChartsModules } from "../register";
import { buildCartesianOption } from "../options/cartesian";
import { buildBoxplotOption } from "../options/boxplot";
import { buildWaterfallOption } from "../options/waterfall";
import { verticalValueAxisFit } from "../axisLabelFit";

/**
 * A VERTICAL CHART'S VALUE AXIS, measured against what ECharts really draws.
 *
 * The defect, read off the live 166x184 dashboard by walking every `<text>` box out of the DOM and
 * testing every pair for intersection: the BAR card drew `100K 200K 300K 400K` down a 10.7px pitch
 * with 15px label boxes — three overlapping pairs, every neighbour touching its neighbour — and the
 * BOX PLOT beside it drew `20 40 60 80` on the same pitch with the same three. Both also drew their
 * rotated `category` title straight through all four of their own category names. The line,
 * waterfall, combo and scatter cards on that same dashboard were clean, at ~21px.
 *
 * WHAT THAT DIFFERENCE IS, because it is not the numbers on the axis: a bar chart's four product
 * names turn 90° and run their own width down the card, reserving 72px of 184 and leaving a 56px
 * plot. The line chart's twelve monthly buckets thin to upright period names and leave 122. Six
 * ticks fit in 122px and do not fit in 56. So this is the row chart's defect (#390) with the axes
 * swapped, and it is fixed the same way — fewer ticks, never a rotation — but NOT with the same
 * arithmetic. See `verticalValueAxisFit` for the three terms that did not carry over.
 *
 * WHY THIS FILE RENDERS. `rowAxisLabels.test.ts` states the argument in full and it holds verbatim
 * here: how many ticks an axis draws is not what the option says (`splitNumber` is a hint ECharts
 * rounds DOWN, and `fixMinMaxLabelShow` then decides whether the two outermost survive — at
 * 166x184 the unfitted bar axis carries SIX ticks and draws four), and where an axis NAME lands
 * relative to its labels is a layout ECharts computes. Both are only observable in a render.
 *
 * THE SSR RENDER REPRODUCES THE LIVE DEFECT: at 166x184 the bar's four ticks came out centred at
 * y 33.63, 44.76, 55.90 and 67.03 — an 11.13px pitch against the browser's 10.7 — and `category`
 * at y 105.2-120.2 with `Doohickey` running 86.2-147.0.
 *
 * THE ONE FABRICATED INPUT is the font metric, fabricated identically on both sides: the mock hands
 * the builder ZRENDER'S OWN per-glyph estimator, which is what ECharts lays this SVG out with when
 * there is no canvas. So the width the builder decides from and the width ECharts draws with are
 * the same number. `rowAxisLabels.test.ts` carries the longer version of this note.
 */

vi.mock("@/components/chart/measure-text", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/components/chart/measure-text")>();
  const { platformApi: api } = await import("zrender/lib/core/platform.js");
  return {
    ...actual,
    measureText: (text: string, style: { size: string | number; family: string; weight: string | number }) => {
      const size = typeof style.size === "number" ? `${style.size}px` : style.size;
      return { width: api.measureText(text, `${style.weight} ${size} ${style.family}`).width, height: 0 };
    },
  };
});

/** The card every chart on the parity harness dashboard renders at — 4x5 on a 12-column grid, the
 *  geometry `scripts/harness/charts.ts` calls load-bearing for exactly this reason. */
const CARD = { width: 166, height: 184 };

/**
 * A DRAWN LABEL'S HEIGHT, in pixels, at the 13px every axis in this family uses.
 *
 * WRITTEN OUT RATHER THAN IMPORTED, deliberately. It is the browser measurement the defect was
 * reported from — every axis label's box on the live card came out 15px tall — and the
 * implementation's own `VALUE_LABEL_BOX_RATIO` is that same number divided by the font size. Sharing
 * one constant between the fit and the test that checks it would let a mutation halve both at once
 * and stay green; this file states the browser's answer independently, so a fit that charges less
 * than what is drawn fails here.
 *
 * zrender lays the same axis out against 13 (`getLineHeight` is the width of `国`), which is why a
 * test that read the height out of the SVG would be checking ECharts' opinion of its own labels
 * rather than the box a reader sees.
 */
const LABEL_BOX = 15;

/** `MB By Category` — the harness's own bar/row dataset, Sample DB revenue by product category.
 *  Its axis runs 0 to a nice 500K and its ticks are the `100K`-shaped strings the defect was
 *  measured on. `Doohickey`, the widest name and the one the 90° band is sized from, is LAST here
 *  and second in `rowAxisLabels.test.ts`: an implementation reaching for `labels[0]` or the last
 *  label is wrong in one of the two files whichever it picks. */
const catCols = [
  { name: "category", type: "varchar" }, { name: "orders", type: "bigint" }, { name: "revenue", type: "double" },
];
const catRows: unknown[][] = [
  ["Widget", 5061, 423_667.5], ["Gadget", 4939, 406_621.25],
  ["Gizmo", 4784, 383_061.95], ["Doohickey", 3976, 297_270.99],
];
const CATEGORIES = ["Widget", "Gadget", "Gizmo", "Doohickey"];

/** `MB Monthly` — the harness's line, area, combo and waterfall dataset. Twelve monthly buckets, so
 *  its category labels THIN to upright period names rather than turning 90°, which is the whole
 *  reason its plot is twice a bar chart's and its axis was clean where the bar's was not. */
const monthCols = [
  { name: "order_month", type: "date" }, { name: "orders", type: "bigint" },
  { name: "revenue", type: "double" }, { name: "avg_total", type: "double" },
];
const monthRows: unknown[][] = [
  ["2026-01-01", 205, 11_094.7, 54.12], ["2026-02-01", 206, 11_243.57, 54.58],
  ["2026-03-01", 256, 14_115.66, 55.14], ["2026-04-01", 238, 13_013.82, 54.68],
  ["2026-05-01", 271, 15_720.52, 58.01], ["2026-06-01", 264, 15_517.71, 58.78],
  ["2026-07-01", 325, 18_570.56, 57.14], ["2026-08-01", 333, 18_976.96, 56.99],
  ["2026-09-01", 346, 19_139.13, 55.32], ["2026-10-01", 354, 20_555.39, 58.07],
  ["2026-11-01", 394, 22_577.58, 57.3], ["2026-12-01", 418, 24_730.43, 59.16],
];

/** `MB Product Prices` — the harness's box-plot dataset, every Sample DB product price by category.
 *  200 prices between 15.69 and 98.82, which is what puts the axis on 0-100 and its ticks on the
 *  `20 40 60 80` the defect names. */
const priceCols = [{ name: "category", type: "varchar" }, { name: "price", type: "double" }];
const PRICES: Record<string, number[]> = {
  Doohickey: [
    19.87, 22.58, 24.25, 25.54, 26.96, 27.55, 28.33, 29.52, 31.73, 31.83, 33.59, 35.15, 35.39,
    39.69, 42.67, 44.43, 45.55, 45.65, 46.9, 47.69, 48.93, 51.19, 51.94, 53.29, 56.05, 61.54,
    61.98, 64.96, 65.14, 65.89, 69.88, 70.05, 70.08, 70.44, 73.99, 74.87, 74.95, 75.97, 78.22,
    78.7, 84.14, 98.82,
  ],
  Gadget: [
    20.41, 22.38, 26.38, 28.96, 29.18, 31.36, 31.73, 33.4, 33.4, 33.59, 34.09, 35.15, 37.02, 38.42,
    40.38, 40.6, 42.78, 44.05, 44.07, 44.53, 44.54, 45.22, 45.55, 48.8, 49.54, 51.94, 52.1, 53.41,
    54.58, 54.6, 55.2, 56.33, 63.85, 67.34, 71.43, 73.19, 73.65, 74.41, 78.7, 79.29, 81.58, 82.75,
    83.88, 85.02, 85.7, 85.72, 85.72, 85.88, 87.62, 88.3, 92.65, 93.22, 95.61,
  ],
  Gizmo: [
    15.69, 21.42, 24.88, 25.54, 27.5, 27.74, 27.74, 29.29, 29.46, 30.54, 31.79, 33.21, 36.6, 36.68,
    37.8, 40.8, 40.8, 43.4, 45.18, 45.22, 45.42, 46.76, 46.76, 50.02, 50.05, 50.43, 54.58, 59.47,
    61.08, 65.98, 65.98, 66.44, 66.56, 70.28, 73.95, 73.95, 74.95, 75.09, 76.28, 76.39, 76.83,
    77.34, 81.58, 81.61, 82.08, 82.14, 83.26, 87.29, 89.01, 93.26, 98.78,
  ],
  Widget: [
    22.38, 23.69, 25.1, 25.1, 25.34, 31.36, 32.6, 33.21, 35.76, 36.61, 37.02, 38.33, 38.62, 40.8,
    41.4, 42.34, 42.48, 42.55, 45.42, 45.49, 45.75, 46.1, 46.12, 46.76, 47.75, 50.9, 50.9, 53.6,
    54.58, 55.67, 58.31, 62.18, 69.12, 70.05, 70.14, 70.96, 72.81, 75.41, 76.95, 77.4, 77.91,
    78.17, 78.22, 79.37, 82.38, 83.5, 84.03, 85.25, 89.01, 89.65, 90.77, 93.26, 94.8, 95.93,
  ],
};
const priceRows: unknown[][] = Object.entries(PRICES).flatMap(([c, vs]) => vs.map((v) => [c, v]));

const barCfg = { kind: "bar" as const, xAxis: "category", yMetrics: ["revenue"] };
const lineCfg = { kind: "line" as const, xAxis: "order_month", yMetrics: ["revenue"], xAxisUnit: "month" };
const comboCfg = {
  kind: "combo" as const, xAxis: "order_month", yMetrics: ["orders", "revenue"], xAxisUnit: "month",
  seriesTypes: { revenue: "line" as const }, rightAxisMetrics: ["revenue"],
};
const scatterCfg = {
  kind: "scatter" as const, xAxis: "category", xMetric: "orders", yMetrics: ["revenue"],
};
const boxCfg = { category: "category", value: "price" };
const wfCfg = { xAxis: "order_month", metric: "revenue", xAxisUnit: "month" };

interface SvgText {
  text: string;
  /** The translate's x, or the rotated matrix's `e`. */
  x: number;
  /** The translate's y, or the rotated matrix's `f`. */
  y: number;
  /** The `y` attribute — a LOCAL offset applied before the transform, so on a rotated name it moves
   *  the ink sideways and on an upright one it moves it down. */
  dy: number;
  anchor: string;
  /** `true` for `matrix(0,-1,1,0,…)`, i.e. text turned onto the vertical. */
  rotated: boolean;
}

function parseTexts(svg: string): SvgText[] {
  return [...svg.matchAll(/<text\b([^>]*)>([^<]*)<\/text>/g)].map(([, attrs, text]) => {
    const translate = /transform="translate\((-?[\d.]+) (-?[\d.]+)\)"/.exec(attrs);
    const matrix = /transform="matrix\(0,-1,1,0,(-?[\d.]+),(-?[\d.]+)\)"/.exec(attrs);
    return {
      text,
      x: translate ? Number(translate[1]) : matrix ? Number(matrix[1]) : NaN,
      y: translate ? Number(translate[2]) : matrix ? Number(matrix[2]) : NaN,
      dy: Number(/\sy="(-?[\d.]+)"/.exec(attrs)?.[1] ?? 0),
      anchor: /text-anchor="([a-z]+)"/.exec(attrs)?.[1] ?? "start",
      rotated: matrix != null,
    };
  }).filter((t) => t.text.trim() !== "");
}

const familyOf = (svg: string) => /font-family:([^;"]+)/.exec(svg)?.[1] ?? "sans-serif";

interface Box { x: [number, number]; y: [number, number] }

/**
 * The rectangle of page a text's ink occupies, both dimensions — the same box the live DOM audit
 * read, reconstructed from the transform ECharts emitted.
 *
 * Every label in this family is drawn `dominant-baseline: central`, so the anchor is the box's
 * vertical CENTRE and `LABEL_BOX` straddles it. `text-anchor` places it along the advance
 * direction. A rotated name maps its local axes onto the page's as `pageX = e + ly`,
 * `pageY = f − lx`, which is why its own `y` attribute pushes it sideways.
 */
function inkBox(t: SvgText, family: string): Box {
  const w = platformApi.measureText(t.text, `400 13px ${family}`).width;
  const along: [number, number] = t.anchor === "end" ? [-w, 0]
    : t.anchor === "middle" ? [-w / 2, w / 2] : [0, w];
  const across: [number, number] = [t.dy - LABEL_BOX / 2, t.dy + LABEL_BOX / 2];
  return t.rotated
    ? { x: [t.x + across[0], t.x + across[1]], y: [t.y - along[1], t.y - along[0]] }
    : { x: [t.x + along[0], t.x + along[1]], y: [t.y + across[0], t.y + across[1]] };
}

/** Two boxes intersect when they overlap on BOTH axes — the test the DOM audit ran over every pair,
 *  and not "their y bands touch", which would call a title beside its labels a collision. */
const intersects = (a: Box, b: Box) =>
  a.x[0] < b.x[1] && b.x[0] < a.x[1] && a.y[0] < b.y[1] && b.y[0] < a.y[1];

function render(option: unknown, size: { width: number; height: number }): string {
  registerEChartsModules();
  const chart = echarts.init(null, null, { renderer: "svg", ssr: true, ...size });
  chart.setOption({ ...(option as object), animation: false });
  const svg = chart.renderToSVGString();
  chart.dispose();
  return svg;
}

/**
 * The VALUE-axis tick labels a vertical chart drew, top to bottom, with the box each occupies.
 *
 * Identified by geometry rather than by a list of strings, because the strings are what is under
 * test: an upright label anchored at one of its ENDS is what a value axis draws and nothing else on
 * these cards is. Both ends, not just `end` — a dual-axis combo anchors its right-hand ticks at
 * `start`, and a fit that reached one axis and missed the other would look clean to a selector that
 * only saw the left one. Everything else here is `middle`: the category labels, the axis names, the
 * legend. `notTicks` removes text that shares the shape — nothing does today, and the claim is
 * asserted rather than assumed by `it("draws the ticks this file thinks it does")`.
 */
function valueTicks(svg: string, notTicks: string[] = []): { text: string; box: Box }[] {
  const family = familyOf(svg);
  return parseTexts(svg)
    .filter((t) => !t.rotated && t.anchor !== "middle" && !notTicks.includes(t.text))
    .map((t) => ({ text: t.text, box: inkBox(t, family) }))
    .sort((a, b) => a.box.y[0] - b.box.y[0]);
}

/** Every pair of value ticks that overlaps — the quantity the live audit reported as 3 for the bar
 *  and 3 for the box plot. */
function overlappingTickPairs(svg: string, notTicks?: string[]): string[] {
  const ticks = valueTicks(svg, notTicks);
  const bad: string[] = [];
  for (let i = 0; i < ticks.length; i++) {
    for (let j = i + 1; j < ticks.length; j++) {
      if (intersects(ticks[i].box, ticks[j].box)) bad.push(`${ticks[i].text}/${ticks[j].text}`);
    }
  }
  return bad;
}

/** The bottom axis's own name, or a thrown error — a title that was never drawn must not read as a
 *  comfortable gap. */
function bottomTitle(svg: string, name: string): SvgText {
  const t = parseTexts(svg).find((x) => x.text === name && !x.rotated);
  if (!t) throw new Error(`the ${name} axis title was not drawn upright`);
  return t;
}

/** The category labels the bottom axis's title is drawn over — the live defect reported all four. */
function labelsUnderTitle(svg: string, title: string, categories: string[]): string[] {
  const family = familyOf(svg);
  const box = inkBox(bottomTitle(svg, title), family);
  const labels = parseTexts(svg).filter((t) => categories.includes(t.text));
  if (labels.length !== categories.length) throw new Error(`drew ${labels.length} category labels`);
  return labels.filter((l) => intersects(box, inkBox(l, family))).map((l) => l.text);
}

/**
 * The same option with BOTH halves of the fit taken back off — what ECharts drew on the live
 * dashboard, and the only way a render-based test can show that its own fixture is capable of
 * failing.
 *
 * SPREAD RATHER THAN CLONED, because the option carries the axis FORMATTERS as functions and a
 * round trip through JSON drops them: an axis that lost its formatter prints `423667.5` where the
 * card printed `400K`, which is a different and much wider label than the one the defect was
 * measured on.
 */
function withoutFit(option: unknown): unknown {
  const o = option as { xAxis: Record<string, unknown>; yAxis: unknown };
  const strip = (ax: Record<string, unknown>) => {
    const { interval: _i, min: _lo, max: _hi, ...rest } = ax;
    return rest;
  };
  return {
    ...(option as object),
    xAxis: { ...o.xAxis, nameGap: 28 },
    yAxis: Array.isArray(o.yAxis)
      ? (o.yAxis as Record<string, unknown>[]).map(strip)
      : strip(o.yAxis as Record<string, unknown>),
  };
}

describe("a vertical chart's value axis fits the height it has", () => {
  /**
   * THE FIXTURE IS NOT BLIND, asserted before anything else claims to fix it: the very option this
   * file builds, with only the fit removed, reproduces the live card. Three overlapping pairs on the
   * bar and three on the box plot, which is the count the DOM audit reported for each.
   *
   * A render-based test cannot otherwise show that its data is capable of failing — a value range
   * that happened to produce two ticks, or a card that happened to be tall, would pass every case
   * below while proving nothing.
   */
  it.each([
    ["bar", () => buildCartesianOption(catCols, catRows, barCfg, CARD)],
    ["box plot", () => buildBoxplotOption(priceCols, priceRows, boxCfg, CARD)],
  ])("reproduces the live %s defect once the fit is taken off", (_what, build) => {
    expect(overlappingTickPairs(render(withoutFit(build()), CARD))).toHaveLength(3);
  });

  /** …and with it on, no two of them touch. */
  it.each([
    ["bar", () => buildCartesianOption(catCols, catRows, barCfg, CARD)],
    ["box plot", () => buildBoxplotOption(priceCols, priceRows, boxCfg, CARD)],
  ])("draws no two value ticks over each other on a %s", (_what, build) => {
    const svg = render(build(), CARD);
    expect(valueTicks(svg).length).toBeGreaterThan(0);
    expect(overlappingTickPairs(svg)).toEqual([]);
  });

  /**
   * The property rather than the count, across the heights a dashboard card really takes. Height is
   * what this fit is budgeted in, so the sweep is over heights — the mirror of
   * `rowAxisLabels.test.ts`, which sweeps widths for the same reason.
   */
  it.each([160, 172, 184, 200, 220, 240, 260, 300, 360, 480])(
    "draws no two value ticks over each other on a bar chart %ipx tall", (height) => {
      const size = { width: 166, height };
      expect(overlappingTickPairs(render(buildCartesianOption(catCols, catRows, barCfg, size), size)))
        .toEqual([]);
    });

  it.each([160, 172, 184, 200, 220, 240, 260, 300, 360, 480])(
    "draws no two value ticks over each other on a box plot %ipx tall", (height) => {
      const size = { width: 166, height };
      expect(overlappingTickPairs(render(buildBoxplotOption(priceCols, priceRows, boxCfg, size), size)))
        .toEqual([]);
    });

  /**
   * The second half of the defect, and the same one #390 fixed on the row chart's other axis:
   * `nameGap` is measured from the AXIS LINE, and on a vertical chart with rotated category names
   * the labels run down into exactly the band a flat 28 puts the title in. Live, `category` sat at
   * y 105.2-120.2 while its names ran 86.2-147.0 — intersecting all four.
   */
  it.each([
    ["bar", () => buildCartesianOption(catCols, catRows, barCfg, CARD), CATEGORIES],
    ["box plot", () => buildBoxplotOption(priceCols, priceRows, boxCfg, CARD), Object.keys(PRICES)],
  ])("draws the %s's title over all four category labels once the fit is taken off",
    (_what, build, cats) => {
      expect(labelsUnderTitle(render(withoutFit(build()), CARD), "category", cats)).toHaveLength(4);
    });

  it.each([
    ["bar", () => buildCartesianOption(catCols, catRows, barCfg, CARD), CATEGORIES],
    ["box plot", () => buildBoxplotOption(priceCols, priceRows, boxCfg, CARD), Object.keys(PRICES)],
  ])("keeps the %s's axis title clear of its category labels", (_what, build, cats) => {
    expect(labelsUnderTitle(render(build(), CARD), "category", cats)).toEqual([]);
  });

  /**
   * The mutation the case above cannot catch on its own: any `nameGap` big enough clears the labels,
   * including one that walks the title off the bottom of the card. Both halves are asserted so that
   * "it cleared them" cannot be satisfied by drawing the title where nobody can see it.
   */
  it.each([
    ["bar", (s: typeof CARD) => buildCartesianOption(catCols, catRows, barCfg, s)],
    ["box plot", (s: typeof CARD) => buildBoxplotOption(priceCols, priceRows, boxCfg, s)],
  ])("keeps the %s's title on the canvas while it does so", (_what, build) => {
    for (const height of [160, 184, 240, 400]) {
      const size = { width: 166, height };
      const svg = render(build(size), size);
      const box = inkBox(bottomTitle(svg, "category"), familyOf(svg));
      expect(box.y[1]).toBeLessThanOrEqual(height);
      expect(box.y[0]).toBeGreaterThanOrEqual(0);
    }
  });

  /**
   * …AND KEEPS IT THERE ON A CARD TOO SHORT TO AFFORD THE CLEARING GAP, which the sweep above cannot
   * show because those cards can all afford it. ECharts stops shrinking a plot at zero height, and
   * from there the axis line cannot rise any further while the labels below it keep growing — so
   * past a certain card the gap that clears them puts the title off the bottom. There the title
   * falls back to the flat 28 and is drawn ACROSS its labels, which is this fix's own defect and
   * deliberately the accepted outcome: a title one can read over the top of beats one that is not
   * there. Both halves asserted, so "it fell back" cannot be satisfied by drawing nothing.
   */
  it("falls back to the flat gap on a card too short for it", () => {
    const size = { width: 166, height: 110 };
    const opt = buildCartesianOption(catCols, catRows, barCfg, size) as { xAxis: { nameGap: number } };
    expect(opt.xAxis.nameGap).toBe(28);
    const svg = render(opt, size);
    expect(inkBox(bottomTitle(svg, "category"), familyOf(svg)).y[1]).toBeLessThanOrEqual(size.height);
  });

  /**
   * The other side of that threshold, so the fallback is a decision about the CARD rather than a
   * rule that quietly retired the fix: the same four names on a taller card clear as they should.
   */
  it("still clears the same labels once the card is tall enough", () => {
    const size = { width: 166, height: 184 };
    expect((buildCartesianOption(catCols, catRows, barCfg, size) as { xAxis: { nameGap: number } })
      .xAxis.nameGap).toBeGreaterThan(28);
  });

  /**
   * THE FOUR THAT WERE ALREADY CLEAN MUST STAY CLEAN, and stay clean by the fit saying NOTHING —
   * `verticalValueAxisFit` is a ceiling, so a card with room for ECharts' own default keeps the
   * option it built before this existed. All four run on `MB Monthly`, whose twelve monthly
   * buckets thin to upright period names and leave a plot twice the bar chart's.
   *
   * The combo is the one that needs BOTH its axes checked: it is the only dual-axis chart on the
   * harness, so it is the only one where a fit written for "the value axis" could reach one and miss
   * the other — or reach the right-hand one with the left-hand one's extent.
   */
  it.each([
    ["line", () => buildCartesianOption(monthCols, monthRows, lineCfg, CARD)],
    ["area", () => buildCartesianOption(monthCols, monthRows, { ...lineCfg, kind: "area" }, CARD)],
    ["combo", () => buildCartesianOption(monthCols, monthRows, comboCfg, CARD)],
    ["waterfall", () => buildWaterfallOption(monthCols, monthRows, wfCfg, CARD)],
    ["scatter", () => buildCartesianOption(catCols, catRows, scatterCfg, CARD)],
  ])("leaves the %s card's value axis exactly as it was", (_what, build) => {
    const axes = [(build() as { yAxis: unknown }).yAxis].flat() as { interval?: number }[];
    for (const ax of axes) expect(ax.interval).toBeUndefined();
    expect(overlappingTickPairs(render(build(), CARD))).toEqual([]);
  });

  /**
   * The other direction on the fit's own charts, and the reason it is a CEILING rather than a count:
   * a bar chart with room for ECharts' default ticks must keep them, and must keep them by saying
   * nothing. A fix that simply asked for fewer ticks everywhere would pass every case above and
   * quietly strip every tall bar chart down to two.
   */
  it("leaves a card that can afford them alone", () => {
    const tall = { width: 400, height: 500 };
    const opt = buildCartesianOption(catCols, catRows, barCfg, tall) as
      { yAxis: { interval?: number; splitNumber?: number } };
    expect(opt.yAxis.interval).toBeUndefined();
    expect(opt.yAxis.splitNumber).toBeUndefined();
    expect(valueTicks(render(opt, tall), ["revenue"]).length).toBeGreaterThanOrEqual(4);
  });

  /**
   * The measured fit is a fallback, not an override: a user who set a tick count has said what they
   * want, and it has to arrive ALONE — an `interval` beside it would win over it in ECharts.
   */
  it.each([
    ["bar", (c: object) => buildCartesianOption(catCols, catRows, { ...barCfg, ...c }, CARD)],
    ["box plot", (c: object) => buildBoxplotOption(priceCols, priceRows, { ...boxCfg, ...c }, CARD)],
    ["waterfall", (c: object) => buildWaterfallOption(monthCols, monthRows, { ...wfCfg, ...c }, CARD)],
  ])("gives way to a tick count the user set on a %s", (_what, build) => {
    const opt = build({ yTickCount: 4 }) as { yAxis: { interval?: number; splitNumber?: number } };
    expect(opt.yAxis.splitNumber).toBe(4);
    expect(opt.yAxis.interval).toBeUndefined();
  });

  /**
   * A LOG axis draws powers, so a linear interval means something else entirely there — the same
   * exclusion the row chart's fit keeps, asked on the other axis.
   */
  it.each([
    ["bar", (c: object) => buildCartesianOption(catCols, catRows, { ...barCfg, ...c }, CARD)],
    ["box plot", (c: object) => buildBoxplotOption(priceCols, priceRows, { ...boxCfg, ...c }, CARD)],
    ["waterfall", (c: object) => buildWaterfallOption(monthCols, monthRows, { ...wfCfg, ...c }, CARD)],
  ])("says nothing about a %s's log axis", (_what, build) => {
    expect((build({ logScale: true }) as { yAxis: { interval?: number } }).yAxis.interval)
      .toBeUndefined();
  });

  /**
   * A ROW CHART'S VALUE AXIS IS THE BOTTOM ONE, where a label's HEIGHT costs nothing and this fit
   * has nothing to say. Its own fit (#390) owns that axis, and the two must not both reach it — an
   * `interval` written twice is one of them silently losing.
   */
  it("says nothing about a row chart's value axis", () => {
    const opt = buildCartesianOption(catCols, catRows, { ...barCfg, kind: "row", horizontal: true }, CARD) as
      { yAxis: { interval?: number }; xAxis: { interval?: number } };
    expect(opt.yAxis.interval).toBeUndefined();
    expect(opt.xAxis.interval).toBeDefined();
  });

  /**
   * A WATERFALL OVER NOMINAL STEPS is the case that brings the third builder in, and it is not the
   * harness's own card — that one is twelve monthly buckets, thins to upright period names and is
   * left alone above. Turn the same chart onto product categories and its labels rotate 90° exactly
   * as a bar chart's do, reserve the same band, and crowd the same axis. Without the fit wired into
   * `options/waterfall.ts` this draws four overlapping pairs.
   */
  it("fits a waterfall whose steps are names rather than periods", () => {
    const build = () => buildWaterfallOption(catCols, catRows, { xAxis: "category", metric: "revenue" }, CARD);
    expect(overlappingTickPairs(render(withoutFit(build()), CARD)).length).toBeGreaterThan(0);
    expect(overlappingTickPairs(render(build(), CARD))).toEqual([]);
    expect(labelsUnderTitle(render(withoutFit(build()), CARD), "category", CATEGORIES)).toHaveLength(4);
    expect(labelsUnderTitle(render(build(), CARD), "category", CATEGORIES)).toEqual([]);
  });

  /**
   * THE ENDS THE FIT NAMES ARE ENDS ITS OWN INTERVAL DIVIDES, which is the property the whole
   * `min`/`max` half exists for and the one a render cannot pin on its own — a fit that named the
   * data's raw ends instead would leave a stub last step that happens, on some data, to be small
   * enough for ECharts to drop by itself. That is luck rather than a rule: it depends on how far the
   * top datum sits from a round number, and the axis then tops out at `423.7K` instead of a tick.
   *
   * Also that the ends only ever WIDEN, in both directions, so nothing can be clipped by them.
   */
  it("names ends its own interval divides, and never inside the data", () => {
    for (const [lo, hi] of [
      [0, 423_667.5], [-290_000, 300_000], [0, 98.82], [0, 1_117_000], [-40.5, 60.25],
    ]) {
      const fit = verticalValueAxisFit({ plotHeight: 56, lo, hi });
      expect(fit).toBeDefined();
      expect(Math.abs(fit!.min % fit!.interval)).toBe(0);
      expect(Math.abs(fit!.max % fit!.interval)).toBe(0);
      expect(fit!.min).toBeLessThanOrEqual(lo);
      expect(fit!.max).toBeGreaterThanOrEqual(hi);
    }
  });

  /**
   * A HIDDEN CATEGORY AXIS GIVES ITS BAND BACK TO THE VALUE TICKS, which is the last rung of the
   * x-label ladder meeting this fit: past a certain crowding `fitXLabels` draws no category label at
   * all, and a band computed from labels that are not on the card would charge the plot for room
   * ECharts did not reserve. Measured against real ECharts: four names cost 21px below the plot
   * upright and 6px hidden, so charging the upright band to a hidden axis is 15px of plot thrown
   * away — one whole rung at this size, which is what this case reads out as a tick count.
   *
   * A COUNT AND NOT AN OVERLAP, deliberately: both answers draw a clean axis here (5 ticks at 15.6px
   * and 3 at 31px), so the only observable difference is how much of the card the fit believed it
   * had. Eight long names on a short card is what reaches the ladder's `hide` rung — see
   * `axisLabelFit.test.ts` for the rung itself.
   */
  it("spends a hidden category axis's band on ticks rather than reserving it", () => {
    const size = { width: 166, height: 125 };
    const cols = [{ name: "category", type: "varchar" }, { name: "revenue", type: "double" }];
    const rows: unknown[][] = Array.from({ length: 8 }, (_, i) =>
      [`Doohickey Trading ${i}`, 100 * (i + 1)]);
    const opt = buildCartesianOption(cols, rows, barCfg, size) as
      { xAxis: { axisLabel: { show?: boolean } } };
    // The premise: this really is the ladder's hide rung, not merely a crowded axis.
    expect(opt.xAxis.axisLabel.show).toBe(false);
    const drawn = valueTicks(render(opt, size));
    expect(overlappingTickPairs(render(opt, size))).toEqual([]);
    expect(drawn.length).toBeGreaterThanOrEqual(4);
  });

  /**
   * A CARD WITH ROOM FOR NOT EVEN ONE STEP still has to answer with numbers. At 140px the four 90°
   * names leave ~8px of plot, which is under one label box, and the honest floor is the two ends an
   * axis always draws — `intervalThatFits` says the same about its own last rung, and that those two
   * touching is a case for a bigger card rather than for a cleverer fit. What must NOT happen is a
   * division by zero steps reaching the option: `Infinity` divides into a `NaN` min and max, and an
   * axis given those draws nothing at all.
   */
  it("keeps the interval and both ends finite on a card with no room for a step", () => {
    const size = { width: 166, height: 140 };
    const y = (buildCartesianOption(catCols, catRows, barCfg, size) as
      { yAxis: { interval?: number; min?: number; max?: number } }).yAxis;
    for (const n of [y.interval, y.min, y.max]) {
      expect(n).toBeDefined();
      expect(Number.isFinite(n!)).toBe(true);
    }
    expect(valueTicks(render(buildCartesianOption(catCols, catRows, barCfg, size), size)).length)
      .toBeGreaterThan(0);
  });

  /**
   * The fitter is EXPORTED and three builders call it. A height that is not a number is not a short
   * card, and `<= 0` is false for NaN, so the guards say `> 0`: an unmeasured input has to come back
   * as "no answer" rather than as an interval of NaN quietly reaching an option.
   */
  it("answers nothing rather than NaN when it is handed one", () => {
    expect(verticalValueAxisFit({ plotHeight: NaN, lo: 0, hi: 500_000 })).toBeUndefined();
    expect(verticalValueAxisFit({ plotHeight: 56, lo: 0, hi: NaN })).toBeUndefined();
    expect(verticalValueAxisFit({ plotHeight: 56, lo: 0, hi: 0 })).toBeUndefined();
  });

  /**
   * What `valueTicks` above believes it is selecting, stated rather than assumed: on the bar card
   * the upright end-anchored texts ARE the value ticks and nothing else. If a future option adds
   * another one, every "no two overlap" case above would start counting it as a tick and this fails
   * first, with the string in the message.
   */
  it("draws the ticks this file thinks it does", () => {
    const drawn = valueTicks(render(buildCartesianOption(catCols, catRows, barCfg, CARD), CARD));
    expect(drawn.every((t) => /^[\d.,]+[KMB]?$/.test(t.text))).toBe(true);
    expect(drawn.length).toBeGreaterThan(0);
  });
});
