import { describe, it, expect } from "vitest";
import {
  MAX_COMPARISONS, comparableUnit, computeComparison, defaultComparison,
  findPreviousNonEmptyRowIndex, isComparisonValid, previousPeriodName, resolveComparisons,
  type TrendComparison, type TrendSeries,
} from "../trend-comparison";
import { periodName } from "../temporal-period";

/**
 * THE ARITHMETIC BEHIND THE TREND'S COMPARISONS, exercised without a card around it.
 *
 * Every expectation here is cited to Metabase — either to its source, to its own unit spec
 * (`frontend/src/metabase/visualizations/visualizations/SmartScalar/compute.unit.spec.ts` in the
 * sibling checkout), or to a reading DRIVEN on the reference instance at :3033. This is a parity
 * contract, not a taste call, so the citations are the point.
 *
 * `periodLabel` here is `periodName` with no column format, which is what the card falls back to
 * when the user has set none — the arm these captions are read through in practice.
 */
const series = (rows: unknown[][], unit?: string): TrendSeries => ({
  rows, di: 0, mi: 1, unit,
  periodLabel: (v) => periodName(unit, v),
});

/** The index the card would pick for its own number: the last row carrying both. */
const latest = (s: TrendSeries) => findPreviousNonEmptyRowIndex(s.rows, s.di, s.mi, s.rows.length);

const run = (c: TrendComparison, s: TrendSeries) => computeComparison(c, s, latest(s));

describe("trend-comparison — finding the latest row", () => {
  it("skips a trailing row whose metric is null, an empty string, or unparseable", () => {
    // egret's own guard, stronger than upstream's `isEmpty`: `Number(null)` and `Number("")` are
    // both 0 and both finite, so without it a not-yet-loaded period reads as a collapse to zero.
    const rows = [["2029-01-01", 100], ["2029-02-01", null], ["2029-03-01", ""], ["2029-04-01", "x"]];
    expect(findPreviousNonEmptyRowIndex(rows, 0, 1, rows.length)).toBe(0);
  });

  it("skips a row with no period, because a caption cannot name one", () => {
    const rows = [["2029-01-01", 100], [null, 200]];
    expect(findPreviousNonEmptyRowIndex(rows, 0, 1, rows.length)).toBe(0);
  });

  it("ignores the missing period when the chart has no dimension column at all", () => {
    // `di < 0` — the date test is vacuous, not failing. Getting this backwards would make every
    // dimension-less trend report "no rows".
    expect(findPreviousNonEmptyRowIndex([[null, 5]], -1, 1, 1)).toBe(0);
  });

  it("reports -1 when nothing qualifies", () => {
    expect(findPreviousNonEmptyRowIndex([["2029-01-01", null]], 0, 1, 1)).toBe(-1);
  });
});

describe("trend-comparison — previousPeriod", () => {
  /**
   * THE DEFECT THIS UNIT EXISTS FOR, in its smallest form.
   *
   * Metabase's own spec, `compute.unit.spec.ts:487-505`: rows `[["2017-01-01", 100],
   * ["2019-01-01", 300]]` at unit `year` with a `previousPeriod` comparison expect
   * `changeType: "missing"` — no comparison value at all. It does NOT fall back to the 2017 row.
   *
   * Driven at :3033 on the same shape at month granularity (Nov 2028, Dec 2028, Apr 2029):
   * "N/A  vs. previous month: (No data)".
   */
  it("has no value when the previous period is missing from the series", () => {
    const r = run({ id: "1", type: "previousPeriod" }, series([["2017-01-01", 100], ["2019-01-01", 300]], "year"));
    expect(r.value).toBeUndefined();
    expect(r.label).toBe("previous year");
  });

  it("finds the previous period when it is there", () => {
    const r = run({ id: "1", type: "previousPeriod" }, series([["2018-01-01", 100], ["2019-01-01", 300]], "year"));
    expect(r).toStrictEqual({ id: "1", label: "previous year", value: 100 });
  });

  it("names the period at the unit the chart is grouped by", () => {
    for (const [unit, name] of [["month", "previous month"], ["quarter", "previous quarter"],
      ["week", "previous week"], ["day", "previous day"], ["hour", "previous hour"],
      ["minute", "previous minute"], ["year", "previous year"]] as const) {
      expect(previousPeriodName(unit)).toBe(name);
    }
  });

  /**
   * THE SEARCH IS BOUNDED AND STOPS EARLY, which is what makes a hole end it rather than be stepped
   * over. `getRowOfPeriodsAgo` looks at most `dateUnitsAgo` rows back AND bails the moment a
   * candidate is older than the target. Without the early bail, a long run of rows before the hole
   * would each be tested and one of them could coincidentally match at a coarser unit.
   */
  it("does not reach past the missing period however many rows precede it", () => {
    const rows = [["2029-01-01", 1], ["2029-02-01", 2], ["2029-03-01", 3], ["2029-07-01", 7]];
    expect(run({ id: "1", type: "previousPeriod" }, series(rows, "month")).value).toBeUndefined();
  });

  it("has no value when the latest row is the only row", () => {
    // Driven at :3033: a trend over a single month renders "N/A / vs. previous month: (No data)".
    const r = run({ id: "1", type: "previousPeriod" }, series([["2029-04-01", 344]], "month"));
    expect(r.value).toBeUndefined();
  });

  /** A previous-period row that exists but holds no number is not a comparison either — the row
   *  must satisfy BOTH tests, which is the `&& hasMetric(...)` in `rowPeriodsAgo`. */
  it("has no value when the previous period is present but empty", () => {
    const r = run({ id: "1", type: "previousPeriod" }, series([["2029-02-01", null], ["2029-03-01", 344]], "month"));
    expect(r.value).toBeUndefined();
  });
});

