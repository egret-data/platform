import { describe, it, expect } from "vitest";
import { buildCartesianOption, splitPanelsApplies } from "../options/cartesian";
import {
  MIN_PANEL_HEIGHT, PANEL_TITLE_BAND, PANEL_TITLE_FONT, splitPanelLayout, VALUE_LABEL_BOX_RATIO,
} from "../axisLabelFit";
import { CHART_THEME } from "../palette";

/**
 * SPLIT PANELS — Metabase's `graph.split_panels`, labelled "Stack series" there.
 *
 * One grid per visible series, stacked down the card, sharing ONE category axis drawn once at the
 * bottom and giving each series a value axis of its own. That last part is the whole feature: two
 * metrics whose ranges differ by three orders of magnitude are two flat lines on a shared axis and
 * two readable ones on their own.
 *
 * WHAT THIS FILE CAN AND CANNOT PROVE. jsdom runs no layout and `measure-text.ts` reports a width of
 * zero where there is no canvas, so every claim here is about the option SHAPE and about arithmetic
 * the builder does itself — the grid rects, which axis each series is bound to, which axis draws
 * labels, and the two extents. Whether four panels are legible at 166x184 is a question about pixels
 * and was answered in a browser; the refusal that answer produced is pinned below by its INPUTS
 * (a card too short for `MIN_PANEL_HEIGHT`), which is the part that lives in arithmetic.
 */
const cols = [
  { name: "x", type: "varchar" },
  { name: "g", type: "varchar" },
  { name: "v", type: "double" },
  { name: "w", type: "double" },
];
/** `v` runs 1-4 and `w` runs 100K-400K — Metabase's own decisive case (Count against Sum of Tax),
 *  five orders of magnitude apart, so a shared axis flattens one of them completely. The gap is that
 *  wide on purpose: it also straddles the 10,000 at which `fmtFor` switches to compact labels, which
 *  is what makes "each panel formats for its own magnitude" a testable claim rather than a hope. */
const rows = [
  ["a", "L", 1, 100_000],
  ["b", "L", 2, 200_000],
  ["c", "L", 3, 300_000],
  ["d", "L", 4, 400_000],
];
/** Four bands of one metric, for the breakout case — a panel per band. */
const breakoutRows = [
  ["a", "L", 1, 1], ["a", "M", 2, 2], ["a", "N", 3, 3], ["a", "O", 4, 4],
  ["b", "L", 5, 5], ["b", "M", 6, 6], ["b", "N", 7, 7], ["b", "O", 8, 8],
];

type Grid = { top?: number; height?: number; left?: number; right?: number; containLabel?: boolean };
type Axis = {
  id?: string; gridIndex?: number; name?: string; min?: number; max?: number; position?: string;
  axisLabel?: { show?: boolean; formatter?: (v: number) => string };
  axisTick?: { show?: boolean };
};
type Series = {
  name?: string; xAxisIndex?: number; yAxisIndex?: number; __trend?: boolean; __total?: boolean;
  stack?: string; markLine?: unknown;
};
type Built = {
  grid: Grid | Grid[];
  xAxis: Axis | Axis[];
  yAxis: Axis | Axis[];
  series: Series[];
  legend?: unknown;
  title?: { text?: string; top?: number; left?: number }[];
  graphic?: unknown;
  __axisNamesFor?: (s?: Record<string, boolean>) => Record<string, unknown>;
};

/** A card with room for the panels it is asked for, unless a test says otherwise. */
const ROOMY = { width: 600, height: 400 };
/** The dashboard card the harness really uses — the size this feature had to be checked against. */
const CARD = { width: 166, height: 184 };

/** `"unmeasured"` rather than `undefined`, because passing `undefined` to a parameter with a default
 *  SELECTS the default — the card would have been measured after all, and the one test that cares
 *  would have passed against the wrong build. */
