import { describe, it, expect, vi } from "vitest";
import * as echarts from "echarts/core";
import { platformApi } from "zrender/lib/core/platform.js";
import { registerEChartsModules } from "../register";
import { buildCartesianOption } from "../options/cartesian";
import { intervalThatFits } from "../axisLabelFit";

/**
 * A ROW CHART'S TWO AXES, each measured against what ECharts really draws.
 *
 * The defect, read off the live 166x184 dashboard card: the bottom axis drew `100K 200K 300K 400K`
 * on an 8px pitch — four labels ~31px wide, each overlapping its neighbour by 23px — while the
 * rotated `category` title sat on top of the very labels it names. Metabase's row chart at that size
 * draws ONE tick.
 *
 * WHY THIS FILE BOOTS REAL ECHARTS, though its name does not say so (`rowAxisLabels.test.ts` is the
 * name the plan gave it; `*.echarts.test.ts` is this directory's usual marker for a file that
 * renders, and `valueAxisGutter.echarts.test.ts` is the precedent this follows). Two reasons, and
 * neither has an option-side substitute:
 *
 *   1. HOW MANY TICKS AN AXIS DRAWS IS NOT WHAT THE OPTION SAYS. `splitNumber` is a hint ECharts
 *      rounds, `interval` is obeyed but the extent it divides is ECharts' own, and
 *      `fixMinMaxLabelShow` then decides whether the two outermost survive. An assertion written
 *      against the option would pin this file's PREDICTION of that machinery rather than the
 *      machinery — the mistake `valueAxisGutter.echarts.test.ts` exists because a previous task
 *      made, and this task made again: the first implementation asked for `splitNumber: 3` on a
 *      250px card and ECharts drew five labels, still overlapping. Only the render caught it.
 *   2. Where the axis NAME lands relative to the labels is a layout ECharts computes from `nameGap`,
 *      the label margin and the grid. Recomputing it here from `nameGap` alone would be the
 *      implementation's own arithmetic restated as an assertion.
 *
 * THE SSR RENDER REPRODUCES THE LIVE DEFECT to within a pixel, which is what makes it worth
 * trusting: at 166x184 it drew `100K 200K 300K 400K` centred at x = 114.7, 123.5, 132.3, 141.2
 * (an 8.8px pitch against the browser's 8px) and put `category` at x 64.8-77.8 with `Doohickey`
 * running 37-97.8 — the same overlap, the same numbers.
 *
 * THE ONE FABRICATED INPUT is the font metric, and it is fabricated identically on both sides.
 * `measure-text.ts` returns `{width: 0}` under jsdom by design ("Size claims belong in a browser"),
 * so the builder would measure every label at 0px and decline to decide anything — see
 * `cartesianXLabels.test.ts`'s header, which is why that file can assert no measured rotation. The
 * mock below hands the builder ZRENDER'S OWN estimator, which is not a stub invented here: it is a
 * per-glyph width table (`DEFAULT_TEXT_WIDTH_MAP`, scaled by font size) and it is exactly what
 * ECharts itself uses to lay out this SVG when there is no canvas. So the width the builder decides
 * from and the width ECharts draws with are the same number, and the geometry below is internally
 * consistent rather than two estimates hoping to agree.
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

const cols = [{ name: "category", type: "varchar" }, { name: "revenue", type: "double" }];
/**
 * The live card's four categories and a revenue peaking in the hundreds of thousands, so the axis
 * formats compactly and its ticks are the `100K`-shaped strings the defect was measured on.
 *
 * THE ORDER IS DELIBERATE. `Doohickey` — the widest name, and the one both the plot-width estimate
 * and the title's clearance have to be sized against — is SECOND. First or last and an
 * implementation that reached for `labels[0]` or the last label would have been handed the right
 * answer by the fixture.
 *
 * THE PEAK IS NOT ROUND, for the same reason: 412,000 is not a tick the axis draws, so an
 * implementation that sized its interval against the datum rather than against the extent ECharts
 * rounds to cannot pass by coincidence.
 */
