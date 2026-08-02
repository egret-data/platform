/**
 * The name of the PERIOD a truncated temporal bucket covers — "January 2026", not "2026-01-01".
 *
 * The defect this exists for: a chart grouped by month draws its bucket values raw, and a bucket
 * value is the period's first DAY. A reader who sees a date on an axis reasonably concludes the
 * chart is daily. The trend card had the same defect from the other end — it printed
 * "December 1, 2026" as the caption for a monthly comparison, observed live on the ZZ Chart Parity
 * harness. Metabase draws the period.
 *
 * WHERE EVERY STRING BELOW COMES FROM. These are a parity contract, not a taste call, so each is
 * cited to the sibling Metabase checkout rather than chosen. Metabase has TWO date-formatting
 * implementations — `src/metabase/util/formatting/date.cljc` (the newer cljc port) and
 * `frontend/src/metabase/visualizations/lib/formatting/date.tsx` — and THEY DISAGREE (cljc renders
 * a month as "April, 2022" and a quarter as "Q4 - 2022"). The .tsx one is what a chart in a browser
 * actually goes through: the ECharts x-axis formatter is
 * `echarts/cartesian/model/axis.ts:706-721`, which calls `formatValue` -> `formatDateTimeWithUnit`
 * in that file. So the .tsx forms are the ones copied here.
 *
 * Extraction units ("day_of_week", "month_of_year", ...) are not periods and are not this module's
 * business — `temporalLabel` in `./temporal-units` has already turned their values into finished
 * words upstream (`options/cartesian.ts`, the `displayCats` line).
 *
 * IT ALSO OWNS THE TICK LADDER (`boundarySteps`), because that is the same knowledge asked a second
 * way. `namesAPeriod` is the single membership rule for "this unit denotes a period", and the two
 * questions a thinned axis asks — how wide can a label at this unit get, and how many buckets apart
 * may two of them be — are both keyed on it. `echarts/axisLabelFit.ts` stays free of any temporal
 * knowledge as a result: it takes the answers as an input it cannot manufacture, which is what
 * makes "an ordinal axis is never thinned" structural rather than a rule someone must remember.
 */

import dayjs from "dayjs";
// `Q` (quarter) is not in dayjs's core token set, and a quarter bucket has to render "Q3 2026".
// `format.ts` extends the same plugin for the same reason; `dayjs.extend` is global and idempotent,
// so registering it here too costs nothing and stops this module depending on whether some other
// module happened to be imported first.
import advancedFormat from "dayjs/plugin/advancedFormat";
// `add(n, "quarter")` is NOT core dayjs either, and its failure mode is silent rather than loud:
// an unrecognised unit falls through to MILLISECONDS, so `dayjs(0).add(1, "quarter")` without this
// is `1970-01-01T00:00:00.001Z` and `TICK_INTERVALS`' quarter rungs would come out as a stride of
// zero rather than of three months. Verified against dayjs 1.11.21 before this line was written.
import quarterOfYear from "dayjs/plugin/quarterOfYear";
// For `dayjs.utc`, which is how a value carrying an explicit offset keeps its OWN wall clock — see
// `parseInOwnZone`.
import utc from "dayjs/plugin/utc";
import { isExtractUnit } from "./temporal-units";

dayjs.extend(advancedFormat);
dayjs.extend(quarterOfYear);
dayjs.extend(utc);

