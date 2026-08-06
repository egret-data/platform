import { describe, it, expect } from "vitest";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";

/**
 * THE REST OF THE SAME JOB `cartesian.scatterValueAxis.test.ts` STARTED.
 *
 * `scatterXY` builds its own option from scratch rather than falling through to `baseOption`, and it
 * was written knowing almost none of the panel's chart-wide keys. #418 and #424 took the five
 * value-axis ones (`yMin`, `yMax`, `yTickCount`, `yAxisTicks`, `unpinFromZero`, plus `xAxisTicks`)
 * and left the rest. This file is the remainder, measured the same way: build the same chart twice,
 * once with the key set and once without, and diff the option. Eleven keys came back BYTE-IDENTICAL
 * on a metric-vs-metric scatter while the same keys are live on a scatter without an X metric —
 * the identical chart type, one Data-tab select apart.
 *
 * THE RULE THIS FILE ENFORCES is the one already written into `options/cartesian.ts` beside the
 * y-axis literal: **A DIVERGENCE IS ONLY HONEST WHILE THE PANEL KEEPS ITS HALF OF IT.** Measured by
 * rendering scatter's own controls with `xMetric` set, the Display and Axes tabs offer: `X label`,
 * `Show X label`, `Y label`, `Show Y label`, `Legend`, `Data labels`, `Data label density`,
 * `Shorten large numbers`, `Goal value`, the per-series `Show <name>` switch and the per-series
 * reorder arrows. Every one of them was being stored and thrown away.
 *
 * WHAT WAS ALREADY HONEST AND IS DELIBERATELY NOT TOUCHED, because the sweep that produced this
 * list over-reported three keys and the corrections belong in the record:
 *
 *   - `trendLine` — `TrendOption` returns `null` for `kind === "scatter"`, so the row is not on
 *     offer on EITHER scatter flavour. (Metabase's `ScatterViz` does spread `GRAPH_TREND_SETTINGS`,
 *     so egret is the narrower one here; that gap predates this change and closing it is a builder
 *     feature, not a panel/builder disagreement.)
 *   - `maxCategories` — `viz/cartesian.tsx` declares the control only for `kind !== "scatter"`.
 *   - `xLabelRotation` / `xLabelCompact` — the Category-ticks select renders on every cartesian
 *     kind, but its Rotate-45°, Rotate-90° and Compact items are `disabled` exactly when
 *     `valueX` is true, and `valueX` includes `kind === "scatter" && xMetric != null`. Offered,
 *     visibly greyed, with a caption saying why. That is the panel keeping its half.
 *
 * `goalValue` IS METABASE'S OWN, not an egret invention: `ScatterPlot.tsx` spreads
 * `GRAPH_GOAL_SETTINGS` explicitly. `showLabels`/`labelDensity` are the opposite — Metabase's
 * scatter omits `GRAPH_DISPLAY_VALUES_SETTINGS`, so it has no data-label control at all. They are
 * honoured here anyway, and the reason is internal rather than upstream: egret already ships and
 * honours them on the category-x scatter, so withdrawing the control would leave a stored
 * `showLabels: true` painting there with nothing left to turn it off. Widening the honouring makes
 * the two flavours of one chart type agree; narrowing the control would not have.
 */

const cols = [
  { name: "d", type: "varchar" },
  { name: "spend", type: "double" },
  { name: "rev", type: "double" },
  { name: "g", type: "varchar" },
];

/** THREE bands, and the fixture is deliberately not in alphabetical order: a two-band fixture
 *  cannot see an order- or name-keyed bug, because dropping one and re-appending it yields the
 *  identical list. The reorder and hide cases below both act on the MIDDLE one. */
const rows: unknown[][] = [
  ["a", 1, 10, "west"],
  ["b", 2, 20, "north"],
  ["c", 3, 30, "east"],
  ["d", 4, 40, "west"],
];

const base: CartesianBuildConfig = { kind: "scatter", xAxis: "d", xMetric: "spend", yMetrics: ["rev"] };

interface Axis { name?: string; axisLabel?: { formatter?: (v: number) => string } }
interface Series {
  name: string;
  label?: { show?: boolean; formatter?: (p: { value: unknown }) => string };
  labelLayout?: { hideOverlap?: boolean };
  markLine?: { data?: { yAxis?: number }[]; label?: { formatter?: string } };
}
interface Opt { xAxis: Axis; yAxis: Axis; series: Series[]; legend?: unknown }

