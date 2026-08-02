import { describe, it, expect } from "vitest";
import dayjs from "dayjs";
import { boundarySteps, periodName } from "../temporal-period";
import { TEMPORAL_UNITS } from "../temporal-units";

describe("periodName", () => {
  /**
   * The defect: a chart grouped by month drew "2026-01-01", the bucket's first DAY. The bucket is
   * January, and a reader who sees a date reasonably concludes the chart is daily. Observed live
   * against Metabase, which draws "January 2026", and on the trend card, which drew
   * "December 1, 2026" for a monthly comparison.
   *
   * Every expected string here is Metabase's, cited in `temporal-period.ts` beside the format it
   * comes from. Where Metabase's two implementations disagree these follow the .tsx one, which is
   * what a chart in a browser goes through.
   */
  it("names each truncation unit the way Metabase does", () => {
    expect(periodName("year", "2026-01-01")).toBe("2026");
    expect(periodName("quarter", "2026-07-01")).toBe("Q3 2026");
    expect(periodName("month", "2026-01-01")).toBe("January 2026");
    expect(periodName("day", "2026-01-01")).toBe("January 1, 2026");
  });

  it("keeps the time on sub-day units, where the date alone would be ambiguous", () => {
    expect(periodName("hour", "2026-01-01T13:00:00Z")).toBe("January 1, 2026, 1:00 PM");
    expect(periodName("minute", "2026-01-01T13:05:00Z")).toBe("January 1, 2026, 1:05 PM");
  });

  /**
   * A week is named by its first day, NOT by a span. That is Metabase's axis behaviour, not an
   * oversight: it expands a week into "January 5–11, 2026" only for `type: "tooltip" | "cell"`
   * (`visualizations/lib/formatting/date.tsx:1328-1336`), and the ECharts axis formatter passes no
   * `type` (`echarts/cartesian/model/axis.ts:713-718`). This plan's original guess was the span
   * form, with spaces around the dash that Metabase does not use either.
   */
  it("names a week by its first day, as Metabase's axis does", () => {
    expect(periodName("week", "2026-01-05")).toBe("January 5, 2026");
  });

  /**
   * A bucket value belongs to the query that produced it, not to whoever is looking at the chart.
   * A bare `dayjs()` renders `13:00Z` at the browser's offset, so this suite would pass in London
   * and fail in Saigon — and at a coarser unit an offset moves the label to the wrong period
   * outright.
   */
  it("names the period in the value's own zone, not the viewer's", () => {
    expect(periodName("hour", "2026-01-01T13:00:00+05:30")).toBe("January 1, 2026, 1:00 PM");
    expect(periodName("hour", "2026-01-01T13:00:00-08:00")).toBe("January 1, 2026, 1:00 PM");
    // The one that is not just cosmetic: at +07:00 this instant is the 1st locally in Saigon and
    // the 31st in UTC. The bucket is the 1st.
    expect(periodName("day", "2026-01-01T02:00:00+07:00")).toBe("January 1, 2026");
  });

  /**
   * The two offset forms the zone regex's `:?` is there for, and the one it deliberately does not
   * take. Without these the `:?` is unbought — dropping it leaves every test above passing.
   */
  it("takes ISO 8601's basic offset form, and leaves the one dayjs rejects alone", () => {
    expect(periodName("hour", "2026-01-01T13:00:00+0530")).toBe("January 1, 2026, 1:00 PM");
    expect(periodName("hour", "2026-01-01T13:00:00-0800")).toBe("January 1, 2026, 1:00 PM");
    // Bare `±HH` is legal ISO 8601 that dayjs will not parse, so it falls through to the raw
    // string rather than being read in the viewer's zone — the safe direction, and the reason
    // this is recorded rather than patched.
    expect(periodName("hour", "2026-01-01T13:00:00+05")).toBe("2026-01-01T13:00:00+05");
  });

  /**
   * Total, like `temporalLabel` beside it: an unknown unit, a value that is not a date, and an
   * extraction unit all fall through to the raw string rather than throwing inside a formatter
   * that ECharts calls per label.
   */
  it("falls through rather than throwing", () => {
    expect(periodName(undefined, "2026-01-01")).toBe("2026-01-01");
    expect(periodName("month", "not a date")).toBe("not a date");
    expect(periodName("day_of_week", "3")).toBe("3");
    expect(periodName("month", null)).toBe("");
  });

  /**
   * Pins the premise of the extraction guard rather than the guard, which nothing can reach past
   * while there is no fallback below it. The guard exists because dayjs ACCEPTS an extraction
   * value as a date — so if this ever stops being true, the comment justifying the guard has gone
   * stale and the guard's cost is no longer bought.
   */
  it("dayjs would happily read an extraction value as a date", () => {
    expect(dayjs("3").isValid()).toBe(true);
    expect(dayjs("3").format("MMMM YYYY")).toBe("March 2001");
  });
});