/**
 * The dayjs pattern each truncation unit's period is named with, each one Metabase's, from
 * `frontend/src/metabase/visualizations/lib/formatting/date.tsx` unless said otherwise:
 *
 *   year    `DEFAULT_DATE_FORMATS.year`, `:23`.
 *   quarter `DEFAULT_DATE_FORMATS.quarter`, `:24`. Note the space — "Q3 2026". The cljc port says
 *           "Q3 - 2022" (`util/formatting/internal/date_formatters.cljc:35`) and is not what the
 *           browser runs.
 *   month   `DATE_STYLE_TO_FORMAT["MMMM D, YYYY"].month`, `:49-51` — the default date style's
 *           override for a month-bucketed column. Not "January, 2026", which is the cljc form.
 *   day     `DEFAULT_DATE_STYLE` itself (`utils/formatting/datetime-utils.ts:4`); no unit override.
 *   week    ALSO the plain date style, i.e. the week's first day. This looks like the defect above
 *           and is deliberate: Metabase expands a week into a span ("January 5–11, 2026") ONLY when
 *           `options.type` is "tooltip" or "cell" (`date.tsx:1328-1336`), and the axis formatter
 *           passes no `type` (`model/axis.ts:713-718`). An axis therefore reads "January 5, 2026"
 *           upstream too. If a caller ever needs the span form — Metabase's trend card does use it,
 *           via `formatDateTimeRangeWithUnit` (`SmartScalar/compute.ts:702`) — that is a second
 *           function, not a change to this one, because `widestPeriodName` below has to stay in
 *           step with what is actually drawn.
 *   hour,   the date style joined to the time style with ", " (`formatDateTimeWithFormats:1273`).
 *   minute  `time_enabled` defaults to "minutes" for the units `hasHour` admits (`:1341`,
 *           `datetime-utils.ts:6`), so seconds are not shown at either unit — a minute bucket has
 *           no seconds to show, and an hour bucket's ":00" is what says it is an hour.
 *
 * `| undefined` in the value type against the fact that `tsconfig.json:6` sets `strict` but NOT
 * `noUncheckedIndexedAccess`: without it an index read is typed `string`, `fmt == null` narrows to
 * `never`, and the totality guard this module's contract rests on is dead code as far as the
 * compiler is concerned — which is the state in which someone deletes it.
 */
const PERIOD_FORMAT: Record<string, string | undefined> = {
  year: "YYYY",
  quarter: "[Q]Q YYYY",
  month: "MMMM YYYY",
  week: "MMMM D, YYYY",
  day: "MMMM D, YYYY",
  hour: "MMMM D, YYYY, h:mm A",
  minute: "MMMM D, YYYY, h:mm A",
};

/**
 * The two suffixes that carry a zone, kept as two regexes so neither has an alternative whose
 * capture groups do not exist — both halves of Metabase's single one at
 * `frontend/src/metabase/utils/time-dayjs.ts:95`, character class included, along with two of its
 * quirks that are recorded rather than fixed so the two stay diffable:
 *
 *   - `:?` also accepts the basic form `±HHMM`, which ISO 8601 allows. Bare `±HH`, which it also
 *     allows, matches neither — and dayjs rejects that form outright, so such a value does not get
 *     read in the wrong zone, it falls through to the raw string. Both are covered by tests.
 *   - CASE-SENSITIVE. A lowercase `z` is legal RFC 3339 and falls through to a local parse. Trino
 *     and StarRocks both emit uppercase, which is why this has never been seen, and Metabase has
 *     the identical blind spot.
 */
const UTC_SUFFIX = /Z$/;
const NUMERIC_OFFSET = /([+-])(\d{2}):?(\d{2})$/;

/**
 * Parse a bucket value in ITS OWN zone rather than the viewer's — Metabase's `parseTimestamp`
 * (`frontend/src/metabase/utils/time-dayjs.ts:95-96`) does the same through `moment.parseZone`'s
 * dayjs equivalent.
 *
 * This is not a nicety. A bucket labelled `2026-01-01T13:00:00Z` is the 13:00 bucket of the query
 * that produced it; handing it to a bare `dayjs()` renders it at the browser's offset, so the same
 * chart names the same bucket "1:00 PM" in London and "8:00 PM" in Saigon — and on a day or month
 * axis a westward offset moves the label to the WRONG PERIOD entirely, which is the same class of
 * defect this module exists to fix. Naive strings ("2026-01-01") have no zone to preserve and are
 * parsed locally, which reproduces the wall clock written in them.
 */
function parseInOwnZone(value: unknown) {
  if (typeof value === "string") {
    if (UTC_SUFFIX.test(value)) return dayjs.utc(value);
    const m = NUMERIC_OFFSET.exec(value);
    if (m) {
      // `dayjs.parseZone` does not exist in dayjs 1.x the way it does in moment; `.utcOffset(n)`
      // on an already-correct instant is the supported way to fix the zone the value RENDERS in.
      const minutes = (Number(m[2]) * 60 + Number(m[3])) * (m[1] === "-" ? -1 : 1);
      return dayjs(value).utcOffset(minutes);
    }
  }
  return dayjs(value as string | number | Date);
}

