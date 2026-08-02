import { describe, it, expect } from "vitest";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";

/**
 * A bucketed temporal x-axis draws the NAME OF THE PERIOD, not the bucket's first day.
 *
 * The defect: a chart grouped by month labelled its categories "2026-01-01". A reader who sees a
 * date on an axis concludes the chart is daily, and — the reason this PR exists at all — ten of
 * those labels never fit on a one-third-width card, so PR 02's all-or-nothing ladder drew NONE.
 * Metabase draws "January 2026".
 */

const COLS = [{ name: "d", type: "date" }, { name: "rev", type: "bigint" }];
const ROWS = [["2026-01-01", 10], ["2026-02-01", 4]];

const base = {
  kind: "bar" as const,
  xAxis: "d",
  yMetrics: ["rev"],
  metricTypes: { rev: "bigint" },
  xAxisType: "date",
};

type Axis = { axisLabel?: { formatter?: (v: unknown) => string } };
const axisOf = (opt: unknown) => {
  const o = opt as { xAxis: Axis | Axis[]; yAxis: Axis | Axis[] };
  return (Array.isArray(o.xAxis) ? o.xAxis[0] : o.xAxis) as Axis;
};
const label = (cfg: Partial<CartesianBuildConfig>, v: unknown = "2026-01-01") =>
  axisOf(buildCartesianOption(COLS, ROWS, { ...base, ...cfg })).axisLabel?.formatter?.(v);

describe("a bucketed temporal x-axis names its periods", () => {
  it("names the period at each truncation unit the chart can be grouped by", () => {
    expect(label({ xAxisUnit: "year" })).toBe("2026");
    expect(label({ xAxisUnit: "quarter" }, "2026-07-01")).toBe("Q3 2026");
    expect(label({ xAxisUnit: "month" })).toBe("January 2026");
    expect(label({ xAxisUnit: "day" })).toBe("January 1, 2026");
    expect(label({ xAxisUnit: "hour", xAxisType: "timestamp" }, "2026-01-01T13:00:00Z"))
      .toBe("January 1, 2026, 1:00 PM");
  });

  /**
   * The guard, and the negative half of it matters more than the positive. A column that was NOT
   * bucketed still reaches the axis as whatever the engine returned, and naming a period from it
   * would be inventing a granularity the query never asked for. This is the case an existing test
   * in `cartesian.test.ts` has pinned since before this PR; it is repeated here because the guard
   * that keeps it true is the one this file adds.
   */
  it("leaves an unbucketed date column raw", () => {
    expect(label({})).toBe("2026-01-01");
  });

  it("leaves a non-temporal axis alone", () => {
    const opt = buildCartesianOption(
      [{ name: "p", type: "varchar" }, { name: "rev", type: "bigint" }],
      [["Widget", 10]],
      { kind: "bar", xAxis: "p", yMetrics: ["rev"], metricTypes: { rev: "bigint" }, xAxisType: "varchar" },
    );
    expect(axisOf(opt).axisLabel?.formatter?.("Widget")).toBe("Widget");
  });

  /**
   * An extraction unit's value is a finished word by the time the axis sees it — `displayCats`
   * substituted it upstream — so the formatter must not try to read "Wed" as a date. It parses:
   * `dayjs("Wed")` is invalid, but `dayjs("3")` for a `day_of_week` of 3 is NOT, and formats as
   * "March 2001". This asserts the raw-value arm both ways round.
   */
  it("leaves an extraction unit's finished word alone", () => {
    const opt = buildCartesianOption(
      [{ name: "d", type: "date" }, { name: "rev", type: "bigint" }],
      [[3, 10]],
      { ...base, xAxisUnit: "day_of_week" },
    );
    expect(axisOf(opt).axisLabel?.formatter?.("Wed")).toBe("Wed");
    expect(axisOf(opt).axisLabel?.formatter?.("3")).toBe("3");
  });

  /**
   * A user who set a date style has SAID what they want, and a stored-then-ignored setting is the
   * defect the pivot, the pie and the breakout legend each shipped with in turn. So the column
   * format outranks the period name — including when it disagrees with the bucketing.
   */
  it("lets the column's own format win over the period name", () => {
    expect(label({ xAxisUnit: "month", columnSettings: { d: { date_style: "YYYY-MM-DD" } } }))
      .toBe("2026-01-01");
  });

  it("still names the period when the format is on a DIFFERENT column", () => {
    // The lookup is keyed on `cfg.xAxis`; a format stored for the metric must not reach the x-axis.
    expect(label({ xAxisUnit: "month", columnSettings: { rev: { date_style: "YYYY-MM-DD" } } }))
      .toBe("January 2026");
  });

  it("draws nothing for an empty category rather than naming a period from it", () => {
    expect(label({ xAxisUnit: "month" }, null)).toBe("");
  });

  /**
   * A horizontal bar puts the categories on the Y axis. Same column, same bucketing, same answer —
   * the axis this formatter lands on is not the one it is named after.
   */
  it("names the periods on the category axis of a horizontal bar, which is Y", () => {
    const opt = buildCartesianOption(COLS, ROWS, { ...base, horizontal: true, xAxisUnit: "month" }) as {
      yAxis: Axis | Axis[];
    };
    const y = (Array.isArray(opt.yAxis) ? opt.yAxis[0] : opt.yAxis) as Axis;
    expect(y.axisLabel?.formatter?.("2026-01-01")).toBe("January 2026");
  });
});
