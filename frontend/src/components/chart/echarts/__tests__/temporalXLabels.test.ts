import { describe, it, expect, vi, afterEach } from "vitest";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";
import type { fitXLabels as FitXLabels, XLabelFit } from "../axisLabelFit";
import { boundarySteps } from "@/components/chart/temporal-period";

type FitArgs = Parameters<typeof FitXLabels>[0];

/** Every argument the builder hands the fitter, in call order, and an optional CANNED decision to
 *  hand back. `vi.hoisted` for the same reason `cartesianXLabels.test.ts` needs it: the `vi.mock`
 *  factory below is hoisted above these imports and would otherwise touch the binding in its
 *  temporal dead zone. */
const { fitCalls, fitResult } = vi.hoisted(() => ({
  fitCalls: [] as FitArgs[],
  fitResult: { value: undefined as XLabelFit | undefined },
}));

/**
 * A pass-through spy by default — the real ladder still decides — over two things this file cannot
 * observe any other way, both because `measure-text.ts` returns `{0, 0}` under jsdom by design.
 *
 * The ARGUMENTS, because with every label 0px wide, measuring the wrong string or sizing a stride
 * against the wrong candidate yields the same decision as doing it right. Which strings were handed
 * over is not a browser question, and that is what is asserted.
 *
 * The DECISION, via `fitResult`, because no `plotWidth` reachable from here can make a 0px label
 * collide, so `kind: "thin"` is unreachable through the real fitter under jsdom. The ladder itself
 * is driven with a stub measurer in `axisLabelFit.test.ts`; what is left for this file is the
 * builder's half of the contract — what a `thin` decision turns into on the axis.
 */
vi.mock("../axisLabelFit", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../axisLabelFit")>();
  return {
    ...actual,
    fitXLabels: (args: FitArgs) => {
      fitCalls.push(args);
      return fitResult.value ?? actual.fitXLabels(args);
    },
  };
});