const build = (
  extra: Record<string, unknown>,
  size: { width: number; height: number } | "unmeasured" = ROOMY,
  data: unknown[][] = rows,
) =>
  buildCartesianOption(
    cols,
    data,
    { kind: "line", xAxis: "x", yMetrics: ["v", "w"], splitPanels: true, ...extra } as never,
    (size === "unmeasured" ? undefined : size) as never,
  ) as never as Built;

const grids = (o: Built) => (Array.isArray(o.grid) ? o.grid : [o.grid]);
const xs = (o: Built) => (Array.isArray(o.xAxis) ? o.xAxis : [o.xAxis]);
const ys = (o: Built) => (Array.isArray(o.yAxis) ? o.yAxis : [o.yAxis]);

describe("splitPanelLayout, on its own terms", () => {
  // Asked DIRECTLY as well as through the builder, because the builder gates on its own
  // `panelSeries.length > 1` before ever calling it — so this function's own refusals are
  // unreachable from there, and a mutation run that only drives the builder scores them as
  // untested. It is exported and can have a second caller; these are its terms, not that caller's.
  const inset = { top: 16, bottom: 33 };

  it("refuses fewer than two panels", () => {
    expect(splitPanelLayout({ size: { width: 600, height: 400 }, count: 1, inset })).toBeUndefined();
    expect(splitPanelLayout({ size: { width: 600, height: 400 }, count: 0, inset })).toBeUndefined();
  });

  it("gives every panel the same height and a constant pitch", () => {
    const p = splitPanelLayout({ size: { width: 600, height: 400 }, count: 3, inset })!;
    expect(p).toHaveLength(3);
    expect(new Set(p.map((x) => x.height)).size).toBe(1);
    expect(p[1].top - p[0].top).toBeCloseTo(p[2].top - p[1].top, 6);
  });

  it("keeps the panels inside the insets it was given", () => {
    const p = splitPanelLayout({ size: { width: 600, height: 400 }, count: 3, inset })!;
    expect(p[0].titleTop).toBeGreaterThanOrEqual(inset.top);
    expect(p[2].top + p[2].height).toBeLessThanOrEqual(400 - inset.bottom);
  });

  it("keeps the title band clear of the topmost value label — in arithmetic, not in pixels", () => {
    /**
     * THE ONE CLAIM IN THIS FILE THAT A jsdom CANNOT OBSERVE, stated the only way it can be.
     *
     * The overlap itself is a drawing: measured in a browser at 540x561, a 20px band put the first
     * title's box at y 20-34 against a "40K" label at 28.5-43.5. Nothing in a jsdom moves when that
     * happens, and the mutation run said so — reverting the band to 20 survived every other test
     * here. What DOES live in arithmetic is the inequality that measurement produced, and it is
     * made entirely of constants this module exports.
     *
     * The title's box is `[top - band, top - band + titleBox]` and the label's is
     * `[top - labelBox/2, top + labelBox/2]` — half a label box above the grid, which is the same
     * overhang `plotHeightLessLabels` charges at the top of a single-grid chart. They are disjoint
     * exactly while `band >= titleBox + labelBox/2`.
     *
     * Both boxes come from `VALUE_LABEL_BOX_RATIO`, the module's own measured line-box-to-font
     * ratio, so changing either FONT moves this threshold with it rather than leaving a stale
     * number behind.
     */
    const box = (font: number) => font * VALUE_LABEL_BOX_RATIO;
    expect(PANEL_TITLE_BAND).toBeGreaterThanOrEqual(box(PANEL_TITLE_FONT) + box(CHART_THEME.axisFontSize) / 2);
  });

  it("refuses the moment a panel would come out under MIN_PANEL_HEIGHT", () => {
    // Bracketed rather than asserted at one height: an off-by-one in the comparison would still
    // refuse somewhere, and only a pair either side of the threshold says WHERE.
    const at = (count: number, height: number) =>
      splitPanelLayout({ size: { width: 600, height }, count, inset });
    const tall = at(4, 500)!;
    expect(tall[0].height).toBeGreaterThanOrEqual(MIN_PANEL_HEIGHT);
    let refusedAt = 500;
    while (refusedAt > 0 && at(4, refusedAt) != null) refusedAt -= 1;
    // One pixel taller than the tallest refusal still draws, and its panels clear the floor.
    expect(at(4, refusedAt)).toBeUndefined();
    expect(at(4, refusedAt + 1)![0].height).toBeGreaterThanOrEqual(MIN_PANEL_HEIGHT);
  });
});