describe("trend-comparison — the timezone guard", () => {
  /**
   * `areDatesTheSame`'s SECOND guard, the one the sizing note called the only real semantic risk in
   * this feature. Upstream's own worked example (`compute.ts:640-649`), reproduced exactly:
   *
   *   target    2023-12-01T00:00-04:00
   *   candidate 2023-11-01T00:00-05:00  ==  2023-11-01T01:00-04:00 in the target's frame
   *
   * The candidate is 0 < x < 1 months behind, `diff(…, "month")` truncates that to 0, and the first
   * guard alone reads it as the SAME month. That is a real shape: a query spanning a DST boundary
   * returns buckets on either side with different offsets, and both `parseInOwnZone` and upstream's
   * `parseZone` preserve them rather than normalising them away.
   *
   * Reached here through the offsets themselves, not by mocking: the November row is one month
   * before the December one by the calendar, so the guard is what decides whether it MATCHES the
   * target of "one month before December" — and it does, correctly, because the target is computed
   * by subtracting from the December value and therefore carries December's offset.
   */
  it("still matches the previous month across an offset change, when it really is the previous month", () => {
    const rows = [["2023-11-01T00:00:00-05:00", 100], ["2023-12-01T00:00:00-04:00", 300]];
    const r = run({ id: "1", type: "previousPeriod" }, series(rows, "month"));
    expect(r.value).toBe(100);
  });

  /**
   * The other side of the same guard, and the one the guard exists for: a candidate that is a
   * DIFFERENT month but lands inside the truncation window must be rejected. October is two months
   * behind a December target, so the search stops at it — the value must not be 50.
   */
  it("does not accept a different month that a truncated diff would let through", () => {
    const rows = [["2023-10-01T00:00:00-05:00", 50], ["2023-12-01T00:00:00-04:00", 300]];
    expect(run({ id: "1", type: "previousPeriod" }, series(rows, "month")).value).toBeUndefined();
  });

  /**
   * THE COARSE GUARD IS NOT REDUNDANT EITHER, and this test exists because mutation said the
   * assertions above could not see it: deleting `diff(…) !== 0` and leaving only the unit-number
   * check survived the whole suite.
   *
   * The unit number ALONE is not a date. `.month()` is 0–11, so February 2028 and February 2029
   * both report 1 — and with the coarse guard gone, a February two years back is accepted as "the
   * previous month" of a March. That is a bigger lie than the one this unit set out to fix, since
   * the row it reaches is a year off rather than a few months.
   */
  it("does not accept the same month of a different year as the previous month", () => {
    const rows = [["2028-02-01", 999], ["2029-03-01", 344]];
    expect(run({ id: "1", type: "previousPeriod" }, series(rows, "month")).value).toBeUndefined();
  });

  /** The same hole read at year granularity, where `.year()` cannot collide and only the coarse
   *  guard can reject — the two guards covering for each other in opposite directions. */
  it("does not accept a year that is two years back as the previous year", () => {
    const rows = [["2027-01-01", 999], ["2029-01-01", 344]];
    expect(run({ id: "1", type: "previousPeriod" }, series(rows, "year")).value).toBeUndefined();
  });

  /**
   * A UTC value and a value with an explicit offset must not be read in the VIEWER's zone. This is
   * `parseInOwnZone`'s contract, asked of the arithmetic rather than of the formatter: parsing
   * "2029-03-01T00:00:00Z" locally in a zone west of UTC moves it into February, at which point the
   * previous month is January and the whole comparison points one period too far back.
   */
  it("reads a Z-suffixed bucket in its own zone rather than the viewer's", () => {
    const rows = [["2029-02-01T00:00:00Z", 100], ["2029-03-01T00:00:00Z", 300]];
    expect(run({ id: "1", type: "previousPeriod" }, series(rows, "month")).value).toBe(100);
  });
});