const rows: unknown[][] = [
  ["Widget", 412_000], ["Doohickey", 128_000], ["Gizmo", 341_000], ["Gadget", 236_000],
];
/**
 * The same four categories with two of them in the red — the axis runs -290K to 300K, so it is 590K
 * across while its biggest number is 300K.
 *
 * THE NEAR-SYMMETRY IS THE POINT, and the fixture it replaced did not have it. That one ran -188K to
 * 412K: 600K across against a biggest number of 412K, and on the {1,2,5} lattice of round intervals
 * a factor of 1.5 disappears — 600/2 and 412/2 both round up to 500K, at every width the case
 * covered. It asserted a distinction its own numbers could not express, which is why the mutant it
 * names survived. At a factor of 2 the two land a rung apart (590/2 -> 500K, 300/2 -> 200K), and an
 * axis given the smaller interval draws five labels where three fit.
 *
 * A LEFT-HAND MAGNITUDE THIS FIXTURE DOES NOT PROBE: the minus sign. `widestTickLabel` samples
 * positives, so `-290K` is charged as `290K`. That under-charge is `estimatePlotWidth`'s own
 * documented residual, is shared with the vertical ladder, and is not this task's to fix — the
 * numbers above hold with or without it.
 */
const crossesZero: unknown[][] = [
  ["Widget", 300_000], ["Doohickey", -290_000], ["Gizmo", 180_000], ["Gadget", -96_000],
];
const CATEGORIES = ["Widget", "Doohickey", "Gizmo", "Gadget"];
const rowCfg = { kind: "row" as const, xAxis: "category", yMetrics: ["revenue"] };
/** The two axis names the builder writes: the category axis is titled from the x column and the
 *  value axis from the metric. Both are drawn, so both have to be excluded from the tick count. */
const TITLES = ["category", "revenue"];
const CARD = { width: 166, height: 184 };

/**
 * THREE METRICS PILED UP, which is the case an extent taken from individual data points gets wrong:
 * the biggest single number is 300K and the tallest bar is 870K, so ECharts runs the axis to 1M.
 *
 * Sized against the datum, the fit asked for `interval: 100000` — nine labels where six fitted — and
 * a stacked row chart that had drawn CLEAN ticks at 300 and 320px before this fit existed came out
 * overlapping by 13.9 and 11.9px. A regression, not a shortfall, and the reason the extent is summed
 * per category rather than scanned per point.
 *
 * The three metrics also put a LEGEND on the card, so its entries join the strings a tick count has
 * to exclude — see `ticksDrawn`.
 */
const stackCols = [
  { name: "category", type: "varchar" }, { name: "north", type: "double" },
  { name: "south", type: "double" }, { name: "east", type: "double" },
];
const stackRows: unknown[][] = [
  ["Widget", 300_000, 290_000, 280_000], ["Doohickey", 120_000, 130_000, 110_000],
  ["Gizmo", 250_000, 240_000, 230_000], ["Gadget", 180_000, 170_000, 160_000],
];
const stackCfg = {
  kind: "row" as const, xAxis: "category", yMetrics: ["north", "south", "east"], stacked: true,
};
/** A three-metric axis has no single name, so the value axis is untitled here — what has to come out
 *  of the tick count instead is the category title and the three legend entries. */
const STACK_TEXT = ["category", "north", "south", "east"];

/**
 * A stack that grows BOTH WAYS: two metrics up and one down, so the tallest category piles to +590K
 * above the baseline and -280K below it and the axis runs 870K. Netted into one figure it would read
 * 310K — a third of the truth, and an interval a third of the size.
 *
 * ECharts stacks the two directions separately (positives up, negatives down from the same
 * baseline), which is why the extent keeps two piles rather than a running total.
 *
 * NO LEGEND, and that is not tidying: at some widths a scrollable legend adds its own `1/3` pager to
 * the markup, and a pager is neither a tick nor a name this file can enumerate. Excluding text by
 * spelling it out is what keeps `ticksDrawn` honest, so the fixture removes the source instead. The
 * legend is above the plot; nothing horizontal changes with it gone.
 */
const divergingStackCfg = { ...stackCfg, showLegend: false };
const divergingStackRows: unknown[][] = [
  ["Widget", 300_000, 290_000, -280_000], ["Doohickey", 120_000, 130_000, -110_000],
  ["Gizmo", 250_000, 240_000, -230_000], ["Gadget", 180_000, 170_000, -160_000],
];