/**
 * Name the period a bucket value covers, or return the value unchanged when it does not name one.
 *
 * A WEEK is named by its FIRST DAY — "January 5, 2026", not a span — because that is what an axis
 * draws upstream. The span form ("January 5–11, 2026") that Metabase's trend card and tooltips use
 * is a SEPARATE function which does not exist here yet, so a caller that wants it must add one
 * rather than widen this; `PERIOD_FORMAT` above carries the citations for both.
 *
 * TOTAL — never throws, for the same reason `temporalLabel` beside it is: this runs inside an
 * ECharts `formatter`, once per label, on whatever the query returned. An unknown unit, an
 * extraction unit, and a value that is not a date all fall through to the raw string.
 */
export function periodName(unit: string | undefined, value: unknown): string {
  // Same nullish answer as `formatValue` (`format.ts:80`) so an empty cell reads the same wherever
  // it is drawn.
  if (value == null) return "";
  // An extraction unit's value is a finished word before it ever reaches a formatter, so naming a
  // period from it would be nonsense. `PERIOD_FORMAT` has no entry for one either, so TODAY this
  // states the rule rather than being the only thing enforcing it — but what it prevents is
  // specific and silent: dayjs accepts an extraction value as a date. `dayjs("3").isValid()` is
  // `true`, and it formats as "March 2001". So the day anyone adds a fallback below this line, a
  // `day_of_week` of 3 stops reading "Wed" and starts reading a year nobody's data covers. No test
  // can defend the guard while there is no fallback to reach past it, which is why the reason is
  // written out here instead.
  if (unit == null || isExtractUnit(unit)) return String(value);
  const fmt = PERIOD_FORMAT[unit];
  if (fmt == null) return String(value);
  const d = parseInOwnZone(value);
  return d.isValid() ? d.format(fmt) : String(value);
}

/**
 * Whether this unit names a period at all — i.e. whether `periodName` will do anything with it.
 *
 * Derived from `PERIOD_FORMAT` rather than from `temporal-units`' trunc/extract split, so it cannot
 * drift from what `periodName` actually implements: a unit that is a truncation token but has no
 * format entry yet would be reported as naming a period while `periodName` fell through to raw.
 *
 * `periodName` is total, so a caller whose fallback IS the raw string does not need this to make
 * the call — the axis formatter still calls `periodName` unguarded. What both callers need it for
 * is to know whether a period name is IN PLAY at all: the trend card because its own fallback is
 * `formatValue` rather than the raw string (a date column with no format there still reads
 * "March 1, 2026", and dropping that would be a regression), and both of them because a stored
 * column format only has to clear a higher bar when there is a period name for it to outrank.
 *
 * A type predicate rather than a bare `boolean` so `widestPeriodName` can use it as its own
 * membership check instead of restating the same lookup ten lines below it.
 */
export function namesAPeriod(unit: string | undefined): unit is string {
  return unit != null && PERIOD_FORMAT[unit] != null;
}

/** The reference year every candidate below is built in. Fixed, because a year is four digits
 *  either way and the alternative — `dayjs()` — would make the tick layout of a chart depend on
 *  the day it is looked at. */
const REFERENCE_YEAR = 2026;