describe("split panels: one grid per visible series", () => {
  it("builds a grid, an x-axis and a y-axis per series, each bound by index", () => {
    const o = build({});
    expect(grids(o)).toHaveLength(2);
    expect(xs(o)).toHaveLength(2);
    expect(ys(o)).toHaveLength(2);
    expect(xs(o).map((a) => a.gridIndex)).toEqual([0, 1]);
    expect(ys(o).map((a) => a.gridIndex)).toEqual([0, 1]);
    expect(o.series.map((s) => [s.name, s.xAxisIndex, s.yAxisIndex])).toEqual([
      ["v", 0, 0],
      ["w", 1, 1],
    ]);
  });

  it("counts BANDS, not metrics, when the chart breaks out", () => {
    const o = build({ series: "g", yMetrics: ["v"] }, ROOMY, breakoutRows);
    expect(grids(o)).toHaveLength(4);
    expect(o.series.map((s) => s.yAxisIndex)).toEqual([0, 1, 2, 3]);
  });

  it("stacks the panels down the card without overlapping them", () => {
    const g = grids(build({}));
    const tops = g.map((p) => p.top as number);
    const heights = g.map((p) => p.height as number);
    expect(heights[0]).toBe(heights[1]);
    expect(tops[1]).toBeGreaterThan(tops[0] + heights[0]);
    // The last panel's labels are drawn BELOW its grid (see `containLabel` below), so the bottom of
    // the last band has to stay clear of the card's own edge.
    expect(tops[1] + heights[1]).toBeLessThan(ROOMY.height);
  });

  it("turns containLabel OFF and gives every panel the same left inset", () => {
    // The panels' plots must line up horizontally or the shared category axis is a lie: with
    // `containLabel: true` each grid expands by ITS OWN value labels, so a panel reading "4" and one
    // reading "4000" would put the same category at different x. Metabase computes the inset once
    // for the widest panel and switches containLabel off; so does this.
    const g = grids(build({}));
    expect(g.map((p) => p.containLabel)).toEqual([false, false]);
    expect(g[0].left).toBe(g[1].left);
    expect(g[0].right).toBe(g[1].right);
  });
});

describe("split panels: each panel gets its own value scale", () => {
  it("sizes each y-axis to its OWN series and not to the chart", () => {
    // The decisive test, and the whole reason the feature exists. 600x220 is chosen so the vertical
    // fit engages — a panel tall enough to be drawn (over `MIN_PANEL_HEIGHT`) and short enough to be
    // under `DEFAULT_VALUE_SPLITS` label boxes, which is the window in which the axis states the two
    // ends it divides. On a roomier card ECharts picks them and the option carries no numbers, so
    // this window is where the claim is observable at all from a jsdom.
    const y = ys(build({}, { width: 600, height: 220 }));
    expect([y[0].min, y[0].max]).toEqual([0, 4]);
    expect([y[1].min, y[1].max]).toEqual([0, 400_000]);
  });

  it("formats each panel's labels for its own magnitude", () => {
    // `fmtFor` switches to compact labels once the series it is given reaches 10,000. Asking BOTH
    // formatters the same question is what makes this a per-panel claim: a chart-wide formatter
    // would give one answer twice.
    const y = ys(build({}));
    expect(y[0].axisLabel?.formatter?.(4000)).toBe("4,000");
    expect(y[1].axisLabel?.formatter?.(4000)).toBe("4K");
  });

  it("gives every panel axis a DISTINCT id, and patches none of them on a legend event", () => {
    // `axisNamesFor` addresses the value axis by `id: "yValue"` through a merge `setOption`. N panels
    // means N value axes: leaving them all on that one id gives ECharts duplicate ids to resolve
    // against, and leaving the patch pointed at it when no component wears it makes ECharts CREATE
    // one — the phantom axis `mkValueAxis` records being measured. There is no legend in this mode,
    // so the patch has nothing to say and says nothing.
    const o = build({});
    const ids = ys(o).map((a) => a.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).not.toContain("yValue");
    expect(o.__axisNamesFor?.()).toStrictEqual({});
  });

  it("puts every panel's axis on the left", () => {
    // Metabase forces `series.axis` to "left" under split panels and hides the control. A right-hand
    // axis is a way of showing two scales on one plot; a panel already IS that.
    const y = ys(build({ rightAxisMetrics: ["w"] }));
    expect(y).toHaveLength(2);
    expect(y.map((a) => a.position)).toEqual([undefined, undefined]);
    expect(build({ rightAxisMetrics: ["w"] }).series.map((s) => s.yAxisIndex)).toEqual([0, 1]);
  });
});

