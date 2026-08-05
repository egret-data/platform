import { describe, it, expect, vi } from "vitest";
import { buildCartesianOption } from "../options/cartesian";
import { buildWaterfallOption } from "../options/waterfall";
import { periodName } from "../../temporal-period";

/**
 * Metabase's fifth "Show lines and tick marks" choice, "Compact" — the one egret's four-way select
 * never offered.
 *
 * WHAT IT ACTUALLY IS, and it is smaller than the name suggests. Read in Metabase's three x-axis
 * model builders (`echarts/cartesian/model/axis.ts:716` timeseries, `:778` numeric, `:856`
 * category), which say the same thing three times:
 *
 *     compact: settings["graph.x_axis.axis_enabled"] === "compact",
 *
 * passed into `formatValue`'s options. It is a FORMATTING flag on the label text. Nothing about
 * layout, tick density or rotation is touched — and for dates `compact` is literally the same
 * switch as `date_abbreviate`, which egret already implements:
 *
 *     const getMonthFormat = (options) =>
 *       options.compact || options.date_abbreviate ? "MMM" : "MMMM";
 *     (`frontend/src/metabase/visualizations/lib/formatting/date.tsx:717-718`)
 *
 * MEASURED LIVE at :3033 before any of this was written, because the observable effect and the rule
 * are NOT the same thing and this programme has been bitten six times by copying the first for the
 * second:
 *
 *   "MB · Line", 12 monthly buckets, Show:     March 2026 / May 2026 / July 2026 / September 2026 /
 *                                              November 2026 / January 2026        — SIX labels
 *   the same chart, Compact:                   Feb 2026 / Mar 2026 / Apr 2026 / … / Jan 2026
 *                                                                                  — TWELVE labels
 *
 * A reader of that pair would reasonably conclude Compact means "draw every label". It does not.
 * Twelve labels are a CONSEQUENCE: shorter strings stop colliding, so the tick chooser stops
 * thinning. Porting "draw every label" would have been the value, not the rule — and it would break
 * the moment a card was narrow enough that even "Jan 2026" did not fit. The tests below therefore
 * assert the formatting directly and the label count only as something that follows.
 *
 *   "MB · Bar", x = CATEGORY (Widget / Gadget / Gizmo / Doohickey), Compact: byte-identical. The
 *   option is offered there and does nothing, because `formatValue`'s `compact` has no effect on a
 *   string. That measurement is what decides which of egret's charts get the option below.
 *
 * Nothing was saved; every card driven was re-fetched afterwards with its settings unchanged.
 */

vi.mock("@/components/chart/measure-text", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/components/chart/measure-text")>();
  const { platformApi: api } = await import("zrender/lib/core/platform.js");
  return {
    ...actual,
    measureText: (text: string, style: { size: string | number; family: string; weight: string | number }) => {
      const size = typeof style.size === "number" ? `${style.size}px` : style.size;
      return { width: api.measureText(text, `${style.weight} ${size} ${style.family}`).width, height: 0 };
    },
  };
});

describe("periodName — the compact period", () => {
  it("abbreviates the month name and nothing else", () => {
    // Metabase's rule is a token swap on the month (and the weekday, which no period format uses):
    // MMMM -> MMM. The year and the day-of-month are untouched, so a compact month is not a
    // different format, it is the same one shortened.
    expect(periodName("month", "2026-01-01")).toBe("January 2026");
    expect(periodName("month", "2026-01-01", true)).toBe("Jan 2026");
    expect(periodName("day", "2026-01-05")).toBe("January 5, 2026");
    expect(periodName("day", "2026-01-05", true)).toBe("Jan 5, 2026");
    expect(periodName("hour", "2026-01-05T13:00:00Z", true)).toBe("Jan 5, 2026, 1:00 PM");
  });

  it("leaves a unit with no month token exactly as it was", () => {
    // The whole point of expressing this as a token swap rather than as a second table: a quarter
    // and a year have no MMMM to shorten, so compact is a no-op on them for free. A second table
    // would have needed two more entries that could drift.
    expect(periodName("year", "2026-01-01", true)).toBe("2026");
    expect(periodName("quarter", "2026-07-01", true)).toBe("Q3 2026");
  });

  it("stays TOTAL under compact — an extraction unit and a non-date still fall through", () => {
    expect(periodName("day_of_week", "3", true)).toBe("3");
    expect(periodName("month", "not a date", true)).toBe("not a date");
    expect(periodName("month", null, true)).toBe("");
  });

  it("names a WEEK by its first day, compact or not — the span form still does not exist here", () => {
    // `periodName`'s own doc records that Metabase's span form ("January 5–11, 2026") lives in a
    // different function used by the trend card, not the axis. Compact does not smuggle it in.
    expect(periodName("week", "2026-01-05", true)).toBe("Jan 5, 2026");
  });
});

