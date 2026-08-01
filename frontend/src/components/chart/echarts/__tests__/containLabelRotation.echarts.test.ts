import { describe, it, expect } from "vitest";
import * as echarts from "echarts/core";
import { registerEChartsModules } from "../register";
import { CHART_THEME } from "../palette";

/**
 * THE PREMISE THE THREE BUILDERS' `grid.bottom` RESTS ON, measured against real ECharts.
 *
 * `CHART_THEME.grid` sets `containLabel: true` (`palette.ts`), and every claim about how much room
 * a rotated x-axis label needs is downstream of one question: does `containLabel` reserve that room
 * itself, or only enough for an upright label? PR 02 assumed the latter and had all three cartesian
 * builders ADD the rotated label's projected footprint to `grid.bottom`. It is the former, so the
 * band was reserved twice: on a one-third-width dashboard card the bars came out about a fifth of
 * the card's height with the bottom 45% of the SVG empty, and the y-axis ticks collided with each
 * other and with the axis title.
 *
 * That defect was invisible to every unit test in the repo — the builder tests assert the OPTION,
 * and the option was fine; what was wrong was what ECharts did with it. So this file asserts the
 * layout, which means it has to run real ECharts. It can: the SVG renderer needs no canvas, and
 * `init(null, ...)` in SSR mode needs no DOM at all, so this is the same layout code a browser
 * runs. Text advance widths differ from a browser's (no canvas `measureText`), so nothing here
 * asserts a pixel count — every assertion is a COMPARISON between two layouts of the same labels,
 * where the measurer cancels out.
 *
 * If ECharts is ever upgraded past a change to `containLabel` — 6.x already routes it through
 * `layOutGridByOuterBounds` rather than the pre-6 implementation — this is the file that fails, and
 * the builders' `grid.bottom` is what has to be reconsidered.
 */

/** The card the live defect was found on: a one-third-width dashboard tile. */
const W = 193;
const H = 214;
/** The four categories from that card. "Doohickey" is the widest and decides the reservation. */
const CATS = ["Doohickey", "Gadget", "Gizmo", "Widget"];

interface Rect { y: number; height: number }

/** Lay a bar chart out for real and report the plot rect ECharts settled on, plus which category
 *  names it actually drew. `bottom` is the grid inset the builder would emit. */
function layout({ rotate, bottom }: { rotate: 0 | 45 | 90; bottom: number }) {
  registerEChartsModules();
  const chart = echarts.init(null, null, { renderer: "svg", ssr: true, width: W, height: H });
  chart.setOption({
    animation: false,
    grid: { ...CHART_THEME.grid, bottom },
    xAxis: {
      type: "category", data: CATS,
      // `interval: 0` is what a decided axis emits — every label drawn or none, never a subset.
      axisLabel: { rotate, interval: 0, fontSize: CHART_THEME.axisFontSize },
    },
    yAxis: { type: "value" },
    series: [{ type: "bar", data: [42000, 51000, 39000, 48000] }],
  });
  const rect = (chart as unknown as {
    getModel(): { getComponent(t: string): { coordinateSystem: { getRect(): Rect } } };
  }).getModel().getComponent("grid").coordinateSystem.getRect();
  const svg = chart.renderToSVGString();
  chart.dispose();
  return {
    plotHeight: rect.height,
    plotBottom: rect.y + rect.height,
    drawn: [...svg.matchAll(/>([^<>]*)</g)].map((m) => m[1]).filter((t) => CATS.includes(t)),
  };
}

/** What `CHART_THEME.grid` actually asks for, so these read as "the builder's inset" rather than
 *  as a literal that could drift away from the thing under test. */
const INSET = CHART_THEME.grid.bottom as number;

describe("containLabel and a rotated x-axis", () => {
  /**
   * The claim itself. If `containLabel` only reserved an upright label's height, these three
   * layouts would have the same plot height and the rotated names would spill off the canvas —
   * which is exactly what would justify a builder adding the footprint by hand.
   */
  it("reserves more room below the plot as the labels turn, from the same grid inset", () => {
    const upright = layout({ rotate: 0, bottom: INSET });
    const tilted = layout({ rotate: 45, bottom: INSET });
    const upended = layout({ rotate: 90, bottom: INSET });

    expect(tilted.plotHeight).toBeLessThan(upright.plotHeight);
    expect(upended.plotHeight).toBeLessThan(tilted.plotHeight);
    // …and the room it took is room for the LABELS, not a shrunken chart with clipped text: all
    // four names are drawn, and the band left below the plot is wider than the inset asked for.
    for (const l of [upright, tilted, upended]) expect(l.drawn).toHaveLength(CATS.length);
    expect(H - upended.plotBottom).toBeGreaterThan(INSET);
  });

  /**
   * …therefore any padding a builder adds on top is not "extra safety", it is plot area deleted.
   * One-for-one, which is the strongest form of the statement: the pixels do not overlap with what
   * `containLabel` reserved and get partly absorbed, they come off the plot in full.
   *
   * The three amounts are the ones the builders used to add: the flat 18/44 explicit ladder, and
   * `bottomPad` at 90° for these four labels on this card (~72px in a browser).
   */
  it.each([
    [45, 18], [90, 44], [90, 72],
  ] as const)("takes every pixel added on top of that off the plot (%i°, +%ipx)", (rotate, pad) => {
    const asIs = layout({ rotate, bottom: INSET });
    const padded = layout({ rotate, bottom: INSET + pad });
    expect(asIs.plotHeight - padded.plotHeight).toBeCloseTo(pad, 6);
    // And it bought nothing: the same four names were already all drawn without it.
    expect(asIs.drawn).toHaveLength(CATS.length);
    expect(padded.drawn).toHaveLength(CATS.length);
  });

  /**
   * What that cost on the card the defect was reported from, stated as the ratio a reader can check
   * against the screenshot: 72px added to a 214px tile left the plot under half what it should be.
   * A single number rather than a comparison, so it is a floor rather than an equality — the exact
   * height depends on the measurer, and only the order of magnitude is being pinned.
   */
  it("more than halves the plot on a one-third-width card at 90°", () => {
    const correct = layout({ rotate: 90, bottom: INSET });
    const doubled = layout({ rotate: 90, bottom: INSET + 72 });
    expect(doubled.plotHeight / correct.plotHeight).toBeLessThan(0.5);
  });
});