describe("split panels: one category axis, drawn once", () => {
  it("suppresses every x-axis's labels but the last", () => {
    const x = xs(build({}));
    expect(x[0].axisLabel?.show).toBe(false);
    expect(x[0].axisTick?.show).toBe(false);
    expect(x[0].name).toBeUndefined();
    // The last one is the chart's own category axis, untouched.
    expect(x[1].axisLabel?.show).not.toBe(false);
  });

  it("keeps the axis LINE on every panel", () => {
    // Metabase draws it (in a stronger colour) on every panel: it is the baseline each series is
    // measured from, and a panel without one floats.
    const x = xs(build({}));
    expect(x.map((a) => (a as { axisLine?: { show?: boolean } }).axisLine?.show)).toEqual([true, true]);
  });
});

describe("split panels: the legend is replaced by a title per panel", () => {
  it("draws no legend and titles each panel with its series", () => {
    // egret's legend is an ECharts component, so clicking it hides a series WITHOUT the option being
    // rebuilt — and "one grid per VISIBLE series" would then have to change the panel COUNT, which a
    // merge patch cannot express. A title per panel says what the legend said, in the panel it says
    // it about, and removes the toggle that could not be honoured.
    const o = build({ showLegend: true });
    expect(o.legend).toBeUndefined();
    expect(o.title?.map((t) => t.text)).toEqual(["v", "w"]);
  });

  it("places each title above its own panel", () => {
    const o = build({});
    const g = grids(o);
    o.title?.forEach((t, i) => {
      expect(t.top as number).toBeLessThan(g[i].top as number);
      expect(t.top as number).toBeGreaterThanOrEqual(i === 0 ? 0 : (g[i - 1].top as number) + (g[i - 1].height as number));
    });
  });

  it("titles a panel with the series' DISPLAY name", () => {
    const o = build({ seriesLabels: { v: "Revenue" } });
    expect(o.title?.map((t) => t.text)).toEqual(["Revenue", "w"]);
  });

  it("zeroes the title's padding, which the band was NOT budgeted for", () => {
    // ECharts' `title.padding` defaults to 5. Measured live at 540x561 before this was set: the
    // first title's box came out at y 20-34 against a "40K" label at 28.5-43.5, overlapping by
    // 5.5px. `PANEL_TITLE_BAND` carries the arithmetic; this is the line that makes it true.
    expect(build({}).title?.map((t) => (t as { padding?: number }).padding)).toEqual([0, 0]);
  });

  it("names the value axis ONCE for the whole stack, not once per panel", () => {
    // Four axes each carrying the measure's name is what the first version drew, and the live
    // 760x560 four-band breakout printed "orders" down the left of the card four times. Metabase
    // draws one rotated label centred over every panel; so does this.
    const o = build({ yLabel: "Revenue", yMetrics: ["v", "w"] });
    expect(ys(o).map((a) => a.name)).toEqual([undefined, undefined]);
    const g = o.graphic as { style?: { text?: string }; top?: number; rotation?: number }[];
    expect(g).toHaveLength(1);
    expect(g[0].style?.text).toBe("Revenue");
    expect(g[0].rotation).toBeCloseTo(Math.PI / 2, 6);
    // Centred over the panels, not over the card: the insets above and below are not symmetric.
    const panels = grids(o);
    const first = panels[0].top as number;
    const last = (panels[1].top as number) + (panels[1].height as number);
    expect(g[0].top).toBeCloseTo((first + last) / 2, 6);
  });

  it("draws no axis name at all when the chart has none to draw", () => {
    // Two metrics on one axis have no shared name — `metricTitle` narrows to a single metric — so
    // there is nothing to centre and no empty `graphic` to walk.
    expect(build({}).graphic).toBeUndefined();
  });
});

