import { describe, it, expect, vi } from "vitest";
import { buildCartesianOption, unpinFromZeroApplies } from "../options/cartesian";
import { buildBoxplotOption } from "../options/boxplot";

/**
 * THE MEASURED PATH IS UNREACHABLE WITHOUT THIS, which is the point rather than a setup detail.
 *
 * `measure-text.ts` returns `{width: 0, height: 0}` under jsdom by design ("Size claims belong in a
 * browser"), so `widestLabelWidth` is 0, `categoryGutter` is `undefined`, and `verticalFit` declines
 * — every `size` a test passes is ignored and no `min`/`max` is ever written. A suite without this
 * mock therefore CANNOT see the half of this feature that a browser hits, and would have signed off
 * on `scale: true` alone while the live axis stayed pinned.
 *
 * Same stub, same reason and the same one line as `verticalAxisLabels.echarts.test.ts` and
 * `rowAxisLabels.test.ts`: zrender's own per-glyph estimator, i.e. the width ECharts itself lays out
 * with when there is no canvas.
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

/**
 * Metabase's `graph.y_axis.unpin_from_zero`, ported as a RULE rather than as a number.
 *
 * WHAT METABASE ACTUALLY DOES, read and then measured rather than inferred:
 *
 *   `frontend/src/metabase/visualizations/echarts/cartesian/option/axis.ts:380`
 *       scale: !!settings["graph.y_axis.unpin_from_zero"],
 *
 * That is the whole implementation. It is ECharts' own `scale` flag on a value axis — `false` (the
 * default) forces the axis to include the origin, `true` lets it fit the data. Nothing is computed,
 * so there is no number to copy and no place for one to drift.
 *
 * MEASURED LIVE on the sibling Metabase at :3033, "MB · Line" (12 monthly revenue buckets), because
 * a source read is the weaker claim:
 *   pinned (default)  y ticks: 0 / 5,000 / 10,000 / 15,000 / 20,000 / 25,000
 *   unpinned          y ticks: 9,000 / 12,000 / 15,000 / 18,000 / 21,000 / 24,000 / 27,000
 * Nothing was saved; all six parity cards were re-fetched afterwards with their settings unchanged.
 *
 * WHERE IT IS OFFERED — `isYAxisUnpinFromZeroValid`
 * (`frontend/src/metabase/visualizations/shared/settings/cartesian-chart.ts:245-260`), verbatim:
 *
 *     if (!settings["graph.y_axis.auto_range"] || settings["stackable.stack_type"] != null) {
 *       return false;
 *     }
 *     return seriesDisplays.every(
 *       (display) => display !== "area" && display !== "bar" && display !== "waterfall",
 *     );
 *
 * and its default (`:242`) is `display === "scatter"` — a scatter is unpinned out of the box, every
 * other chart is pinned. All four arms were confirmed in the browser: Line SHOWN, Scatter SHOWN and
 * already checked, Boxplot SHOWN and unchecked, Area HIDDEN, Combo HIDDEN (its default second series
 * is a bar), and turning "Auto y-axis range" off on the Line made the row disappear.
 *
 * THE EGRET-SPECIFIC TRAP THAT `scale: true` ALONE DOES NOT COVER, and the reason half the cases
 * below pass a `size`. Metabase hands ECharts nothing but `scale`; egret's builder does not — it
 * measures the card and, when the ticks would crowd, writes an EXPLICIT `min`/`max` from
 * `verticalValueAxisFit` (and, on a row chart, an `interval` from `intervalThatFits`). Both are
 * derived from `axisExtent`, which seeds `lo = 0, hi = 0`. An explicit `min` outranks `scale`
 * outright in ECharts, so porting the flag alone would have left the axis pinned on exactly the
 * cards a user looks at — a browser measures, jsdom does not — while every unmeasured unit test
 * passed. That is this programme's recurring "saves but does not paint" defect, reached from a new
 * direction, and it is why the extent seed moves with the flag.
 */

const columns = [
  { name: "month", type: "date" },
  { name: "revenue", type: "double" },
  { name: "orders", type: "double" },
];
// Deliberately far from the origin: an axis that fits this data cannot be mistaken for one that
// includes zero, at either end.
const rows = [
  ["2026-01-01", 9200, 40],
  ["2026-02-01", 26800, 55],
  ["2026-03-01", 18400, 47],
];
const opt = (cfg: Record<string, unknown>, size?: { width: number; height: number }) =>
  buildCartesianOption(columns, rows, cfg as never, size) as never as Record<string, never>;

/** A card small enough that the vertical fit engages and writes explicit ends — the state in which
 *  a `scale` flag on its own would have been silently overridden. */
const SMALL = { width: 166, height: 184 };

