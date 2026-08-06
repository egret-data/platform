import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

/**
 * A BREAKOUT BAND MAY NOW CHOOSE ITS VALUE AXIS — Metabase's per-series `axis`
 * (`lib/settings/series.ts`), which is keyed per formatted breakout value in
 * `echarts/cartesian/model/series.ts` and read per `seriesModel` in `model/axis.ts`.
 *
 * WHAT WAS WRONG. `decorateSeries` took `const metric = cfg.yMetrics[0]` from the breakout branch and
 * was handed that same `metric` for EVERY band, then asked `rightMetrics(cfg).includes(metric)` — one
 * metric, one answer, all bands. Measured before the change, on `series: "g"`, `yMetrics: ["v","w"]`,
 * `rightAxisMetrics: ["w"]`: series `[["L",null],["R",null]]`, `yAxis` a single object. #423 gated
 * the panel's Left/Right selects off a breakout chart for exactly that reason, recording that the
 * capability gap behind them was untouched. This is that gap.
 *
 * NO NEW STORED KEY, AND THAT IS THE DESIGN RATHER THAN A SAVING. `rightAxisMetrics` was already a
 * list of NAMES filtered against what the chart draws; the only thing that changed is WHICH list it
 * is filtered against — the chart's metrics when there is no breakout, its bands when there is.
 * That is Metabase's model, where `series.axis` hangs off a series key that is a metric on one chart
 * and a formatted breakout value on the other. A second key would have meant two places to store one
 * answer and a rule for which wins.
 *
 * The one hazard it carries is stated where it lives (`rightAxisSeries`): a stored value can only
 * hold metric names, since the panel had no way to write a band name, so it starts inert on every
 * existing breakout chart — unless a band happens to be named exactly like one of the chart's
 * metrics, which is the collision this trades for the extra key.
 */
const cols = [
  { name: "x", type: "varchar" },
  { name: "g", type: "varchar" },
  { name: "v", type: "double" },
  { name: "w", type: "double" },
];
const rows = [
  ["a", "L", 1, 1000], ["a", "R", 2, 2000],
  ["b", "L", 3, 3000], ["b", "R", 4, 4000],
];

type Axis = { id?: string; position?: string; axisLabel?: { formatter?: (v: number) => string } };
type Built = {
  series: { name?: string; yAxisIndex?: number; __trend?: boolean }[];
  yAxis: Axis | Axis[];
};
const build = (extra: Record<string, unknown>) =>
  buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["v", "w"], ...extra } as never) as never as Built;
const sides = (o: Built) => o.series.map((s) => [s.name, s.yAxisIndex ?? null]);
const axisCount = (o: Built) => (Array.isArray(o.yAxis) ? o.yAxis.length : 1);

