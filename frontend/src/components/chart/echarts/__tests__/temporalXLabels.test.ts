import { describe, it, expect, vi } from "vitest";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";
import type { fitXLabels as FitXLabels } from "../axisLabelFit";

type FitArgs = Parameters<typeof FitXLabels>[0];

/** Every argument the builder hands the fitter, in call order. `vi.hoisted` for the same reason
 *  `cartesianXLabels.test.ts` needs it: the `vi.mock` factory below is hoisted above these
 *  imports and would otherwise touch the binding in its temporal dead zone. */
const { fitCalls } = vi.hoisted(() => ({ fitCalls: [] as FitArgs[] }));

/** A pass-through spy, not a stub — the real ladder still decides. All it adds is a view of the
 *  `labels` the builder measures, which is otherwise UNOBSERVABLE from here: `measure-text.ts`
 *  returns `{0, 0}` under jsdom by design, so every label is 0px wide and measuring the wrong
 *  string yields the same decision as measuring the right one. The consequence is a browser
 *  question; which strings were handed over is not, and that is what is asserted. */
vi.mock("../axisLabelFit", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../axisLabelFit")>();
  return {
    ...actual,
    fitXLabels: (args: FitArgs) => {
      fitCalls.push(args);
      return actual.fitXLabels(args);
    },
  };
});

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

// `satisfies`, not a bare object: spread properties escape excess-property checking, so `xAxisTyp`
// here would compile clean and silently make every case below a `varchar` column — the "format
// wins" test would then go green while testing nothing it names. Same hole `as never` opened on
// the argument, one scope out.
const base = {
  kind: "bar" as const,
  xAxis: "d",
  yMetrics: ["rev"],
  metricTypes: { rev: "bigint" },
  xAxisType: "date",
} satisfies Partial<CartesianBuildConfig>;

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
   * A format object SILENT ABOUT DATES does not outrank the period name, and this is not
   * hypothetical: `FormatSection`'s `patch` spreads into `{...fmt, ...p}` (`settings/
   * FormatSection.tsx:67`) and `styleColumnsFor` offers the x dimension to it for every cartesian
   * viz (`settings/SharedDisplaySections.tsx:27-38`). So renaming the date column to "Month" on a
   * month-bucketed bar stored `{column_title: "Month"}` — truthy — which took arm 1, and the axis
   * reverted from "January 2026" to `formatDate`'s default "January 1, 2026" with nothing in the
   * panel to explain it. An unrelated rename undoing this task's fix.
   */
  it("is not displaced by a format that says nothing about dates", () => {
    expect(label({ xAxisUnit: "month", columnSettings: { d: { column_title: "Month" } } }))
      .toBe("January 2026");
    // A title typed and then deleted persists the key with no value. Still a truthy object.
    expect(label({ xAxisUnit: "month", columnSettings: { d: { column_title: undefined } } }))
      .toBe("January 2026");
    // A date switch toggled on and back off leaves the falsy value, which means "not set".
    expect(label({ xAxisUnit: "month", columnSettings: { d: { date_abbreviate: false } } }))
      .toBe("January 2026");
    // …but the same switch left ON is a date instruction, and takes arm 1 with the default style.
    expect(label({ xAxisUnit: "month", columnSettings: { d: { date_abbreviate: true } } }))
      .toBe("Jan 1, 2026");
  });

  /**
   * The higher bar applies ONLY where a period name is in play. On an unbucketed axis there is
   * nothing for the format to outrank, so it keeps winning on existence — anything else would be
   * this task changing behaviour it was not sent to change.
   */
  it("keeps the old existence test where there is no period name to outrank", () => {
    expect(label({ columnSettings: { d: { column_title: "When" } } })).toBe("January 1, 2026");
  });

  /**
   * THE FITTER MEASURES THE STRING THE AXIS DRAWS.
   *
   * It measured `categories.map(String)` — the raw bucket — while the formatter drew the period
   * name, and the comment there justified the gap as "the formatted label is wider, so this
   * under-measures". Naming the periods falsified both halves at once: the two now differ whenever
   * the axis is bucketed, which is this PR's whole subject rather than the rare `columnSettings`
   * case that comment named, and the period name is often NARROWER — a year measured "2026-01-01"
   * at ten characters to draw "2026" at four. Over-measuring rotates labels that would have sat
   * upright, in the PR that exists because labels did not fit.
   *
   * Asserted as the ARGUMENT rather than as a rotation, because no rotation is reachable from
   * here: every label is 0px wide under jsdom, so both strings produce the same decision. See the
   * spy above.
   */
  it("hands the fitter the drawn labels, not the raw buckets", () => {
    fitCalls.length = 0;
    buildCartesianOption(
      COLS,
      [["2025-01-01", 1], ["2026-01-01", 2]],
      { ...base, xAxisUnit: "year" },
      { width: 320, height: 200 },
    );
    expect(fitCalls).toHaveLength(1);
    expect(fitCalls[0].labels).toEqual(["2025", "2026"]);
  });

  it("measures the column's format too, where that is what wins", () => {
    fitCalls.length = 0;
    buildCartesianOption(
      COLS, ROWS,
      { ...base, xAxisUnit: "month", columnSettings: { d: { date_style: "YYYY-MM-DD" } } },
      { width: 320, height: 200 },
    );
    expect(fitCalls[0].labels).toEqual(["2026-01-01", "2026-02-01"]);
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
