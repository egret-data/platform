import { describe, it, expect } from "vitest";
import dayjs from "dayjs";
import { periodName, widestPeriodName } from "../temporal-period";
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

describe("widestPeriodName", () => {
  /**
   * The tick chooser needs the worst case for a unit, not the worst case in this dataset — same
   * reason Metabase measures `getRepresentativeDates` rather than its data. A month axis whose
   * visible range happens to be May–July would otherwise size itself on "May 2026" and clip
   * "September 2026" when the range moves.
   */
  it("returns the longest name a unit can produce, not one from the data", () => {
    expect(widestPeriodName("month")).toBe("September 2026");
    expect(widestPeriodName("quarter").length).toBe("Q1 2026".length);
    expect(widestPeriodName("year").length).toBe("2026".length);
  });

  /** Both a one- and a two-digit day: feeding only `-05` leaves `day` and `week` three characters
   *  of slack, and the assertion would keep passing if the representative days were wrong. */
  it("is at least as wide as any name that unit actually produces", () => {
    for (const unit of ["year", "quarter", "month", "day", "week"] as const) {
      const widest = widestPeriodName(unit).length;
      for (let m = 0; m < 12; m++) {
        for (const d of [5, 28]) {
          const v = `2026-${String(m + 1).padStart(2, "0")}-${String(d).padStart(2, "0")}`;
          expect(periodName(unit, v).length).toBeLessThanOrEqual(widest);
        }
      }
    }
  });

  /**
   * The sub-day units separately, because their worst case is not a month name: a two-digit hour
   * ("10:00 AM") and a two-digit day are each a character longer than the one-digit case the loop
   * above happens to use, and that character is what an axis clips.
   */
  it("covers the two-digit hour and day a sub-day unit can produce", () => {
    for (const unit of ["hour", "minute"] as const) {
      const widest = widestPeriodName(unit).length;
      for (let m = 0; m < 12; m++) {
        for (const h of [0, 1, 9, 10, 11, 12, 13, 21, 23]) {
          const v = `2026-${String(m + 1).padStart(2, "0")}-28T${String(h).padStart(2, "0")}:45:00`;
          expect(periodName(unit, v).length).toBeLessThanOrEqual(widest);
        }
      }
    }
  });

  /**
   * The one assertion here that guards a FUTURE change rather than this one. Nothing ties
   * `PERIOD_FORMAT`'s keys to `TEMPORAL_UNITS`, so a truncation unit added upstream — the backend's
   * `TemporalUnit::parse` and this list move together — would silently reinstate the exact defect
   * this module exists to fix: its buckets fall through to the raw first day on the axis, and Task
   * 3 sizes its ticks against a zero width. Driven off `TEMPORAL_UNITS` rather than a copied list,
   * so adding the unit is what turns this red.
   */
  it("can name every truncation unit egret supports", () => {
    const trunc = TEMPORAL_UNITS.filter((u) => u.group === "trunc").map((u) => u.token);
    expect(trunc.length).toBeGreaterThan(0);
    for (const token of trunc) {
      expect(widestPeriodName(token), `no period name for "${token}"`).not.toBe("");
      expect(periodName(token, "2026-01-05T13:00:00")).not.toBe("2026-01-05T13:00:00");
    }
  });

  /** Total too, and deliberately zero-width rather than plausibly wrong — see the function. */
  it("has no width to offer for a unit that names no period", () => {
    expect(widestPeriodName(undefined)).toBe("");
    expect(widestPeriodName("day_of_week")).toBe("");
    expect(widestPeriodName("fortnight")).toBe("");
  });
});