/**
 * The values a unit's widest name is searched over — Metabase's `getRepresentativeDates`
 * (`echarts/cartesian/utils/timeseries.ts:305-349`), whose contract is "the date with the longest
 * string representation for the unit is contained in the array".
 *
 * Its shape is copied, including which axes collapse to a single value at coarse units, because
 * every one of them is an observation about a FORMAT rather than about a date: month names differ
 * in length so all twelve are tried; a two-digit day and a two-digit hour are longer than a
 * one-digit one; and AM/PM can differ in length, so both halves of the day are tried at the units
 * that print a time.
 *
 * THE SEVEN CONSECUTIVE DAYS ARE PARITY BALLAST, and since they are 7/8 of this loop's cost the
 * reason belongs here rather than looking like an accident. Metabase's own comment is "test each
 * possible weekday name" (`timeseries.ts:325`): its `dddd, MMMM D, YYYY` date style prints one, and
 * weekday names differ in length. No format in `PERIOD_FORMAT` prints a weekday today, so all seven
 * collapse to the same answer — they are kept so this array can be diffed against upstream's line
 * for line, and they start earning their cost the day egret honours a weekday-bearing style.
 *
 * ONE DELIBERATE DIVERGENCE, because upstream is wrong here. Metabase builds each candidate as
 * `new Date(2026, month, day, hour).toISOString()` (`:344`) — a LOCAL-zone construction rendered to
 * UTC. On a +07 machine its own "two-digit hour" candidate of 10 comes back as `03`, and the array
 * quietly stops containing the longest representation its contract promises. These are naive
 * strings instead, with no zone for `parseInOwnZone` to preserve, so they are read as the wall
 * clock written here and the answer does not move with the viewer.
 */