const build = (patch: Partial<CartesianBuildConfig> = {}) =>
  buildCartesianOption(cols, rows, { ...base, ...patch }) as unknown as Opt;

describe("scatterXY — axis titles", () => {
  it("names each axis after its metric when nothing overrides it", () => {
    const o = build();
    expect([o.xAxis.name, o.yAxis.name]).toEqual(["spend", "rev"]);
  });

  it("takes an explicit X label", () => {
    expect(build({ xLabel: "Marketing spend" }).xAxis.name).toBe("Marketing spend");
  });

  it("takes an explicit Y label", () => {
    expect(build({ yLabel: "Revenue (USD)" }).yAxis.name).toBe("Revenue (USD)");
  });

  it("drops the X title when Show X label is off", () => {
    expect(build({ xLabel: "Marketing spend", showXLabel: false }).xAxis.name).toBeUndefined();
  });

  it("drops the Y title when Show Y label is off", () => {
    expect(build({ yLabel: "Revenue (USD)", showYLabel: false }).yAxis.name).toBeUndefined();
  });

  /**
   * The same precedence `baseOption`'s `titleOf` applies, asked through the same helper rather than
   * restated: a per-series rename first, then the column's `column_title`, then the raw name. This
   * is also what removes the metric-vs-metric scatter from the Column-title sweep's list — before
   * it, that chart was the one cartesian flavour whose axis ignored a column rename.
   */
  it("falls back to the column rename, then to the series rename", () => {
    expect(build({ columnSettings: { spend: { column_title: "Spend" }, rev: { column_title: "Revenue" } } }))
      .toMatchObject({ xAxis: { name: "Spend" }, yAxis: { name: "Revenue" } });
    expect(build({
      columnSettings: { rev: { column_title: "Revenue" } },
      seriesLabels: { rev: "Top line" },
    }).yAxis.name).toBe("Top line");
  });
});

describe("scatterXY — legend", () => {
  it("shows one automatically once there is more than one band", () => {
    expect(build({ series: "g" }).legend).toBeDefined();
    expect(build().legend).toBeUndefined();
  });

  it("an explicit Off wins over the automatic yes", () => {
    expect(build({ series: "g", showLegend: false }).legend).toBeUndefined();
  });

  it("an explicit On wins over the automatic no", () => {
    expect(build({ showLegend: true }).legend).toBeDefined();
  });
});

describe("scatterXY — data labels", () => {
  it("labels every point when Data labels is on", () => {
    const s = build({ showLabels: true }).series[0];
    expect(s.label?.show).toBe(true);
    // The Y value, formatted through the metric's own column settings — the axis and the label must
    // not disagree about what a point is worth.
    expect(s.label?.formatter?.({ value: [2, 20] })).toBe("20");
  });

  it("draws no labels by default", () => {
    expect(build().series[0].label).toBeUndefined();
  });

  it("thins colliding labels on Fit and keeps every one on All", () => {
    expect(build({ showLabels: true }).series[0].labelLayout).toEqual({ hideOverlap: true });
    expect(build({ showLabels: true, labelDensity: "all" }).series[0].labelLayout).toBeUndefined();
  });
});

describe("scatterXY — shorten large numbers", () => {
  const big: unknown[][] = [["a", 12000, 25468], ["b", 24000, 51000]];

  it("shortens both value axes when asked", () => {
    const o = buildCartesianOption(cols, big, { ...base, compactValues: true }) as unknown as Opt;
    expect(o.yAxis.axisLabel?.formatter?.(25468)).toBe("25.5K");
    expect(o.xAxis.axisLabel?.formatter?.(12000)).toBe("12K");
  });

  it("an explicit Off keeps the full numbers even when they are wide", () => {
    const o = buildCartesianOption(cols, big, { ...base, compactValues: false }) as unknown as Opt;
    expect(o.yAxis.axisLabel?.formatter?.(25468)).toBe("25,468");
  });
});