const columns = [{ name: "month", type: "date" }, { name: "revenue", type: "double" }];
const rows = [
  ["2026-01-01", 100], ["2026-02-01", 120], ["2026-03-01", 140], ["2026-04-01", 160],
  ["2026-05-01", 180], ["2026-06-01", 200], ["2026-07-01", 220], ["2026-08-01", 240],
  ["2026-09-01", 260], ["2026-10-01", 280], ["2026-11-01", 300], ["2026-12-01", 320],
];
const opt = (cfg: Record<string, unknown>, size?: { width: number; height: number }) =>
  buildCartesianOption(columns, rows, cfg as never, size) as unknown as {
    xAxis: { axisLabel: { formatter: (v: string) => string; interval: unknown } };
  };

describe("compact x labels — the cartesian category axis", () => {
  const base = { kind: "line", xAxis: "month", yMetrics: ["revenue"], xAxisUnit: "month" };

  it("draws the full period name by default", () => {
    expect(opt(base).xAxis.axisLabel.formatter("2026-01-01")).toBe("January 2026");
  });

  it("draws the abbreviated one under xLabelCompact", () => {
    expect(opt({ ...base, xLabelCompact: true }).xAxis.axisLabel.formatter("2026-01-01")).toBe("Jan 2026");
  });

  it("compacts through a STORED column format too, not only the period name", () => {
    // Arm 1 of `drawLabel`. Metabase spreads its `compact` over the column's own settings
    // (`{...columnSettings, compact: ...}`), so a user who has set a date style still gets it
    // abbreviated rather than having their format silently replaced.
    const cfg = {
      // `xAxisType` is what `ChartRenderer` threads from the RESULT column
      // (`ChartRenderer.tsx:641`); without it arm 1 formats a date as a varchar and hands the raw
      // string back, which is a fixture bug rather than a finding — the first draft of this test hit
      // exactly that and read `'2026-01-05'`.
      ...base, xLabelCompact: true, xAxisType: "date",
      columnSettings: { month: { date_style: "MMMM D, YYYY" } },
    };
    expect(opt(cfg).xAxis.axisLabel.formatter("2026-01-05")).toBe("Jan 5, 2026");
  });

  it("compacts a NUMBER on the category axis, which is the other half of Metabase's one flag", () => {
    const numCols = [{ name: "bucket", type: "bigint" }, { name: "n", type: "double" }];
    const numRows = [[12400, 1], [25500, 2]];
    const o = buildCartesianOption(numCols, numRows, {
      kind: "bar", xAxis: "bucket", yMetrics: ["n"], xLabelCompact: true,
      xAxisType: "bigint", columnSettings: { bucket: {} },
    } as never) as unknown as { xAxis: { axisLabel: { formatter: (v: string) => string } } };
    expect(o.xAxis.axisLabel.formatter("12400")).toBe("12.4K");
  });

  it("THINS LESS as a consequence, because the fitter measures the string the axis draws", () => {
    // The live 6-labels-becomes-12 observation, reproduced — but as an OUTCOME of shorter strings
    // and never as an instruction. `fitXLabels` is handed `drawnCategories`, which is the compact
    // closure's own output, so nothing here had to be told about compaction at all. If this ever
    // starts failing while the formatter tests above still pass, the fitter has stopped measuring
    // what the axis draws — which is the exact drift the `labels:` comment in the builder warns of.
    const narrow = { width: 420, height: 240 };
    const full = opt({ ...base }, narrow).xAxis.axisLabel.interval as number;
    const short = opt({ ...base, xLabelCompact: true }, narrow).xAxis.axisLabel.interval as number;
    // ECharts' category `interval` counts labels SKIPPED, so a smaller number is a denser axis.
    expect(short).toBeLessThan(full);
  });

  it("leaves a chart that has not asked for it byte-identical", () => {
    // The cheapest guard against a formatter that quietly changed for everyone.
    expect(JSON.stringify(opt(base))).toBe(JSON.stringify(opt({ ...base, xLabelCompact: undefined })));
  });
});