describe("split panels: the settings it overrides", () => {
  it("draws no stack, whatever the config says", () => {
    // Metabase hides the Stacking control here and defaults the key to null. A stack is one baseline
    // shared by several series; a panel holds one series, so there is nothing to share.
    const o = build({ kind: "bar", stacked: true });
    expect(o.series.map((s) => s.stack)).toEqual([undefined, undefined]);
    expect(grids(o)).toHaveLength(2);
  });

  it("does not normalise, and draws no stack total", () => {
    // BOTH FOLLOW FROM STACKING rather than being cleared in their own right — `isNormalized` is
    // `!!normalized && resolveStacked` and the stack total is written under `resolveStacked` too.
    // This test is what holds that chain: `buildCartesianOption` explains why it does NOT restate
    // the two keys, and if either predicate is ever loosened to read its flag alone, this fails
    // instead of the panels quietly going flat.
    const o = build({ kind: "bar", stacked: true, normalized: true, showStackTotal: true });
    expect(o.series.filter((s) => s.__total)).toHaveLength(0);
    expect(ys(o).map((a) => a.max)).toEqual([undefined, undefined]);
  });

  it("does not split the y-axis automatically either", () => {
    // The automatic split resolves into `rightAxisMetrics`, which this mode empties — so the two
    // halves of that decision agree rather than one of them quietly surviving.
    const o = build({ autoSplitY: true });
    expect(ys(o).map((a) => a.gridIndex)).toEqual([0, 1]);
    expect(o.series.map((s) => s.yAxisIndex)).toEqual([0, 1]);
  });
});

describe("split panels: the overlays follow their panel", () => {
  it("binds each trend line to the panel of the series it was fitted to", () => {
    const o = build({ trendLine: true });
    const trends = o.series.filter((s) => s.__trend);
    expect(trends.map((s) => [s.name, s.xAxisIndex, s.yAxisIndex])).toEqual([
      ["v trend", 0, 0],
      ["w trend", 1, 1],
    ]);
  });

  it("draws the goal line in every panel, not only the first", () => {
    // A goal at 2 means something different against a 0-4 axis and a 0-4000 one, and a reader given
    // it on one panel only would take the other panel's silence for "no goal here".
    const o = build({ goalValue: 2, goalLabel: "Target" });
    expect(o.series.filter((s) => !s.__trend).map((s) => s.markLine != null)).toEqual([true, true]);
  });
});