/**
 * WHAT SURVIVES OF THE `widestPeriodName` BLOCK THAT USED TO SIT HERE. That function is gone with
 * the last commit of PR 03 — the tick chooser measures each rung's DRAWN labels now, so nothing was
 * left for a unit-wide worst case to decide — and with it went every assertion about representative
 * dates, two-digit hours and the `draw` mapper, all of which described its internals.
 *
 * This one did not. It guards a FUTURE change rather than that function: nothing ties
 * `PERIOD_FORMAT`'s keys to `TEMPORAL_UNITS`, so a truncation unit added upstream — the backend's
 * `TemporalUnit::parse` and this list move together — would silently reinstate the exact defect this
 * module exists to fix, its buckets falling through to the raw first day on the axis. Driven off
 * `TEMPORAL_UNITS` rather than a copied list, so adding the unit is what turns this red.
 * `boundarySteps` below makes the ladder half of the same claim against the same list.
 */
describe("every truncation unit egret supports", () => {
  it("has a period name, so no bucketing falls through to the raw value", () => {
    const trunc = TEMPORAL_UNITS.filter((u) => u.group === "trunc").map((u) => u.token);
    expect(trunc.length).toBeGreaterThan(0);
    for (const token of trunc) {
      expect(periodName(token, "2026-01-05T13:00:00"), `no period name for "${token}"`)
        .not.toBe("2026-01-05T13:00:00");
    }
  });
});

/**
 * The candidate strides a bucketed category axis picks from — Metabase's
 * `TICKS_TIMESERIES_INTERVALS` at and above the data's own unit, converted from `{unit, count}`
 * into a count of the data's own buckets because ECharts strides a category axis by INDEX.
 */
describe("boundarySteps", () => {
  /**
   * The two the plan names, and the reason the ladder is not just doubling: a reader expects a
   * month axis to break at quarters and half-years, so 3 and 6 have to be rungs — every 4th month
   * is an arbitrary stride even though it is a regular one. 3 is Metabase's `quarter` x 1 rung and
   * 6 its `quarter` x 2; 12 is `year` x 1.
   */
  it("lands a month axis on the boundaries a reader expects", () => {
    expect(boundarySteps("month")).toEqual([1, 2, 3, 6, 12, 24, 60, 120, 600, 1200]);
    expect(boundarySteps("quarter")).toEqual([1, 2, 4, 8, 20, 40, 200, 400]);
    expect(boundarySteps("year")).toEqual([1, 2, 5, 10, 50, 100]);
  });

  /**
   * The sub-month units, whose rungs are where the ladder stops being arithmetic about the data's
   * own unit: a day axis breaks at a week (7) and a fortnight (14) before it reaches a month, and
   * an hour axis at 3, 6 and 12 before a day (24).
   *
   * `week` is the one place the conversion cannot be exact — `diff` truncates, so Metabase's
   * 1-month rung is 4 weeks (31/7 = 4.43) and its quarter is 12 (90/7 = 12.85). Pinned rather than
   * corrected: 13 would be a nicer quarter but it is not what a truncating `diff` gives, and a test
   * that asserts the intention rather than the behaviour is how the two drift apart.
   */
  it("keeps every rung Metabase has between the data's unit and a year", () => {
    expect(boundarySteps("day")).toEqual([1, 7, 14, 31, 59, 90, 181, 365, 730, 1826, 3652, 18262, 36525]);
    expect(boundarySteps("week")).toEqual([1, 2, 4, 8, 12, 25, 52, 104, 260, 521, 2608, 5217]);
    expect(boundarySteps("hour").slice(0, 6)).toEqual([1, 3, 6, 12, 24, 168]);
    expect(boundarySteps("minute").slice(0, 6)).toEqual([1, 5, 15, 30, 60, 180]);
  });

  /**
   * Strictly increasing and starting at 1, for both of `strideThatFits`' loop invariants: it walks
   * UP and takes the first rung that fits, so a non-monotonic ladder would return a stride that a
   * cheaper later rung beat, and a repeated rung would be measured twice for the same answer.
   */
  it("is strictly increasing at every unit, which is what the walk up assumes", () => {
    for (const unit of ["minute", "hour", "day", "week", "month", "quarter", "year"] as const) {
      const steps = boundarySteps(unit);
      expect(steps[0], unit).toBe(1);
      for (let i = 1; i < steps.length; i++) expect(steps[i], `${unit} at ${i}`).toBeGreaterThan(steps[i - 1]);
    }
  });

  /**
   * THE NEGATIVE, and the half that keeps a stride from ever reaching an ordinal axis. An empty
   * ladder is not a fallback — `strideThatFits` has nothing to try and hands the axis back to PR
   * 02's rotate/hide. There is no such thing as every second product.
   *
   * Driven off `TEMPORAL_UNITS` for the positive half, like the period-name coverage test above,
   * so a truncation unit added upstream turns this red rather than silently getting no
   * ladder — that unit's charts would go on drawing no labels at all on a narrow card.
   */
  it("offers no stride for anything that is not a truncation unit", () => {
    expect(boundarySteps(undefined)).toEqual([]);
    expect(boundarySteps("day_of_week")).toEqual([]);
    expect(boundarySteps("month_of_year")).toEqual([]);
    expect(boundarySteps("fortnight")).toEqual([]);
    for (const u of TEMPORAL_UNITS.filter((u) => u.group === "trunc")) {
      expect(boundarySteps(u.token).length, `no stride ladder for "${u.token}"`).toBeGreaterThan(1);
    }
  });
});