/** A category name wide enough that the card cannot hold it, its title and a plot at once. At 166px
 *  the clearing gap would put the title at x -1; at 250px the same name clears with room to spare,
 *  which is what makes the pair a bracket around the threshold rather than one anecdote. */
const LONG_NAME = "Doohickey Trading";
const longNameRows: unknown[][] = [
  ["Widget", 412_000], [LONG_NAME, 128_000], ["Gizmo", 341_000], ["Gadget", 236_000],
];

interface SvgText {
  text: string;
  /** The transform's translate, or the matrix's `e` for the rotated axis name. */
  x: number;
  /** The `y` attribute — a local offset applied BEFORE the transform, which is why a rotated name's
   *  own vertical offset moves it horizontally on the page. */
  dy: number;
  anchor: string;
  fontSize: number;
  /** The matrix's `c`, i.e. how much of the local y axis points along the page's x. 0 for an
   *  unrotated `translate(...)`. */
  c: number;
}

function parseTexts(svg: string): SvgText[] {
  return [...svg.matchAll(/<text\b([^>]*)>([^<]*)<\/text>/g)].map(([, attrs, text]) => {
    const translate = /transform="translate\((-?[\d.]+) (-?[\d.]+)\)"/.exec(attrs);
    const matrix = /transform="matrix\((-?[\d.]+),(-?[\d.]+),(-?[\d.]+),(-?[\d.]+),(-?[\d.]+),(-?[\d.]+)\)"/.exec(attrs);
    return {
      text,
      x: translate ? Number(translate[1]) : matrix ? Number(matrix[5]) : NaN,
      dy: Number(/\sy="(-?[\d.]+)"/.exec(attrs)?.[1] ?? 0),
      anchor: /text-anchor="([a-z]+)"/.exec(attrs)?.[1] ?? "start",
      fontSize: Number(/font-size:([\d.]+)px/.exec(attrs)?.[1] ?? 0),
      c: matrix ? Number(matrix[3]) : 0,
    };
  });
}

/** The family the SVG says it drew in, so the widths below are measured at the font on the page
 *  rather than at one this file assumed. */
