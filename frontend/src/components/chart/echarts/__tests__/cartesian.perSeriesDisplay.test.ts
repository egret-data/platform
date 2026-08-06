import { describe, it, expect } from "vitest";
import { buildCartesianOption, seriesDisplayFor, perSeriesDisplayApplies } from "../options/cartesian";

/**
 * PER-SERIES DISPLAY TYPE ON A LINE, AREA OR BAR CHART — Metabase's `series.display`, whose
 * `getHidden` names the four kinds that offer it verbatim (`lib/settings/series.ts`):
 *
 *     getHidden: (single, settings) =>
 *       !["line", "area", "bar", "combo"].includes(single.card.display ?? "") ||
 *       settings["stackable.stack_type"] != null,
 *
 * so line, AREA, bar and combo — not the row chart and not the scatter, whose displays are what
 * those chart types are.
 *
 * THIS WAS NOT A PANEL GATE, WHICH A PROBE SETTLED BEFORE ANY CODE MOVED. `seriesDisplayFor`
 * already resolved per series, but it consulted `seriesTypes` on `combo` alone and returned the
 * CHART KIND for everything else — measured, on a line chart carrying `seriesTypes: {w: "bar"}`:
 *
 *     seriesDisplayFor("line", "v", 0, {v: "bar"}) -> "line"
 *     built series -> [["v","line"], ["w","line"]]
 *
 * A stored per-series display was therefore discarded by the builder, not merely unreachable from
 * the panel, so opening the panel gate alone would have produced exactly this programme's recurring
 * defect: a control that saves and does not paint.
 */
const cols = [
  { name: "x", type: "varchar" },
  { name: "v", type: "double" },
  { name: "w", type: "double" },
];
const rows = [["a", 1, 10], ["b", 2, 20], ["c", 3, 30]];

type Built = { series: { name?: string; type?: string; areaStyle?: unknown; barWidth?: unknown; stack?: string }[] };
const build = (extra: Record<string, unknown>) =>
  buildCartesianOption(cols, rows, { xAxis: "x", yMetrics: ["v", "w"], ...extra } as never) as never as Built;
const shapes = (o: Built) =>
  o.series.map((s) => [s.name, s.type, s.areaStyle != null ? "filled" : "bare"]);

describe("which kinds let a series choose its own display", () => {
  it("matches Metabase's four, and only those four", () => {
    expect(perSeriesDisplayApplies("line")).toBe(true);
    expect(perSeriesDisplayApplies("area")).toBe(true);
    expect(perSeriesDisplayApplies("bar")).toBe(true);
    expect(perSeriesDisplayApplies("combo")).toBe(true);
    // A row chart IS a horizontal bar and a scatter IS points; neither has an alternative to offer,
    // and Metabase leaves both out of the list above.
    expect(perSeriesDisplayApplies("row")).toBe(false);
    expect(perSeriesDisplayApplies("scatter")).toBe(false);
  });

  it("resolves the override on line/area/bar and refuses it on row/scatter", () => {
    expect(seriesDisplayFor("line", "v", 0, { v: "bar" })).toBe("bar");
    expect(seriesDisplayFor("area", "v", 0, { v: "bar" })).toBe("bar");
    expect(seriesDisplayFor("bar", "v", 0, { v: "line" })).toBe("line");
    expect(seriesDisplayFor("row", "v", 0, { v: "line" })).toBe("bar");
    expect(seriesDisplayFor("scatter", "v", 0, { v: "bar" })).toBe("scatter");
  });

  it("leaves every default exactly where it was", () => {
    // The one positional default in the family is the combo's, and it is Metabase's
    // `getSeriesDefaultDisplay` verbatim. Widening the lookup must not give the other three one.
    expect(seriesDisplayFor("combo", "v", 0, undefined)).toBe("line");
    expect(seriesDisplayFor("combo", "w", 1, undefined)).toBe("bar");
    expect(seriesDisplayFor("line", "w", 1, undefined)).toBe("line");
    expect(seriesDisplayFor("area", "w", 1, undefined)).toBe("area");
    expect(seriesDisplayFor("bar", "w", 1, undefined)).toBe("bar");
  });
});

describe("the builder paints what the series chose", () => {
  it("draws one band of a line chart as a bar", () => {
    expect(shapes(build({ kind: "line", seriesTypes: { w: "bar" } })))
      .toEqual([["v", "line", "bare"], ["w", "bar", "bare"]]);
  });

  it("draws one band of a bar chart as a filled area", () => {
    expect(shapes(build({ kind: "bar", seriesTypes: { v: "area" } })))
      .toEqual([["v", "line", "filled"], ["w", "bar", "bare"]]);
  });

  it("leaves an untouched chart byte-identical", () => {
    // The widened lookup must be invisible to every chart that has never stored a `seriesTypes`.
    for (const kind of ["line", "area", "bar", "row", "scatter", "combo"]) {
      expect(JSON.stringify(build({ kind })))
        .toBe(JSON.stringify(build({ kind, seriesTypes: {} })));
    }
  });

  it("groups bars that a per-series display created, instead of stacking them at one x", () => {
    // `barMetrics`/`grouped` already asked `seriesDisplayFor`, so this follows from the same change
    // — and it is the assertion that catches it NOT following: two pinned-width bars at one
    // category draw on top of one another with only the last visible (measured on a combo, #423).
    const two = build({ kind: "line", seriesTypes: { v: "bar", w: "bar" } });
    expect(two.series.every((s) => s.type === "bar")).toBe(true);
    expect(two.series.every((s) => s.barWidth === undefined)).toBe(true);
    // …and a LONE bar among lines keeps its pinned width, because there is nothing to sit beside.
    expect(build({ kind: "line", seriesTypes: { w: "bar" } }).series.find((s) => s.name === "w")?.barWidth)
      .not.toBe(undefined);
  });

  it("stacks a line chart's two bar bands, which is now a reachable state", () => {
    const opt = build({ kind: "line", stacked: true, seriesTypes: { v: "bar", w: "bar" } });
    expect(opt.series.map((s) => s.stack)).toEqual(["total", "total"]);
  });
});