describe("split panels: where it declines", () => {
  it("does not split a single series", () => {
    const o = build({ yMetrics: ["v"] });
    expect(Array.isArray(o.grid)).toBe(false);
    expect(Array.isArray(o.yAxis)).toBe(false);
  });

  it("does not split a row chart", () => {
    // Metabase does not spread `SPLIT_PANELS_SETTINGS` into its RowChart. A row chart's categories
    // already run down the card, so splitting it down the card too has nowhere to go.
    const o = build({ kind: "bar", horizontal: true });
    expect(Array.isArray(o.grid)).toBe(false);
    expect(splitPanelsApplies({ kind: "bar", horizontal: true, yMetrics: ["v", "w"] } as never)).toBe(false);
  });

  it("does not split without a measured card", () => {
    // Every panel boundary is a pixel, and this builder's `size` doc says never to decide one
    // against a guessed width. An unmeasured build keeps the chart it always drew.
    const o = build({}, "unmeasured");
    expect(Array.isArray(o.grid)).toBe(false);
  });

  it("does not split a metric-against-metric scatter", () => {
    // That chart leaves through `scatterXY`, which composes its own option and knows nothing about
    // panels — so the panel must not offer what this builder will not draw.
    expect(splitPanelsApplies({ kind: "scatter", xMetric: "v", yMetrics: ["w"] } as never)).toBe(false);
    expect(splitPanelsApplies({ kind: "scatter", xAxis: "x", yMetrics: ["v", "w"] } as never)).toBe(true);
  });

  it("REFUSES a card that cannot give each panel a legible height", () => {
    // The dashboard card is 166x184. Four panels there come out under 10px each — an axis, a title
    // and no room for a line between them. Falling back to the ordinary chart shows the reader
    // something they can read; splitting shows them four slivers.
    const four = build({ series: "g", yMetrics: ["v"] }, CARD, breakoutRows);
    expect(Array.isArray(four.grid)).toBe(false);
  });

  it("…and still splits the same four panels on a card with the room", () => {
    // The refusal must be about the CARD and not about the count, or it is a feature that never runs.
    const four = build({ series: "g", yMetrics: ["v"] }, { width: 600, height: 500 }, breakoutRows);
    expect(grids(four)).toHaveLength(4);
  });

  it("keeps the settings overridden on the chart it falls back TO", () => {
    // The overrides are resolved from the CONFIG, before any card is measured, and this is the case
    // that makes that visible. A chart that fell back would otherwise stack itself and grow a
    // second y-axis the moment its dashboard card was dragged shorter, and lose them again on the
    // way back — a chart whose shape depends on its width, which is worse than either outcome.
    const o = build(
      { kind: "bar", stacked: true, rightAxisMetrics: ["w"], autoSplitY: true },
      { width: 250, height: 184 },
    );
    expect(Array.isArray(o.grid)).toBe(false);
    expect(Array.isArray(o.yAxis)).toBe(false);
    expect(o.series.map((s) => s.stack)).toEqual([undefined, undefined]);
    expect(o.series.map((s) => s.yAxisIndex ?? 0)).toEqual([0, 0]);
    /**
     * …and it DOES get its legend back, which the split chart does not have. An unsplit two-series
     * chart with no legend is a chart whose lines are unnamed.
     *
     * MOVED FROM `CARD` (166x184) TO A CARD 250px WIDE, because a second and entirely independent
     * rule now also decides this key: a legend needs 100px per entry, and two entries do not fit
     * across 166px, so at that width the legend is refused for a reason that has nothing to do with
     * panels (Metabase's `LegendLayout`, ported in `legendFitsAcross`). Asserting `toBeDefined()`
     * there would have been asserting that BOTH rules pass, and would have started failing the day
     * either changed — while the claim this test exists to make is only about the first one.
     *
     * 250x184 still falls back (every assertion above is unchanged and still measured on it), and
     * it has the width two entries need — so the legend's presence here isolates "splitting no
     * longer suppresses it", which is the subject. The width rule gets its own line below.
     */
    expect(o.legend).toBeDefined();
  });

  it("…but a fallback onto a card too narrow for the entries still has no legend", () => {
    // The other half of the pair above, stated so the interaction is recorded rather than
    // discovered. On the 166px dashboard card the fallback happens exactly as before and the legend
    // is still absent — not because the chart split, but because two entries need 200px. Metabase
    // hides its own legend on the same card for the same reason.
    const o = build({ kind: "bar", stacked: true, rightAxisMetrics: ["w"], autoSplitY: true }, CARD);
    expect(Array.isArray(o.grid)).toBe(false);
    expect(o.legend).toBeUndefined();
  });
});