const familyOf = (svg: string) => /font-family:([^;"]+)/.exec(svg)?.[1] ?? "sans-serif";

/** The horizontal band of page a text's ink occupies. An upright label is as wide as its glyphs
 *  measure, anchored per `text-anchor`; a name rotated onto the y axis is as wide as its font is
 *  tall, and its `y` offset is what pushes it sideways (`x' = c*y + e`). */
function inkX(t: SvgText, family: string): [number, number] {
  if (t.c !== 0) {
    const ends = [t.x + t.c * (t.dy - t.fontSize / 2), t.x + t.c * (t.dy + t.fontSize / 2)];
    return [Math.min(...ends), Math.max(...ends)];
  }
  const w = platformApi.measureText(t.text, `400 ${t.fontSize}px ${family}`).width;
  if (t.anchor === "end") return [t.x - w, t.x];
  if (t.anchor === "middle") return [t.x - w / 2, t.x + w / 2];
  return [t.x, t.x + w];
}

function render(option: unknown, size: { width: number; height: number }): string {
  registerEChartsModules();
  const chart = echarts.init(null, null, { renderer: "svg", ssr: true, ...size });
  chart.setOption({ ...(option as object), animation: false });
  const svg = chart.renderToSVGString();
  chart.dispose();
  return svg;
}

const rowOption = (size: { width: number; height: number }, data: unknown[][] = rows) =>
  buildCartesianOption(cols, data, rowCfg, size);

/** The value-axis tick labels the chart DREW, left to right, with the page band each occupies. A
 *  row chart with one series draws nothing else: four category names, two axis titles, and the
 *  ticks. Anything new appearing in that markup would show up here as an extra tick and fail loudly
 *  rather than being quietly counted as none — which is exactly what a stacked chart's LEGEND did
 *  when it was first written, hence the second argument. */
function ticksDrawn(svg: string, notTicks: string[] = TITLES): { text: string; ink: [number, number] }[] {
  const family = familyOf(svg);
  return parseTexts(svg)
    .filter((t) => t.text.trim() !== "" && !CATEGORIES.includes(t.text) && !notTicks.includes(t.text))
    .map((t) => ({ text: t.text, ink: inkX(t, family) }))
    .sort((a, b) => a.ink[0] - b.ink[0]);
}

/** Clear ink between the two closest neighbouring tick labels — negative when they overlap, which is
 *  the live defect. `Infinity` when the axis drew one label, which cannot overlap anything. */
function tightestTickGap(svg: string, notTicks?: string[]): number {
  const ticks = ticksDrawn(svg, notTicks);
  if (ticks.length === 0) throw new Error("no value ticks were drawn");
  return Math.min(...ticks.slice(1).map((t, i) => t.ink[0] - ticks[i].ink[1]));
}

/** The rotated `category` title, or a thrown error if the chart did not draw one — a missing title
 *  must never read as a comfortable gap. */
function axisTitle(svg: string): SvgText {
  const name = parseTexts(svg).find((t) => t.text === "category" && t.c !== 0);
  if (!name) throw new Error("the rotated category title was not drawn");
  return name;
}

/** Clear ink between the rotated `category` title and the nearest category label. */
function titleGapPx(svg: string, categories: string[] = CATEGORIES): number {
  const family = familyOf(svg);
  const labels = parseTexts(svg).filter((t) => categories.includes(t.text));
  if (labels.length !== categories.length) throw new Error(`drew ${labels.length} category labels`);
  return Math.min(...labels.map((l) => inkX(l, family)[0])) - inkX(axisTitle(svg), family)[1];
}

describe("a row chart's axes fit what they draw", () => {
  /**
   * The defect itself. At this size the plot is ~45px of the card's 166 — the category names and
   * their title eat the rest — and a `100K` tick measures ~31px, so at most one label can be drawn
   * without touching its neighbour. ECharts' floor for a value axis is the two ends, so two is the
   * honest bound; four is what it drew.
   */
  it("does not draw four value ticks in 57px", () => {
    const drawn = ticksDrawn(render(rowOption(CARD), CARD));
    expect(drawn.length).toBeGreaterThan(0);
    expect(drawn.length).toBeLessThanOrEqual(2);
  });

  /**
   * The same defect stated as the property rather than as a count, across the widths a dashboard
   * card really takes — and the case that caught the first implementation. Asking for fewer SPLITS
   * fixed 166px and left 220, 260 and 280 overlapping, because ECharts rounds a `splitNumber` it
   * dislikes back down to a finer interval. Nothing about the option said so; only the render did.
   *
   * 300px is in the list on purpose: ECharts' own default already clears there by 1.4px, so this
   * case also states that the fit does not have to IMPROVE a card that was never broken — only that
   * it must not break one.
   */
  it.each([166, 180, 200, 220, 240, 260, 280, 300, 320, 400, 640])(
    "draws no two value ticks over each other at %ipx", (width) => {
      expect(tightestTickGap(render(rowOption({ width, height: 240 }), { width, height: 240 })))
        .toBeGreaterThan(0);
    });

  /**
   * The same property on an axis that crosses zero — a bar axis always includes the origin, so two
   * losses in the data make the axis 700K long while its widest label still says `500K`. An interval
   * sized against the biggest NUMBER rather than against the DISTANCE would come out half what is
   * needed here, and draw twice the ticks that fit.
   */
  it.each([200, 220, 240, 250, 260, 280, 320])(
    "draws no two value ticks over each other on an axis that crosses zero at %ipx", (width) => {
      const size = { width, height: 240 };
      expect(tightestTickGap(render(rowOption(size, crossesZero), size))).toBeGreaterThan(0);
    });

  /**
   * A STACK IS AS LONG AS ITS TOTAL. Three metrics of 300K, 290K and 280K make a bar of 870K and an
   * axis that runs to 1M, and an extent read off the individual points would call it 300K — asking
   * for nine labels on a card that fits six. These four widths are the ones where that regressed:
   * measured against the commit before this one they overlap by 16.7, 14.7, 13.9 and 11.9px, and the
   * last two of those cards drew CLEAN ticks before the fit existed at all.
   */
  it.each([260, 280, 300, 320])(
    "draws no two value ticks over each other on a stacked row chart at %ipx", (width) => {
      const size = { width, height: 240 };
      const opt = buildCartesianOption(stackCols, stackRows, stackCfg, size);
      expect(tightestTickGap(render(opt, size), STACK_TEXT)).toBeGreaterThan(0);
    });

  /** The same rule for a stack that grows in both directions — see `divergingStackRows` for why the
   *  two piles cannot be netted into one. */
  it.each([240, 300, 400])(
    "draws no two value ticks over each other on a stack that grows both ways at %ipx", (width) => {
      const size = { width, height: 240 };
      const opt = buildCartesianOption(stackCols, divergingStackRows, divergingStackCfg, size);
      expect(tightestTickGap(render(opt, size), STACK_TEXT)).toBeGreaterThan(0);
    });

  /**
   * The second half of the defect: `nameGap: 28` measures from the axis LINE, and on a row chart the
   * category labels are what sits between the two — so the title was drawn across them. Rendered at
   * 166x184 before the fix, `category` occupied x 64.8-77.8 while `Doohickey` ran 37-97.8: a gap of
   * -40.8px.
   */
  it("keeps the rotated axis title clear of the category labels", () => {
    expect(titleGapPx(render(rowOption(CARD), CARD))).toBeGreaterThan(0);
  });

  /**
   * The mutation the case above cannot catch on its own: any `nameGap` big enough clears the labels,
   * including one that walks the title off the left edge of the card. `grid.left` reserves 26px for
   * it, and the title has to land inside that band rather than outside the SVG.
   */
  it("keeps the title on the canvas while it does so", () => {
    const svg = render(rowOption(CARD), CARD);
    expect(inkX(axisTitle(svg), familyOf(svg))[0]).toBeGreaterThanOrEqual(0);
  });

  /**
   * …AND KEEPS IT THERE WHEN THE LABELS ARE TOO WIDE FOR THE CARD, which the case above cannot show
   * because its own labels always fit. A gap that clears the labels is measured from where the
   * labels ARE, and past a certain width ECharts stops honouring `grid.left` and spills them
   * leftwards — so the gap that clears them walks the title off the canvas with them. Rendered at
   * 166px, `Doohickey Trading` (108.7px) put the title at x -1, `Doohickey Trading Co` at -6, and a
   * 223px name at -100.
   *
   * The title then falls back to the flat 28 and OVERLAPS its labels, which is this PR's own defect
   * — and is deliberately the accepted outcome, because before this PR the title was always drawn
   * that way and a reader could still read it. An invisible title is the worse failure of the two.
   * Both halves are asserted so that "it fell back" cannot be satisfied by drawing nothing.
   */
  it("keeps a title on the canvas when its labels are too wide for the card", () => {
    const svg = render(buildCartesianOption(cols, longNameRows, rowCfg, CARD), CARD);
    const title = axisTitle(svg);
    expect(inkX(title, familyOf(svg))[0]).toBeGreaterThanOrEqual(0);
    expect((buildCartesianOption(cols, longNameRows, rowCfg, CARD) as { yAxis: { nameGap: number } })
      .yAxis.nameGap).toBe(28);
  });

  /**
   * The other side of that threshold, so the fallback is a decision about the CARD and not a rule
   * that quietly retired the fix for any long name: the same labels on a 250px card have room, and
   * there the title clears them as it always should.
   */
  it("still clears the same labels once the card is wide enough", () => {
    const size = { width: 250, height: 184 };
    const svg = render(buildCartesianOption(cols, longNameRows, rowCfg, size), size);
    const categories = longNameRows.map((r) => String(r[0]));
    expect(titleGapPx(svg, categories)).toBeGreaterThan(0);
    expect(inkX(axisTitle(svg), familyOf(svg))[0]).toBeGreaterThanOrEqual(0);
  });

  /**
   * The other direction, and the reason the fit is a CEILING rather than a count: a card with room
   * for ECharts' default ticks must keep them, and must keep them by saying NOTHING. A fix that
   * simply asked for fewer ticks everywhere would pass every case above and quietly strip every wide
   * row chart down to two.
   */
  it("leaves a card that can afford them alone", () => {
    const wide = { width: 640, height: 400 };
    const opt = rowOption(wide) as { xAxis: { interval?: number; splitNumber?: number } };
    expect(opt.xAxis.interval).toBeUndefined();
    expect(opt.xAxis.splitNumber).toBeUndefined();
    expect(ticksDrawn(render(opt, wide)).length).toBeGreaterThanOrEqual(4);
  });

  /**
   * The measured fit is a fallback, not an override: a user who has set a tick count has said what
   * they want and the panel offers that control on a row chart. It has to reach the axis unbeaten,
   * and it has to arrive alone — an `interval` beside it would win over it in ECharts.
   */
  it("gives way to a tick count the user set", () => {
    const opt = buildCartesianOption(
      cols, rows, { ...rowCfg, yTickCount: 4 } as never, CARD,
    ) as { xAxis: { interval?: number; splitNumber?: number } };
    expect(opt.xAxis.splitNumber).toBe(4);
    expect(opt.xAxis.interval).toBeUndefined();
  });

  /**
   * The fitter is EXPORTED, and the builder is not its only future caller — the waterfall and the
   * box plot lay out the same axis from the same grid. A width that is not a number is not a narrow
   * card, and `<= 0` is false for NaN, so the two guards say `> 0` instead: an unmeasured input has
   * to come back as "no answer" and not as an interval of NaN quietly reaching an option.
   */
  it("answers nothing rather than NaN when it is handed one", () => {
    const fmt = (v: number) => String(v);
    expect(intervalThatFits({ plotWidth: NaN, span: 500_000, widestValue: 412_000, format: fmt }))
      .toBeUndefined();
    expect(intervalThatFits({ plotWidth: 200, span: NaN, widestValue: 412_000, format: fmt }))
      .toBeUndefined();
  });

  /**
   * A row chart is a cartesian, so this fit runs in the file every other cartesian kind builds
   * through — and a vertical chart's value axis is a DIFFERENT axis with a different budget. Its
   * ticks stack downwards, where a label's width costs nothing, so a width fit has nothing to say
   * about it and must not reach it.
   *
   * WHAT CHANGED UNDER THIS CASE, and why it is not the guard weakening. This used to assert that
   * NOTHING reached a vertical chart's value axis, which was true while nothing had been written
   * for it. `verticalAxisLabels.echarts.test.ts` is the fix for the same crowding on that axis, and
   * it budgets in HEIGHT: the bar chart's own labels overlapped on the live card too, at a 10.7px
   * pitch, because four 90° category names leave it a 56px plot. So the claim here narrows from
   * "no interval" to "not THIS interval", which is the one this file can actually speak for —
   * an interval sized against the height differs from one sized against the width, and the two
   * being equal by coincidence is what the horizontal card below rules out.
   */
  it("says nothing about a vertical chart's value axis", () => {
    const bar = buildCartesianOption(cols, rows, { ...rowCfg, kind: "bar" }, CARD) as {
      xAxis: { interval?: number; splitNumber?: number };
      yAxis: { interval?: number; splitNumber?: number; nameGap?: number };
    };
    const row = buildCartesianOption(cols, rows, rowCfg, CARD) as { xAxis: { interval?: number } };
    expect(bar.yAxis.splitNumber).toBeUndefined();
    // The row chart's own bottom axis is what this file's fit decides, and the bar chart's left one
    // is not it: on this card they land on different intervals, so a wire crossed between them
    // would show up here rather than passing as a coincidence.
    expect(row.xAxis.interval).toBeDefined();
    expect(bar.yAxis.interval).not.toBe(row.xAxis.interval);
    // …and the row chart's CATEGORY axis is the left one, so the measured `nameGap` this file
    // asserts above belongs to a left axis. A vertical chart's category axis is the bottom one and
    // is measured by the height fit instead — a different function with a different clamp — so what
    // this case can say is only that it is not the flat constant either edge used to keep.
    expect((bar.xAxis as { nameGap?: number }).nameGap).toBeGreaterThan(28);
  });
});
