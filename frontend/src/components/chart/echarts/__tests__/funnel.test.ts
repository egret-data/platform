import { describe, it, expect } from "vitest";
import { funnelSteps } from "../options/funnel";

const cols = [{ name: "stage", type: "varchar" }, { name: "cnt", type: "double" }];

describe("funnelSteps — a sequence, not a ranking", () => {
  // Deliberately NOT descending: the third step is larger than the second.
  const rows = [["Visited", 100], ["Signed up", 40], ["Verified", 60], ["Paid", 10]];

  // Measured against Metabase on four categories whose values do not descend: it keeps them in row
  // order. egret sorted by value, which rearranges a funnel's sequence into a story the data never
  // told — "Verified" appearing before "Signed up" reads as a different process.
  it("keeps the rows in the order they arrived", () => {
    const steps = funnelSteps(cols, rows, { dimension: "stage", metric: "cnt" });
    expect(steps.map((s) => s.name)).toEqual(["Visited", "Signed up", "Verified", "Paid"]);
  });

  it("honours a stored order, keeping unlisted steps after it", () => {
    const steps = funnelSteps(cols, rows, { dimension: "stage", metric: "cnt", stepOrder: ["Paid", "Visited"] });
    expect(steps.map((s) => s.name)).toEqual(["Paid", "Visited", "Signed up", "Verified"]);
  });

  it("drops a hidden step", () => {
    const steps = funnelSteps(cols, rows, { dimension: "stage", metric: "cnt", hiddenSteps: ["Signed up"] });
    expect(steps.map((s) => s.name)).toEqual(["Visited", "Verified", "Paid"]);
  });

  // Only a value that is not a NUMBER goes. A zero step — "nobody got this far" — is the single
  // most important thing a funnel can report, and Metabase keeps it; dropping the row silently
  // shortens the sequence and makes the next step look like it followed the one before it.
  it("keeps zero and negative steps, drops non-finite ones", () => {
    const steps = funnelSteps(cols, [["A", 10], ["B", 0], ["C", -5], ["D", "n/a"]], {
      dimension: "stage", metric: "cnt",
    });
    expect(steps.map((s) => s.name)).toEqual(["A", "B", "C"]);
    expect(steps[1].share).toBe("0.00 %");
  });

  // Metabase prints each step's share of the FIRST one ("124.22 %"); egret printed only the raw
  // value, leaving the reader to divide in their head.
  it("gives every step after the first its share of the first", () => {
    const steps = funnelSteps(cols, rows, { dimension: "stage", metric: "cnt" });
    expect(steps[0].share).toBe("");
    expect(steps[1].share).toBe("40.00 %");
    expect(steps[3].share).toBe("10.00 %");
  });

  // The edge height Metabase draws: `value / max`. On its own chart `3976/5061 = 0.78562` was
  // exactly the polygon edge, which is where this rule comes from.
  it("sizes each edge as a share of the tallest step", () => {
    const steps = funnelSteps(cols, rows, { dimension: "stage", metric: "cnt" });
    expect(steps[0].fraction).toBeCloseTo(1, 10);
    expect(steps[1].fraction).toBeCloseTo(0.4, 10);
    expect(steps[3].fraction).toBeCloseTo(0.1, 10);
  });

  it("formats the value through the metric's own column settings", () => {
    const steps = funnelSteps(cols, [["A", 1234.5]], {
      dimension: "stage", metric: "cnt", columnSettings: { cnt: { prefix: "$", decimals: 0 } },
    });
    expect(steps[0].label).toContain("$");
  });

  it("returns nothing without a dimension or a metric", () => {
    expect(funnelSteps(cols, rows, { dimension: "stage" })).toEqual([]);
    expect(funnelSteps(cols, rows, { metric: "cnt" })).toEqual([]);
  });
});