describe("compact x labels — the waterfall", () => {
  /**
   * THE WATERFALL GETS THIS, and that overturns a decision recorded twice in this repo.
   *
   * `settings/ChartAxesSection.tsx` says Compact "shortens temporal axis labels, and these charts' x
   * is a category run through its column format, so the option would do nothing here", and
   * `CartesianDisplaySection.tsx` repeats the premise. The premise is false for the waterfall:
   * `options/waterfall.ts:234-242` carries `bucketedX = namesAPeriod(cfg.xAxisUnit)` and a
   * `drawLabel` whose second arm is `periodName(cfg.xAxisUnit, v)` — the same two arms, on the same
   * module, as the cartesian builder. A month-bucketed waterfall draws "January 2026" today.
   *
   * It IS true for the box plot, which is why that chart still does not get the option: its axis
   * has no `axisLabel.formatter` at all (`options/boxplot.ts` draws the raw group key, and says so),
   * so there is nothing for a formatting flag to act through. Offering it there would be the inert
   * control this programme keeps removing.
   */
  const wfCols = [{ name: "month", type: "date" }, { name: "delta", type: "double" }];
  const wfRows = [["2026-01-01", 10], ["2026-02-01", -4], ["2026-03-01", 7]];
  const wf = (cfg: Record<string, unknown>) =>
    buildWaterfallOption(wfCols, wfRows, cfg as never) as unknown as {
      xAxis: { axisLabel: { formatter: (v: string) => string } };
    };

  it("draws the full period name by default and the abbreviated one under xLabelCompact", () => {
    const base = { xAxis: "month", metric: "delta", xAxisUnit: "month" };
    expect(wf(base).xAxis.axisLabel.formatter("2026-01-01")).toBe("January 2026");
    expect(wf({ ...base, xLabelCompact: true }).xAxis.axisLabel.formatter("2026-01-01")).toBe("Jan 2026");
  });

  /**
   * THE TWO BELOW EXIST BECAUSE A MUTATION SURVIVED, and the shape of the gap is worth recording
   * because it is one this repo has been caught by four times: a behaviour a comment argues for at
   * length, with no assertion pinning it.
   *
   * `drawLabel` has two arms that Compact reaches — the stored column format (arm 1) and the period
   * name (arm 2) — and the test above exercises only arm 2. Deleting the whole compact spread from
   * arm 1 (`const colFmt = compactX ? {...stored, compact, date_abbreviate} : stored` reduced to
   * `const colFmt = stored`) left the suite fully green, because the fixture above stores NO column
   * settings: `stored` is `undefined`, `formatWins` is false, and every label falls through to arm 2,
   * which its own mutation already pins. So the surviving mutant was not a weakness in the period
   * arm — it was a fixture that could not reach the other one.
   *
   * The two cases are deliberately the waterfall's copies of the cartesian suite's arm-1 pair, since
   * the two builders run the same three arms in the same order on purpose and the coverage should not
   * be asymmetric either.
   */
  it("compacts through a STORED column format, the arm the period-name test cannot reach", () => {
    // Arm 1 on a BUCKETED axis: `specifiesDateRendering` is true because `date_style` is set, so the
    // stored format outranks the period name — and Compact must still abbreviate it rather than
    // being silently dropped on the way through.
    const cfg = {
      xAxis: "month", metric: "delta", xAxisUnit: "month",
      columnSettings: { month: { date_style: "MMMM D, YYYY" } },
    };
    expect(wf(cfg).xAxis.axisLabel.formatter("2026-01-05")).toBe("January 5, 2026");
    expect(wf({ ...cfg, xLabelCompact: true }).xAxis.axisLabel.formatter("2026-01-05")).toBe("Jan 5, 2026");
  });

  it("compacts a NUMBER on a non-bucketed waterfall — the other half of Metabase's one flag", () => {
    // No `xAxisUnit`, so `bucketedX` is false and arm 1 wins on existence alone. This is the case
    // that proves Compact reaches the NUMBER half here as well as the date half: `periodName` is
    // total and would hand back `"12400"` untouched, so nothing but the format arm can produce this.
    const numCols = [{ name: "bucket", type: "bigint" }, { name: "delta", type: "double" }];
    const numRows = [[12400, 10], [25500, -4]];
    const num = (cfg: Record<string, unknown>) =>
      buildWaterfallOption(numCols, numRows, cfg as never) as unknown as {
        xAxis: { axisLabel: { formatter: (v: string) => string } };
      };
    const cfg = { xAxis: "bucket", metric: "delta", columnSettings: { bucket: {} } };
    expect(num(cfg).xAxis.axisLabel.formatter("12400")).toBe("12,400");
    expect(num({ ...cfg, xLabelCompact: true }).xAxis.axisLabel.formatter("12400")).toBe("12.4K");
  });
});