describe("unpin from zero — the axis option", () => {
  it("pins the value axis to the origin by default, as it always has", () => {
    const y = opt({ kind: "line", xAxis: "month", yMetrics: ["revenue"] }).yAxis as Record<string, unknown>;
    // Not `false`: egret writes nothing, and ECharts' own default for a value axis is `scale: false`.
    // Asserted as an absence so a future line that spells the default out loudly fails here and gets
    // read, rather than passing because `!!undefined === !!false`.
    expect(y.scale).toBeUndefined();
  });

  it("sets ECharts' `scale` when the chart is unpinned", () => {
    const y = opt({ kind: "line", xAxis: "month", yMetrics: ["revenue"], unpinFromZero: true })
      .yAxis as Record<string, unknown>;
    expect(y.scale).toBe(true);
  });

  it("keeps the measured ends AT the origin while pinned", () => {
    const y = opt({ kind: "line", xAxis: "month", yMetrics: ["revenue"] }, SMALL) as unknown as Record<string, unknown>;
    const axis = (y as { yAxis: Record<string, number> }).yAxis;
    // The fit only writes ends when it engages; if it stopped engaging at this size the two cases
    // below would agree for the wrong reason, so the pinned case pins the precondition too.
    expect(axis.min).toBeDefined();
    expect(axis.min).toBe(0);
  });

  it("moves the measured ends OFF the origin when unpinned — the half `scale` cannot reach", () => {
    const axis = (opt({ kind: "line", xAxis: "month", yMetrics: ["revenue"], unpinFromZero: true }, SMALL) as unknown as
      { yAxis: Record<string, number> }).yAxis;
    // An explicit `min` beats `scale` in ECharts, so a min of 0 here is a pinned axis whatever the
    // flag says. The exact rounded value is the fit's business; that it clears the origin and stays
    // under the smallest datum is this assertion's.
    expect(axis.min).toBeGreaterThan(0);
    expect(axis.min).toBeLessThanOrEqual(9200);
  });

  it("still lets an explicit yMin outrank the unpinned extent", () => {
    const axis = (opt({ kind: "line", xAxis: "month", yMetrics: ["revenue"], unpinFromZero: true, yMin: 5000 }, SMALL) as unknown as
      { yAxis: Record<string, number> }).yAxis;
    expect(axis.min).toBe(5000);
  });

  it("carries the flag onto BOTH axes of a dual-axis chart", () => {
    const ys = (opt({
      kind: "combo", xAxis: "month", yMetrics: ["revenue", "orders"],
      seriesTypes: { revenue: "line", orders: "line" },
      rightAxisMetrics: ["orders"], unpinFromZero: true,
    }) as unknown as { yAxis: Record<string, unknown>[] }).yAxis;
    expect(ys).toHaveLength(2);
    expect(ys.map((a) => a.scale)).toEqual([true, true]);
  });
});

describe("unpin from zero — the metric-vs-metric scatter", () => {
  const sc = [
    { name: "price", type: "double" },
    { name: "rating", type: "double" },
  ];
  const scRows = [[15.7, 3.9], [48.9, 4.4], [84.1, 4.1]];
  const scatter = (cfg: Record<string, unknown>) =>
    buildCartesianOption(sc, scRows, cfg as never) as unknown as { yAxis: Record<string, unknown> };

  /** This chart leaves through `scatterXY`, which builds its own pair of value axes and never
   *  reaches `mkValueAxis` — so the flag has to be applied a second time or a scatter, the ONE
   *  display Metabase unpins by default, would be the one chart it never reached. */
  it("unpins by default, matching Metabase's `getYAxisUnpinFromZeroDefault`", () => {
    expect(scatter({ kind: "scatter", xMetric: "price", yMetrics: ["rating"] }).yAxis.scale).toBe(true);
  });

  it("still obeys an explicit pin", () => {
    expect(scatter({ kind: "scatter", xMetric: "price", yMetrics: ["rating"], unpinFromZero: false }).yAxis.scale)
      .toBe(false);
  });
});