/**
 * THE STEP NAME IS A VALUE, AND IT HAS A FORMAT — the defect this codebase has now closed on the
 * pivot, the cartesian, the pie and the smart scalar, and had left open here.
 *
 * `styleColumnsFor` falls through to `return resultColumns` for a funnel, so the DIMENSION reaches
 * `FormatSection` and gets a Date style / prefix / decimals select of its own. `funnelSteps` read
 * `columnSettings` for the metric only, so everything typed into that select was stored and drawn
 * nowhere. Measured in the browser on a month-bucketed funnel over `MB Monthly`: the panel read
 * `Date style: January 2024` while all twelve steps still read `2026-01-01`.
 *
 * Metabase formats it — `FunnelNormal.tsx`'s `formatDimension` spreads
 * `settings.column?.(cols[dimensionIndex])` into `formatValue` for every step header — so the rule
 * copied here is "the dimension's own column settings decide the step name", not a rule invented
 * for egret.
 */
describe("funnelSteps — the step name's own format", () => {
  const dated = [{ name: "d", type: "timestamp" }, { name: "cnt", type: "double" }];
  const datedRows = [["2026-01-01", 10], ["2026-02-01", 5]];

  it("draws the step name through the dimension's stored format", () => {
    const steps = funnelSteps(dated, datedRows, {
      dimension: "d", metric: "cnt", columnSettings: { d: { date_style: "MMMM YYYY" } },
    });
    expect(steps.map((s) => s.name)).toEqual(["January 2026", "February 2026"]);
  });

  it("carries a text dimension's prefix and suffix", () => {
    const steps = funnelSteps(cols, [["Visited", 10]], {
      dimension: "stage", metric: "cnt", columnSettings: { stage: { prefix: "→ " } },
    });
    expect(steps[0].name).toBe("→ Visited");
  });

  /**
   * THE HAZARD THE OTHER FOUR FIXES ALL RECORD. `FormatSection`'s `patch` spreads into
   * `{...fmt, ...p}`, so RENAMING the column stores `{column_title: "Month"}` — truthy, and silent
   * about dates. Handing that to `formatValue` on a temporal column takes `formatDate`'s DEFAULT
   * style and turns "2026-01-01" into "January 1, 2026", i.e. a rename that quietly restyles every
   * step. `specifiesDateRendering` is the guard `format.ts` exists to provide; this is the test
   * that keeps it wired up here. A title typed and then deleted leaves the key with no value,
   * which is a truthy object too.
   */
  it("is not restyled by a format that says nothing about dates", () => {
    const titled = funnelSteps(dated, datedRows, {
      dimension: "d", metric: "cnt", columnSettings: { d: { column_title: "Month" } },
    });
    expect(titled.map((s) => s.name)).toEqual(["2026-01-01", "2026-02-01"]);
    const emptied = funnelSteps(dated, datedRows, {
      dimension: "d", metric: "cnt", columnSettings: { d: { column_title: undefined } },
    });
    expect(emptied.map((s) => s.name)).toEqual(["2026-01-01", "2026-02-01"]);
  });

  /**
   * The step name is also the IDENTITY a stored order and a hidden-step list are keyed on
   * (`stepOrder`, `hiddenSteps`) and the value a click drills by (`FunnelChart` hands `step.row`
   * to the drill so it filters on the RAW cell, not on this text). Formatting the name must not
   * move either: a chart whose steps were ordered by hand and then given a date style would
   * otherwise silently lose its order. That is the drill-on-display-strings defect four chart
   * families have now had, and it is why `raw` below is asserted alongside the label.
   */
  it("keeps ordering, hiding and the drill value on the raw cell", () => {
    const steps = funnelSteps(dated, datedRows, {
      dimension: "d", metric: "cnt",
      stepOrder: ["2026-02-01", "2026-01-01"],
      columnSettings: { d: { date_style: "MMMM YYYY" } },
    });
    expect(steps.map((s) => s.name)).toEqual(["February 2026", "January 2026"]);
    expect(steps.map((s) => s.raw)).toEqual(["2026-02-01", "2026-01-01"]);
    const hidden = funnelSteps(dated, datedRows, {
      dimension: "d", metric: "cnt", hiddenSteps: ["2026-01-01"],
      columnSettings: { d: { date_style: "MMMM YYYY" } },
    });
    expect(hidden.map((s) => s.name)).toEqual(["February 2026"]);
  });
});