describe("a breakout band can be sent to the right value axis", () => {
  it("moves the named band and builds the second axis", () => {
    const opt = build({ series: "g", rightAxisMetrics: ["R"] });
    expect(sides(opt)).toEqual([["L", null], ["R", 1]]);
    expect(axisCount(opt)).toBe(2);
    expect((opt.yAxis as Axis[]).map((a) => a.position)).toEqual(["left", "right"]);
    expect((opt.yAxis as Axis[]).map((a) => a.id)).toEqual(["yValue", "yValueRight"]);
  });

  it("refuses a side that holds every band, exactly as the metric path refuses one", () => {
    // "All of them on the right" is not a split — it is the same chart with its axis moved, and
    // ECharts would draw an empty left axis beside it. `isDualAxis`' own rule, kept.
    const opt = build({ series: "g", rightAxisMetrics: ["L", "R"] });
    expect(sides(opt)).toEqual([["L", null], ["R", null]]);
    expect(axisCount(opt)).toBe(1);
  });

  it("sizes each axis over ITS OWN bands, not over every band sharing the metric", () => {
    /**
     * THE TRAP THIS CATCHES, and it is what makes the split mean anything on screen. `axisExtent`,
     * `widestOf` and `fmtFor` all used to select the series feeding an axis by `__metric` — and
     * under a breakout every band carries the SAME `__metric`, so both axes would have been fed all
     * the bands and the second axis would have been a copy of the first.
     *
     * ASSERTED THROUGH THE AXIS FORMATTER because that is what the option actually carries in a
     * jsdom: the min/max a fit writes need a measured card, but the compact-number decision
     * (`widestOf(list) >= 10_000`) is taken per axis at build time either way. Bands of 1 and 2 on
     * the left, 20000 and 40000 on the right: the left axis draws plain thousands and the right
     * draws shorthand. Feed both axes everything and both come out compact.
     */
    const wide = [
      ["a", "small", 1, 0], ["a", "big", 20000, 0],
      ["b", "small", 2, 0], ["b", "big", 40000, 0],
    ];
    const opt = buildCartesianOption(cols, wide, {
      kind: "bar", xAxis: "x", series: "g", yMetrics: ["v"], rightAxisMetrics: ["big"],
    } as never) as never as Built;
    expect(axisCount(opt)).toBe(2);
    expect(opt.series.filter((s) => (s.yAxisIndex ?? 0) === 0).map((s) => s.name)).toEqual(["small"]);
    expect(opt.series.filter((s) => s.yAxisIndex === 1).map((s) => s.name)).toEqual(["big"]);
    const [left, right] = opt.yAxis as Axis[];
    expect(left.axisLabel?.formatter?.(12345)).toBe("12,345");
    expect(right.axisLabel?.formatter?.(12345)).toBe("12.3K");
  });

  it("leaves a stale metric name inert, so an already-saved breakout chart is unchanged", () => {
    // Every stored `rightAxisMetrics` predates this and can only hold METRIC names, which are not
    // band names. #423's measurement, re-run: still `[["L",null],["R",null]]`, still one axis.
    const opt = build({ series: "g", rightAxisMetrics: ["w"] });
    expect(sides(opt)).toEqual([["L", null], ["R", null]]);
    expect(axisCount(opt)).toBe(1);
  });

  /**
   * …AND A STALE NAME BESIDE A LIVE ONE STILL SPLITS, which is where the `names.includes` filter is
   * actually load-bearing and where the assertion above cannot see it. A stale name ALONE is inert
   * either way — nothing matches it, so no series moves — so dropping the filter passes that test.
   * With a live name beside it the two diverge: filtered, `["R"]` is one of two bands and splits;
   * unfiltered, `["R","w"]` counts as two of two and the "a side holding everything is not a split"
   * rule cancels a split the user did ask for. A mutant that removed the filter survived until this
   * case existed.
   */
  it("splits on the live name when a stale one is stored beside it", () => {
    const opt = build({ series: "g", rightAxisMetrics: ["R", "w"] });
    expect(sides(opt)).toEqual([["L", null], ["R", 1]]);
    expect(axisCount(opt)).toBe(2);
  });

  it("carries a band's trend line onto the band's own axis", () => {
    const opt = build({ series: "g", rightAxisMetrics: ["R"], trendLine: true });
    expect(opt.series.find((s) => s.name === "R trend")?.yAxisIndex).toBe(1);
    expect(opt.series.find((s) => s.name === "L trend")?.yAxisIndex ?? 0).toBe(0);
  });

  /**
   * A STACK REFUSES THE SPLIT ENTIRELY, which is the caveat this feature had to be scoped against.
   * `shouldAutoSplitY` has always bailed on `resolveStacked` in its own words — "a stack answers a
   * different question … a second scale would make the stack meaningless" — and until now that rule
   * governed only the AUTOMATIC split, so a hand-typed one slipped past it. Metabase collapses
   * instead of refusing (`model/stack.ts`: a stack goes right only if every member was sent right);
   * egret is narrower, and `rightAxisSeries` says why.
   *
   * ASSERTED ON BOTH PATHS, because the refusal is one line and a fixture on one path alone would
   * not notice it being moved to the other.
   */
  it("refuses the split on a stacked breakout chart", () => {
    const opt = build({ series: "g", rightAxisMetrics: ["R"], stacked: true });
    expect(sides(opt)).toEqual([["L", null], ["R", null]]);
    expect(axisCount(opt)).toBe(1);
  });

  it("refuses it on an area chart, which is stacked with nothing stored", () => {
    // `resolveStacked` is `cfg.stacked ?? cfg.kind === "area"`, so this needs no `stacked: true` —
    // and reading `cfg.stacked` instead of resolving would let the area chart through.
    const opt = build({ kind: "area", series: "g", rightAxisMetrics: ["R"] });
    expect(sides(opt)).toEqual([["L", null], ["R", null]]);
    expect(axisCount(opt)).toBe(1);
    // …and turning the stack off by hand brings the split back, so the refusal is about stacking
    // rather than about the area kind.
    expect(axisCount(build({ kind: "area", series: "g", rightAxisMetrics: ["R"], stacked: false }))).toBe(2);
  });

  it("keeps the row chart and the horizontal bar out of it, as the metric path always has", () => {
    for (const extra of [{ kind: "row" }, { horizontal: true }]) {
      const opt = build({ series: "g", rightAxisMetrics: ["R"], ...extra });
      expect(sides(opt)).toEqual([["L", null], ["R", null]]);
    }
  });
});

describe("the metric path is untouched", () => {
  it("still splits by metric with no breakout", () => {
    const opt = build({ rightAxisMetrics: ["w"] });
    expect(sides(opt)).toEqual([["v", null], ["w", 1]]);
    expect(axisCount(opt)).toBe(2);
  });

  it("still ignores a name that is not a metric", () => {
    expect(axisCount(build({ rightAxisMetrics: ["nope"] }))).toBe(1);
    // The same pair as the breakout case above, for the same reason: a stale name alone is inert
    // whether or not it is filtered out, and only a live name beside it tells the two apart.
    expect(sides(build({ rightAxisMetrics: ["w", "nope"] }))).toEqual([["v", null], ["w", 1]]);
  });

  it("refuses a stacked chart's hand-typed split too", () => {
    expect(axisCount(build({ rightAxisMetrics: ["w"], stacked: true }))).toBe(1);
    expect(axisCount(build({ rightAxisMetrics: ["w"] }))).toBe(2);
  });

  it("still computes a split when none is assigned", () => {
    // `w` is a thousand times `v`, which is what `shouldAutoSplitY` exists to catch.
    expect(axisCount(build({}))).toBe(2);
    expect(axisCount(build({ autoSplitY: false }))).toBe(1);
  });
});