describe("unpinFromZeroApplies — Metabase's `isYAxisUnpinFromZeroValid`, arm for arm", () => {
  const base = { xAxis: "month", yMetrics: ["revenue"] };

  it("allows a line and a scatter", () => {
    expect(unpinFromZeroApplies({ ...base, kind: "line" })).toBe(true);
    expect(unpinFromZeroApplies({ ...base, kind: "scatter" })).toBe(true);
  });

  it("refuses a bar, an area and a row — Metabase's three excluded displays", () => {
    // `waterfall`, its third, is not a cartesian kind here; the waterfall's own panel simply never
    // offers the row, which is the same refusal expressed as structure.
    expect(unpinFromZeroApplies({ ...base, kind: "bar" })).toBe(false);
    expect(unpinFromZeroApplies({ ...base, kind: "area" })).toBe(false);
    expect(unpinFromZeroApplies({ ...base, kind: "row" })).toBe(false);
  });

  it("judges a combo BY ITS SERIES, not by its kind", () => {
    // `seriesDisplayFor`'s default is line for the first metric and bar for the rest, so a combo
    // left alone has a bar in it and is refused — which is what the live MB · Combo does.
    expect(unpinFromZeroApplies({ ...base, kind: "combo", yMetrics: ["revenue", "orders"] })).toBe(false);
    expect(unpinFromZeroApplies({
      ...base, kind: "combo", yMetrics: ["revenue", "orders"],
      seriesTypes: { revenue: "line", orders: "line" },
    })).toBe(true);
    expect(unpinFromZeroApplies({
      ...base, kind: "combo", yMetrics: ["revenue", "orders"],
      seriesTypes: { revenue: "line", orders: "area" },
    })).toBe(false);
  });

  it("refuses a stacked or normalised chart", () => {
    expect(unpinFromZeroApplies({ ...base, kind: "line", stacked: true })).toBe(false);
    expect(unpinFromZeroApplies({ ...base, kind: "line", stacked: true, normalized: true })).toBe(false);
  });

  it("refuses once a bound is set — egret's auto-range is DERIVED from the pair", () => {
    // Metabase gates on its stored `graph.y_axis.auto_range`; egret has no such key, and its switch
    // is `yMin == null && yMax == null` (`ValueRangeRows`). Same rule, read off the same state.
    expect(unpinFromZeroApplies({ ...base, kind: "line", yMin: 0 })).toBe(false);
    expect(unpinFromZeroApplies({ ...base, kind: "line", yMax: 100 })).toBe(false);
  });
});

describe("unpin from zero — the box plot", () => {
  const bpCols = [{ name: "category", type: "varchar" }, { name: "price", type: "double" }];
  /**
   * FOUR categories, not the two this fixture had when it was written, and the difference is the
   * one this file's own header warns about: a precondition nobody measured.
   *
   * Two short names on a 166x184 card do not rotate far enough to eat the plot, so
   * `verticalValueAxisFit` finds room for its five ticks and DECLINES — no `min` is written at all,
   * and the pinned arm's `expect(pinned.min).toBe(0)` read `undefined`. That is a fixture that fails
   * to reach the measured path, not a chart that is pinned. Measured across three card sizes: at
   * 166x184 the two-name fixture writes no ends on either arm, and the four-name one writes
   * `{min: 0, max: 60}` pinned against `{min: 40, max: 60}` unpinned.
   *
   * Four is also the shape `options/boxplot.ts`'s own comment measured the defect on — "four product
   * names turned 90° reserve 72px of a 184px card, leaving a 56px plot for six ticks" — so this is
   * the fixture that chart's fit was designed against rather than a size hunted for until it passed.
   */
  const bpRows = [
    ["Doohickey", 40], ["Doohickey", 46], ["Doohickey", 52], ["Doohickey", 58],
    ["Widget", 41], ["Widget", 47], ["Widget", 53], ["Widget", 59],
    ["Gadget", 42], ["Gadget", 48], ["Gadget", 54], ["Gadget", 60],
    ["Gizmo", 43], ["Gizmo", 49], ["Gizmo", 55], ["Gizmo", 57],
  ];
  const bp = (cfg: Record<string, unknown>, size?: { width: number; height: number }) =>
    buildBoxplotOption(bpCols, bpRows, cfg as never, size) as unknown as { yAxis: Record<string, number | boolean> };

  it("is pinned by default", () => {
    expect(bp({ category: "category", value: "price" }).yAxis.scale).toBeUndefined();
  });

  it("sets `scale` and lifts the measured ends off the origin when unpinned", () => {
    expect(bp({ category: "category", value: "price", unpinFromZero: true }).yAxis.scale).toBe(true);
    const pinned = bp({ category: "category", value: "price" }, SMALL).yAxis;
    const free = bp({ category: "category", value: "price", unpinFromZero: true }, SMALL).yAxis;
    // The fit only writes ends when it ENGAGES, and this fixture had to be resized before it did —
    // see the note on `bpRows`. Pinning the precondition on the pinned arm is what stops the two
    // arms agreeing at `undefined` and the pair passing for the wrong reason.
    expect(pinned.min).toBeDefined();
    expect(pinned.min).toBe(0);
    expect(free.min).toBeGreaterThan(0);
    // …and stays under the smallest datum, so "off the origin" cannot be satisfied by an axis that
    // simply started somewhere arbitrary above the data.
    expect(free.min).toBeLessThanOrEqual(40);
  });
});