function representativeValues(unit: string): string[] {
  const months =
    unit === "year" ? [0] : unit === "quarter" ? [0, 3, 6, 9] : [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
  const coarserThanDay = unit === "year" || unit === "quarter" || unit === "month" || unit === "week";
  const days = coarserThanDay ? [10] : [10, 11, 12, 13, 14, 15, 16];
  const hours = coarserThanDay || unit === "day" ? [10] : [10, 20];

  const out: string[] = [];
  for (const month of months) {
    for (const day of days) {
      for (const hour of hours) {
        const mm = String(month + 1).padStart(2, "0");
        const dd = String(day).padStart(2, "0");
        const hh = String(hour).padStart(2, "0");
        out.push(`${REFERENCE_YEAR}-${mm}-${dd}T${hh}:00:00`);
      }
    }
  }
  return out;
}

/**
 * Memoised, but ONLY FOR THE DEFAULT FORM — and since this comment used to claim otherwise, the
 * correction is the point of it. A `draw` mapper is a closure the axis builds fresh per render, so
 * there is no key this Map could hold it under; `widestPeriodName` reads and writes here only when
 * `draw` is absent.
 *
 * The sole production caller (`options/cartesian.ts`, the `boundary:` line) PASSES `draw`. So this
 * cache does not serve the layout path — an earlier version of this comment said it did, naming
 * exactly the caller that bypasses it, which is the shape of a performance claim nobody rechecks.
 * What it serves now is the plain `widestPeriodName(unit)` form and this module's own tests.
 *
 * WHAT THE LAYOUT PATH THEREFORE PAYS, measured rather than estimated (dayjs 1.11.21, Node 22, per
 * call, warm): 0.01ms at `year`, 0.03ms at `quarter`, 0.06ms at `month` and `week`, 0.36ms at `day`,
 * 0.83ms at `hour` and `minute`. `EChartsRenderer` coalesces to one rebuild per animation frame, so
 * the worst case is ~0.8ms of a 16.7ms frame for the whole of a drag-resize, on the two bucketings
 * that are rarest — and it sits beside a full option rebuild and an ECharts re-render that cost
 * considerably more. Left uncached deliberately: a cache keyed on anything less than the closure's
 * real inputs is a correctness risk, and this is not the frame budget's problem.
 *
 * If it ever becomes one, the key is constructible at the call site rather than here — `drawLabel`'s
 * output depends only on `cfg.xAxisUnit`, `cfg.xAxis`, `cfg.columnSettings?.[cfg.xAxis]` and
 * `cfg.xAxisType`.
 */
const WIDEST_BY_UNIT = new Map<string, string>();

/**
 * The longest name this unit can produce, BY CHARACTER COUNT — a candidate to measure, not a width.
 *
 * Not the longest name in the data. The tick chooser sizes an axis against this (`axisLabelFit.ts`,
 * the `boundary` arm), the way Metabase measures `getRepresentativeDates` rather than its dataset
 * (`timeseries.ts:280-299`). A month axis whose visible range happens to be May–July would
 * otherwise size itself on "May 2026" and clip "September 2026" the moment the range moved — a
 * chart whose layout depends on which slice of it you are looking at.
 *
 * Character count is a PROXY for width and not the same thing ("WWW" outruns "iiiiii"). It is
 * Metabase's own proxy at the same spot (`timeseries.ts:293-295`), and the caller measures the
 * returned string with the real measurer, so the proxy only chooses BETWEEN candidates — it never
 * decides a width.
 *
 * `draw` IS THE POINT OF THE PARAMETER, not a convenience. The tick chooser's candidate has to be a
 * string the axis would actually DRAW, or the fit is computed against one string while another is
 * rendered — the exact drift `options/cartesian.ts` closed by hoisting its `drawLabel` closure so
 * the fitter measures the axis's own output. A bucketed axis does not always draw the period name:
 * a stored column format outranks it (arm 1 there), and "September 2026" is then neither drawn nor
 * a bound on what is. So the axis passes its own `drawLabel` and this maps the same representative
 * VALUES through it. The default reproduces the plain period name, which is what the name says and
 * what this module's own tests pin.
 *
 * COST — one parse-and-format per representative value, and `representativeValues` collapses hard at
 * the coarse units rather than always producing twelve: **1** at `year` (one month, one day, one
 * hour), **4** at `quarter`, **12** at `month` and `week`, **84** at `day`, **168** at `hour` and
 * `minute`. Bounded by the unit rather than by the data, and the one caller pays it only on a
 * bucketed temporal axis that has a measured size — never on an ordinal axis and never on an
 * unmeasured container. The `draw` form is not memoised; `WIDEST_BY_UNIT` above says why, what the
 * layout path actually pays in milliseconds, and where a key would go if it ever mattered.
 *
 * TOTAL, and "" for anything that is not a truncation unit: a caller measuring "" gets a width of
 * zero, which is a visible nonsense rather than a plausible wrong answer. That is also the SECOND
 * of the two things stopping a non-temporal axis being thinned — the caller's own `namesAPeriod`
 * guard is the first — since a zero-width candidate is not what makes an axis fit, an empty
 * `boundarySteps` is.
 */
export function widestPeriodName(
  unit: string | undefined,
  draw?: (value: string) => string,
): string {
  if (!namesAPeriod(unit)) return "";
  const label = draw ?? ((v: string) => periodName(unit, v));
  const cached = draw == null ? WIDEST_BY_UNIT.get(unit) : undefined;
  if (cached != null) return cached;
  const widest = representativeValues(unit).reduce((longest, v) => {
    const name = label(v);
    return name.length > longest.length ? name : longest;
  }, "");
  if (draw == null) WIDEST_BY_UNIT.set(unit, widest);
  return widest;
}

/**
 * Metabase's `TICKS_TIMESERIES_INTERVALS` (`echarts/cartesian/utils/timeseries.ts:45-75`), from
 * `minute` up. Its `ms` and `second` rungs are dropped because no egret truncation unit is finer
 * than a minute, so `boundarySteps` could never start on one; everything from `minute` is carried
 * verbatim, INCLUDING the half-steps that carry no `testFn` upstream (2 weeks, 2 months,
 * 2 quarters, 2/5/50/100 years). Those exist there precisely because only the TICK chooser uses
 * them — `DATA_TIMESERIES_INTERVALS` filters them back out — and the tick chooser is what this is.
 *
 * Order is load-bearing twice over: `boundarySteps` finds its starting rung by unit-with-count-1
 * (Metabase's `_.findIndex`, `:225-230`) and then walks UP, taking the first stride that fits.
 */
/** `"quarter"` is spelled out beside `ManipulateType` rather than folded into it because the
 *  `quarterOfYear` plugin adds `add` as a SECOND overload (`quarterOfYear.d.ts`: `add(value, unit:
 *  QUnitType)`) rather than widening the first, and no overload accepts the union of the two — so a
 *  rung typed as that union matches no signature. Keeping the literal separate lets the call site
 *  narrow on it, which is a real check where a cast would be a promise. */
const TICK_INTERVALS: { unit: dayjs.ManipulateType | "quarter"; count: number }[] = [
  { unit: "minute", count: 1 }, { unit: "minute", count: 5 },
  { unit: "minute", count: 15 }, { unit: "minute", count: 30 },
  { unit: "hour", count: 1 }, { unit: "hour", count: 3 },
  { unit: "hour", count: 6 }, { unit: "hour", count: 12 },
  { unit: "day", count: 1 },
  { unit: "week", count: 1 }, { unit: "week", count: 2 },
  { unit: "month", count: 1 }, { unit: "month", count: 2 },
  { unit: "quarter", count: 1 }, { unit: "quarter", count: 2 },
  { unit: "year", count: 1 }, { unit: "year", count: 2 }, { unit: "year", count: 5 },
  { unit: "year", count: 10 }, { unit: "year", count: 50 }, { unit: "year", count: 100 },
];

/** Fixed and UTC, so a stride is a property of the ladder and not of the day or the zone it is
 *  computed in. The epoch is also what Metabase measures its own rung durations from
 *  (`getTimeSeriesIntervalDuration`, `:160-166`: `dayjs(0).add(count, unit)`). */
const STRIDE_REFERENCE = "1970-01-01T00:00:00Z";

const STEPS_BY_UNIT = new Map<string, number[]>();

/**
 * How many BUCKETS apart two drawn labels may be, cheapest rung first — the candidate strides a
 * bucketed category axis picks from.
 *
 * Metabase strides a real time axis in time, so its rungs stay `{unit, count}` and land on calendar
 * boundaries (`expectedTickCount`, `:169`). egret plots one CATEGORY per bucket and ECharts strides
 * a category axis by INDEX, so each rung is converted here into a count of the data's own unit:
 * every 3rd bucket of a month-bucketed axis is Metabase's 1-quarter rung. The conversion is a dayjs
 * `diff` in the data unit rather than a duration ratio, because a ratio is wrong at the rungs that
 * matter — 5 years is 1826 days including leap days, which over a 31-day "month" is 58.9 and rounds
 * to 59 rather than to 60.
 *
 * WHAT THAT COSTS, stated because it is the one place this is not Metabase: the stride is aligned
 * on the FIRST BUCKET, not on the calendar. Twelve months starting in January draw January and July
 * at stride 6, which is the parity case and the common one; twelve starting in March draw March and
 * September. Metabase would draw January and July there too. Fixing it means an `interval` CALLBACK
 * rather than a number (ECharts accepts one — `axisTickLabelBuilder.js:142`), which is a bigger
 * change than the defect needs and would break the "regular stride" property the justification at
 * the `interval` line rests on.
 *
 * `diff` truncates toward zero, which is why a month over a week-bucketed axis is 4 weeks and not
 * 4.43; the ladder is deduplicated and kept strictly increasing so a rung that collapses into its
 * predecessor is dropped rather than retried.
 *
 * EMPTY for anything that is not a truncation unit — an ordinal axis of product names has no such
 * thing as "every second product", and this is what makes that unrepresentable rather than merely
 * discouraged.
 */
export function boundarySteps(unit: string | undefined): number[] {
  if (!namesAPeriod(unit)) return [];
  const cached = STEPS_BY_UNIT.get(unit);
  if (cached != null) return cached;
  const start = TICK_INTERVALS.findIndex((i) => i.unit === unit && i.count === 1);
  // Not reachable while every `PERIOD_FORMAT` key has a count-1 rung above, which a test pins. It
  // is here so that adding a truncation unit to one table and not the other degrades to "no
  // thinning" rather than to a stride of `NaN` buckets.
  if (start < 0) return [];
  const ref = dayjs.utc(STRIDE_REFERENCE);
  const steps: number[] = [];
  for (let i = start; i < TICK_INTERVALS.length; i++) {
    const rung = TICK_INTERVALS[i];
    const end = rung.unit === "quarter" ? ref.add(rung.count, "quarter") : ref.add(rung.count, rung.unit);
    const step = end.diff(ref, unit as dayjs.QUnitType);
    if (step >= 1 && step > (steps[steps.length - 1] ?? 0)) steps.push(step);
  }
  STEPS_BY_UNIT.set(unit, steps);
  return steps;
}
