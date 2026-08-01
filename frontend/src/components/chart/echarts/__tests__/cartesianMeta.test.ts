import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";
import { cartesianMeta } from "../options/cartesianMeta";

const columns = [
  { name: "category", type: "varchar" },
  { name: "region", type: "varchar" },
  { name: "revenue", type: "double" },
];
const rows: unknown[][] = [
  ["Widget", "North", 10],
  ["Gadget", "South", 20],
  ["Gizmo", "East", 30],
  ["Doohickey", "West", 40],
];
const cfg = {
  kind: "bar" as const,
  xAxis: "category",
  yMetrics: ["revenue"],
  series: "region",
  maxCategories: 2,
  // One series renamed, because `name` and `__raw` are equal on every series until something
  // renames one or formats its column — and a fixture where they agree cannot tell the two apart,
  // which is the single distinction this module exists to preserve.
  seriesLabels: { North: "Northern" },
};
// The same data without the breakout. A breakout chart returns before the fold is reached, so
// `cfg` above can only ever report "no fold" — an equality check on it would hold against a helper
// that never reported one at all. This config is the one that actually folds.
const foldedCfg = { ...cfg, series: undefined };

describe("cartesianMeta", () => {
  /**
   * The metadata used to be read back off the built option (`__foldedCategory`, `series[i].__raw`).
   * That only worked while the option was a value; it becomes a function of the container size in
   * this PR. These assertions pin the extracted helper to the option's own answer, so the two
   * cannot drift apart while both exist.
   */
  it("reports no fold on a breakout chart, which returns before the fold is reached", () => {
    const option = buildCartesianOption(columns, rows, cfg) as { __foldedCategory?: string };
    const meta = cartesianMeta(columns, rows, cfg);
    expect(meta.foldedCategory).toBe(option.__foldedCategory);
  });

  it("reports the fold on a chart that actually folds, not just the absence of one", () => {
    const option = buildCartesianOption(columns, rows, foldedCfg) as { __foldedCategory?: string };
    const meta = cartesianMeta(columns, rows, foldedCfg);
    // Named outright rather than only compared: the drill refuses this exact bucket, and the
    // comparison alone cannot tell "both say Other" from "both say nothing".
    expect(option.__foldedCategory).toBe("Other");
    expect(meta.foldedCategory).toBe(option.__foldedCategory);
  });

  it("reports the same raw series value at each index the option carries", () => {
    const option = buildCartesianOption(columns, rows, cfg) as {
      series?: { name?: string; __raw?: string }[];
    };
    const meta = cartesianMeta(columns, rows, cfg);
    const built = option.series ?? [];
    // A helper that returned undefined for everything would pass an equality-only check, so the
    // series list must be non-trivial before the comparison means anything.
    expect(built.length).toBeGreaterThan(1);
    // And one that returned the DISPLAY name would pass too, on a fixture where the two agree.
    // Reading the display name is the exact bug `__raw` was introduced to fix: the drill filtered
    // by the legend's label and matched nothing the moment a series was renamed.
    expect(built.some((s) => s.name !== s.__raw)).toBe(true);
    built.forEach((s, i) => expect(meta.rawSeriesAt(i)).toBe(s.__raw));
  });

  it("returns undefined past the end of the series list rather than throwing", () => {
    const meta = cartesianMeta(columns, rows, cfg);
    expect(meta.rawSeriesAt(999)).toBeUndefined();
    expect(meta.rawSeriesAt(undefined)).toBeUndefined();
  });
});