describe("trend-comparison — periodsAgo", () => {
  const rows = [["2029-01-01", 400], ["2029-02-01", 550], ["2029-03-01", 344]];

  it("counts back the requested number of periods, not one", () => {
    const r = run({ id: "1", type: "periodsAgo", value: 2 }, series(rows, "month"));
    expect(r.value).toBe(400);
  });

  /** DATED rather than named past one period: "3 months ago" and the month's own name are the same
   *  length and only one of them says which month. Upstream splits on the same `=== 1` test. */
  it("names the period by date once it is more than one back", () => {
    expect(run({ id: "1", type: "periodsAgo", value: 2 }, series(rows, "month")).label).toBe("January 2029");
    expect(run({ id: "1", type: "periodsAgo", value: 1 }, series(rows, "month")).label).toBe("previous month");
  });

  /** With no row there, the caption still names the period that was SEARCHED FOR — computed from
   *  the latest date — so the reader learns which period had nothing in it. */
  it("names the period it looked for even when nothing was recorded in it", () => {
    const r = run({ id: "1", type: "periodsAgo", value: 3 }, series([["2029-01-01", 400], ["2029-03-01", 344]], "month"));
    expect(r.value).toBeUndefined();
    expect(r.label).toBe("December 2028");
  });

  it("cannot see past its own window", () => {
    // Two rows, asked for one period ago: the January row is three months back, not one.
    const r = run({ id: "1", type: "periodsAgo", value: 1 }, series([["2029-01-01", 400], ["2029-04-01", 344]], "month"));
    expect(r.value).toBeUndefined();
  });
});

describe("trend-comparison — previousValue", () => {
  /**
   * The row walk — and the arithmetic egret already had, now under the caption it belongs to.
   * Metabase's spec `compute.unit.spec.ts:250-271`: rows Sep/Nov with October absent give
   * `comparisonValue: 300` and `dateStr: "Sep"`. It reaches the row AND it names that row.
   */
  it("reaches the previous non-empty row across a hole, and names the period it reached", () => {
    const r = run({ id: "1", type: "previousValue" }, series([["2029-09-01", 300], ["2029-11-01", 100]], "month"));
    expect(r).toStrictEqual({ id: "1", label: "September 2029", value: 300 });
  });

  it("skips a row whose metric is null on the way back", () => {
    // `compute.unit.spec.ts:272-291`, "percent decrease over null row data".
    const rows = [["2029-09-01", 300], ["2029-10-01", null], ["2029-11-01", 100]];
    expect(run({ id: "1", type: "previousValue" }, series(rows, "month")).value).toBe(300);
  });

  it("falls back to a bare 'previous' only when there is nothing to name", () => {
    const r = run({ id: "1", type: "previousValue" }, series([["2029-11-01", 100]], "month"));
    expect(r).toStrictEqual({ id: "1", label: "previous" });
    // `toStrictEqual` rather than `toEqual` BECAUSE THE ABSENCE OF `value` IS THE POINT: `toEqual`
    // skips undefined properties, so it cannot tell "no value" from "value: undefined" from a
    // result object that grew a stray key.
  });

  it("works with no unit at all, which is when it is the default", () => {
    const s: TrendSeries = { rows: [["Jan", 100], ["Feb", 120]], di: 0, mi: 1, periodLabel: (v) => String(v) };
    expect(run({ id: "1", type: "previousValue" }, s)).toStrictEqual({ id: "1", label: "Jan", value: 100 });
  });
});