describe("scatterXY — goal line", () => {
  it("draws a reference line at the goal value", () => {
    const s = build({ goalValue: 25 }).series[0];
    expect(s.markLine?.data).toEqual([{ yAxis: 25 }]);
    expect(s.markLine?.label?.formatter).toBe("Goal");
  });

  it("labels it with the goal label when one is set", () => {
    expect(build({ goalValue: 25, goalLabel: "Target" }).series[0].markLine?.label?.formatter)
      .toBe("Target");
  });

  it("draws none without a goal", () => {
    expect(build().series[0].markLine).toBeUndefined();
  });

  // A goal of 0 is a break-even line and a legitimate thing to draw — `!= null`, not truthiness,
  // the same test `baseOption` and `GoalOptions` both make.
  it("draws one at zero", () => {
    expect(build({ goalValue: 0 }).series[0].markLine?.data).toEqual([{ yAxis: 0 }]);
  });
});

describe("scatterXY — hidden and reordered bands", () => {
  const names = (o: Opt) => o.series.map((s) => s.name);

  it("emits the bands in first-seen order by default", () => {
    expect(names(build({ series: "g" }))).toEqual(["west", "north", "east"]);
  });

  it("drops a hidden band", () => {
    expect(names(build({ series: "g", hiddenSeries: ["north"] }))).toEqual(["west", "east"]);
  });

  it("reorders the bands", () => {
    expect(names(build({ series: "g", seriesOrder: ["east", "north", "west"] })))
      .toEqual(["east", "north", "west"]);
  });

  it("keeps a band the stored order does not mention, after the ones it does", () => {
    expect(names(build({ series: "g", seriesOrder: ["east"] })))
      .toEqual(["east", "west", "north"]);
  });

  /**
   * KEYED ON THE RAW BAND VALUE, not on the label. This is the eighth appearance of that family in
   * this programme and the one the sibling fix in `cartesianColors.test.ts` is about: the panel
   * writes `hiddenSeries` and `seriesOrder` against the names `cartesianSeriesNames` handed it,
   * which are the raw breakout values — so renaming a band must not make its own Show switch stop
   * working. Renaming the MIDDLE band, because losing one name from a two-band list and
   * re-appending it produces the identical result.
   */
  it("hides a RENAMED band by its raw name", () => {
    const o = build({ series: "g", seriesLabels: { north: "Northern" }, hiddenSeries: ["north"] });
    expect(names(o)).toEqual(["west", "east"]);
  });

  it("reorders a RENAMED band by its raw name", () => {
    const o = build({
      series: "g", seriesLabels: { north: "Northern" }, seriesOrder: ["north", "east", "west"],
    });
    expect(names(o)).toEqual(["Northern", "east", "west"]);
  });

  /** Hiding the only band leaves no series rather than an undrawable option — and the goal line,
   *  which hangs off `series[0]`, must not throw looking for one. */
  it("survives every band being hidden", () => {
    const o = build({ series: "g", hiddenSeries: ["west", "north", "east"], goalValue: 5 });
    expect(o.series).toEqual([]);
  });

  /**
   * AN UN-BROKEN-OUT SCATTER IS THE CASE A BREAKOUT FIXTURE CANNOT SEE, and a surviving mutant is
   * what proved it: `__raw: g` in place of `__raw: groupKey(g)` passed every assertion above,
   * because with a breakout `groupKey` is the identity and the two are the same string.
   *
   * Without one there is a single group whose RAW value is `""` — a key `cartesianSeriesNames` never
   * hands the panel and `SeriesListSection` therefore cannot store. The panel's row for that series
   * says `Show rev`, and flicking it writes `hiddenSeries: ["rev"]`, the Y-METRIC. That is `__raw`'s
   * job: to be the key the SETTINGS speak, which for this one series is `groupKey("")`. Keyed on the
   * raw `""` instead, the switch moved and the points stayed on screen — `seriesLabels`' defect
   * exactly, one field over, on the same value.
   */
  it("hides the lone series of an un-broken-out scatter under its y-metric", () => {
    expect(names(build({ hiddenSeries: ["rev"] }))).toEqual([]);
  });

  it("does not hide it under the raw empty group name", () => {
    expect(names(build({ hiddenSeries: [""] }))).toEqual(["rev"]);
  });

  it("hides a RENAMED lone series under its y-metric, not its label", () => {
    expect(names(build({ seriesLabels: { rev: "Revenue" }, hiddenSeries: ["rev"] }))).toEqual([]);
    expect(names(build({ seriesLabels: { rev: "Revenue" }, hiddenSeries: ["Revenue"] })))
      .toEqual(["Revenue"]);
  });
});
