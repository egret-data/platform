import { describe, it, expect, vi } from "vitest";
import * as echarts from "echarts/core";
import { platformApi } from "zrender/lib/core/platform.js";
import { registerEChartsModules } from "../register";
import { buildCartesianOption } from "../options/cartesian";

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
/** The same four categories with two of them in the red, so the axis runs -200K to 500K rather than
 *  0 to 500K. Twice the axis for the same widest label, and the case that separates "how far does
 *  this axis run" from "what is the biggest number on it". */
const crossesZero: unknown[][] = [
  ["Widget", 412_000], ["Doohickey", -188_000], ["Gizmo", 341_000], ["Gadget", -96_000],
];
const CATEGORIES = ["Widget", "Doohickey", "Gizmo", "Gadget"];
const rowCfg = { kind: "row" as const, xAxis: "category", yMetrics: ["revenue"] };
/** The two axis names the builder writes: the category axis is titled from the x column and the
 *  value axis from the metric. Both are drawn, so both have to be excluded from the tick count. */
const TITLES = ["category", "revenue"];
const CARD = { width: 166, height: 184 };

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
 *  rather than being quietly counted as none. */
function ticksDrawn(svg: string): { text: string; ink: [number, number] }[] {
  const family = familyOf(svg);
  return parseTexts(svg)
    .filter((t) => t.text.trim() !== "" && !CATEGORIES.includes(t.text) && !TITLES.includes(t.text))
    .map((t) => ({ text: t.text, ink: inkX(t, family) }))
    .sort((a, b) => a.ink[0] - b.ink[0]);
}

/** Clear ink between the two closest neighbouring tick labels — negative when they overlap, which is
 *  the live defect. `Infinity` when the axis drew one label, which cannot overlap anything. */
function tightestTickGap(svg: string): number {
  const ticks = ticksDrawn(svg);
  if (ticks.length === 0) throw new Error("no value ticks were drawn");
  return Math.min(...ticks.slice(1).map((t, i) => t.ink[0] - ticks[i].ink[1]));
}

/** Clear ink between the rotated `category` title and the nearest category label. */
function titleGapPx(svg: string): number {
  const family = familyOf(svg);
  const texts = parseTexts(svg);
  const name = texts.find((t) => t.text === "category" && t.c !== 0);
  const labels = texts.filter((t) => CATEGORIES.includes(t.text));
  // Not vacuous: a title or labels that failed to parse would otherwise read as a comfortable gap.
  if (!name) throw new Error("the rotated category title was not drawn");
  if (labels.length !== CATEGORIES.length) throw new Error(`drew ${labels.length} category labels`);
  return Math.min(...labels.map((l) => inkX(l, family)[0])) - inkX(name, family)[1];
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
  it.each([200, 240, 280, 320])(
    "draws no two value ticks over each other on an axis that crosses zero at %ipx", (width) => {
      const size = { width, height: 240 };
      expect(tightestTickGap(render(rowOption(size, crossesZero), size))).toBeGreaterThan(0);
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
    const name = parseTexts(svg).find((t) => t.text === "category" && t.c !== 0);
    expect(name).toBeDefined();
    expect(inkX(name!, familyOf(svg))[0]).toBeGreaterThanOrEqual(0);
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
   * A row chart is a cartesian, so the fit runs in the file every other cartesian kind builds
   * through. A vertical bar's value axis is the LEFT one — its ticks stack downwards, where their
   * width costs nothing and this fit has nothing to say — and its category labels are the ones the
   * PR 02 ladder already owns. Neither axis may acquire an interval from this change.
   */
  it("says nothing about a vertical chart's value axis", () => {
    const bar = buildCartesianOption(cols, rows, { ...rowCfg, kind: "bar" }, CARD) as {
      xAxis: { interval?: number; splitNumber?: number };
      yAxis: { interval?: number; splitNumber?: number; nameGap?: number };
    };
    expect(bar.yAxis.interval).toBeUndefined();
    expect(bar.yAxis.splitNumber).toBeUndefined();
    // The category axis is the bottom one here, where a title needs no room made for labels beside
    // it — so it keeps the constant it always had rather than the row chart's measured gap.
    expect((bar.xAxis as { nameGap?: number }).nameGap).toBe(28);
  });
});