describe("trend-comparison — staticNumber", () => {
  it("compares against the number, captioned with the label", () => {
    const r = run({ id: "1", type: "staticNumber", value: 500, label: "Goal" }, series([["2029-03-01", 344]], "month"));
    expect(r).toStrictEqual({ id: "1", label: "Goal", value: 500 });
  });

  it("reads nothing off the rows, so it works on an empty series", () => {
    const r = computeComparison({ id: "1", type: "staticNumber", value: 500, label: "Goal" }, series([], "month"), -1);
    expect(r.value).toBe(500);
  });
});

describe("trend-comparison — validity and defaults", () => {
  /** Metabase's `getDefaultComparison`: a series WITH a date unit defaults to `previousPeriod`, one
   *  without defaults to `previousValue`. This is the pair that makes egret's shipped caption
   *  ("vs previous month") describe the arithmetic underneath it. */
  it("defaults to a period comparison when bucketed and a row walk when not", () => {
    expect(defaultComparison("month").type).toBe("previousPeriod");
    expect(defaultComparison(undefined).type).toBe("previousValue");
    expect(defaultComparison("day_of_week").type).toBe("previousValue");
  });

  it("refuses a period comparison on a chart with no period", () => {
    expect(isComparisonValid({ id: "1", type: "previousPeriod" }, undefined)).toBe(false);
    expect(isComparisonValid({ id: "1", type: "periodsAgo", value: 2 }, undefined)).toBe(false);
    expect(isComparisonValid({ id: "1", type: "previousPeriod" }, "month")).toBe(true);
  });

  it("refuses an extraction unit, which names no period", () => {
    // `day_of_week` is a unit, and it is not a period — `namesAPeriod` is what separates them, and
    // gating on it here is why a comparison can never be captioned with a raw bucket value.
    expect(comparableUnit("day_of_week")).toBeUndefined();
    expect(isComparisonValid({ id: "1", type: "previousPeriod" }, "day_of_week")).toBe(false);
  });

  it("refuses a half-typed custom value", () => {
    expect(isComparisonValid({ id: "1", type: "staticNumber", value: 500 }, "month")).toBe(false);
    expect(isComparisonValid({ id: "1", type: "staticNumber", label: "Goal" }, "month")).toBe(false);
    expect(isComparisonValid({ id: "1", type: "staticNumber", value: 500, label: "  " }, "month")).toBe(false);
    expect(isComparisonValid({ id: "1", type: "staticNumber", value: 500, label: "Goal" }, "month")).toBe(true);
  });

  it("refuses a periods-ago count that is not a whole number of periods", () => {
    for (const v of [0, -1, 1.5, undefined, Number.NaN]) {
      expect(isComparisonValid({ id: "1", type: "periodsAgo", value: v }, "month")).toBe(false);
    }
    expect(isComparisonValid({ id: "1", type: "periodsAgo", value: 1 }, "month")).toBe(true);
  });

  it("falls back to the default when every stored comparison is unusable", () => {
    const stored: TrendComparison[] = [{ id: "1", type: "staticNumber", value: 500 }];
    expect(resolveComparisons(stored, "month")).toStrictEqual([{ id: "default", type: "previousPeriod" }]);
    expect(resolveComparisons([], "month")).toStrictEqual([{ id: "default", type: "previousPeriod" }]);
    expect(resolveComparisons(undefined, "month")).toStrictEqual([{ id: "default", type: "previousPeriod" }]);
  });

  it("keeps the usable ones and drops only the rest", () => {
    const stored: TrendComparison[] = [
      { id: "1", type: "staticNumber", value: 500 },        // no label
      { id: "2", type: "previousValue" },
    ];
    expect(resolveComparisons(stored, "month").map((c) => c.id)).toStrictEqual(["2"]);
  });

  it("caps the list at Metabase's maximum", () => {
    const many = Array.from({ length: 5 }, (_, i) => ({ id: `${i}`, type: "previousValue" as const }));
    expect(resolveComparisons(many, "month")).toHaveLength(MAX_COMPARISONS);
    expect(MAX_COMPARISONS).toBe(3);
  });
});