afterEach(() => {
  fitResult.value = undefined;
  fitCalls.length = 0;
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

type Axis = {
  axisLabel?: { formatter?: (v: unknown) => string; interval?: unknown; rotate?: number; show?: boolean };
};
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

/**
 * A bucketed axis too narrow for all its labels draws every Nth, and only a BUCKETED one does.
 *
 * The decision itself is `axisLabelFit.ts`'s and is driven rung by rung with a stub measurer in its
 * own test. What is left here is the builder's half: the precondition it enforces before a stride
 * is even offered, the candidate it sizes that stride against, and the `interval` a `thin` decision
 * turns into.
 */
describe("a bucketed temporal x-axis thins to period boundaries", () => {
  const SIZE = { width: 320, height: 200 };
  const buildWithSize = (cfg: Partial<CartesianBuildConfig>) =>
    buildCartesianOption(COLS, ROWS, { ...base, ...cfg }, SIZE);
  /** Clears the spy's log first, so a test that builds more than once reads THIS build's argument
   *  and not the first one it happened to make. */
  const boundaryOf = (cfg: Partial<CartesianBuildConfig>) => {
    fitCalls.length = 0;
    buildWithSize(cfg);
    expect(fitCalls, "the fitter was not consulted at all").toHaveLength(1);
    return fitCalls[0].boundary;
  };

  /**
   * The candidate a stride is measured against is the worst case for the UNIT — "September 2026" —
   * and the ladder is the one Metabase's `TICKS_TIMESERIES_INTERVALS` gives at and above `month`.
   * Both are read from `temporal-period.ts`, so this asserts the wiring rather than restating the
   * values, which `temporal-period.test.ts` pins.
   */
  it("sizes the stride against the widest label the unit can produce", () => {
    expect(boundaryOf({ xAxisUnit: "month" })).toEqual({
      widestLabel: "September 2026",
      steps: boundarySteps("month"),
    });
    expect(boundaryOf({ xAxisUnit: "quarter" })?.widestLabel).toBe("Q1 2026");
  });

  /**
   * AND THAT CANDIDATE IS A STRING THE AXIS DRAWS, which is the property the hoisted `drawLabel`
   * closure exists to keep. Measuring one string while rendering another is the drift closed one
   * level down; sizing a stride against a candidate the formatter would never produce reopens it
   * one level up, and nothing about a 0px jsdom measurement would show it.
   *
   * Proved by feeding the formatter the value the candidate was derived from and demanding the same
   * string back — not by re-deriving the candidate, which would compare the implementation to
   * itself.
   */
  it("sizes it against a string the axis would really draw", () => {
    const drawn = (cfg: Partial<CartesianBuildConfig>, v: string) =>
      axisOf(buildWithSize(cfg)).axisLabel?.formatter?.(v);
    // 2026-09-10T10:00:00 is one of the unit's representative values; September is the long month.
    expect(drawn({ xAxisUnit: "month" }, "2026-09-10T10:00:00")).toBe("September 2026");
    expect(boundaryOf({ xAxisUnit: "month" })?.widestLabel).toBe("September 2026");
  });

  /**
   * A stored column format outranks the period name on the axis (`drawLabel`, arm 1), so it has to
   * outrank it in the candidate too — otherwise a chart whose labels read "2026-01-10" picks its
   * stride from the width of "September 2026", four characters wider, and thins further than it
   * needs to. This is what `widestPeriodName`'s `draw` parameter is for.
   */
  it("sizes it against the column's format where that is what wins", () => {
    const fmt = { d: { date_style: "YYYY-MM-DD" } };
    const b = boundaryOf({ xAxisUnit: "month", columnSettings: fmt });
    expect(b?.widestLabel).toBe("2026-01-10");
    expect(axisOf(buildWithSize({ xAxisUnit: "month", columnSettings: fmt }))
      .axisLabel?.formatter?.("2026-01-10T10:00:00")).toBe("2026-01-10");
  });

  /**
   * THE NEGATIVE, and the one the whole design rests on. A non-temporal axis of the same label
   * count at the same width gets no `boundary` at all, so `fitXLabels` cannot reach its stride arm
   * — PR 02's all-or-nothing is what an ordinal axis still gets, because there is no such thing as
   * every second product. An unbucketed DATE column is the same case: its values are whatever the
   * engine returned, and striding them would invent a granularity the query never asked for.
   */
  it("offers no stride to an axis that is not bucketed", () => {
    const ordinal = buildCartesianOption(
      [{ name: "p", type: "varchar" }, { name: "rev", type: "bigint" }],
      [["Widget", 10], ["Gadget", 4]],
      { kind: "bar", xAxis: "p", yMetrics: ["rev"], metricTypes: { rev: "bigint" }, xAxisType: "varchar" },
      SIZE,
    );
    expect(ordinal).toBeTruthy();
    expect(fitCalls).toHaveLength(1);
    expect(fitCalls[0].boundary).toBeUndefined();

    expect(boundaryOf({})).toBeUndefined();
    expect(boundaryOf({ xAxisUnit: "day_of_week" })).toBeUndefined();
  });

  /**
   * `interval` COUNTS THE LABELS SKIPPED, NOT THE STRIDE. ECharts reads it as
   * `step = max((interval || 0) + 1, 1)` (`echarts/lib/scale/helper.js:236`), so every 6th is 5. An
   * off-by-one here draws January, June and November instead of January and July, which still looks
   * like a plausible date axis in a screenshot — hence an assertion rather than an eyeball.
   */
  it("writes the stride as the count of labels ECharts should skip", () => {
    fitResult.value = { kind: "thin", rotate: 0, every: 6 };
    const axis = axisOf(buildWithSize({ xAxisUnit: "month" })).axisLabel;
    expect(axis?.interval).toBe(5);
    // Upright and shown: thinning is what lets the labels stay horizontal, and it is not a hide.
    expect(axis?.rotate).toBe(0);
    expect(axis?.show).toBe(true);
  });

  /**
   * The other three arms are untouched by this change, which is the claim "PR 02 was not undone"
   * has to survive: a decided axis still writes 0 — every label or none — and only `thin` writes
   * anything else.
   */
  it("still writes every-label-or-none for every other decision", () => {
    for (const value of [
      { kind: "show", rotate: 0 },
      { kind: "rotate", rotate: 45 },
      { kind: "hide" },
    ] as XLabelFit[]) {
      fitResult.value = value;
      expect(axisOf(buildWithSize({ xAxisUnit: "month" })).axisLabel?.interval, value.kind).toBe(0);
      fitCalls.length = 0;
    }
  });

  /** A stride of 1 is every label, so it must write 0 and not -1 — which ECharts' `|| 0` would
   *  silently read as 0 anyway, hiding the arithmetic error rather than the behaviour. */
  it("writes 0 for a stride of one rather than a negative interval", () => {
    fitResult.value = { kind: "thin", rotate: 0, every: 1 };
    expect(axisOf(buildWithSize({ xAxisUnit: "month" })).axisLabel?.interval).toBe(0);
  });
});
