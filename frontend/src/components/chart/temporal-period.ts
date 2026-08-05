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
 * words upstream ON THE CARTESIAN AXIS (`options/cartesian.ts`, the `displayCats` line).
 * `options/waterfall.ts` does NOT substitute them, so `periodName` is called there with an
 * unsubstituted `"3"` and its extraction guard is REACHED IN PRODUCTION rather than only in
 * principle — see that guard, which was written when cartesian was the only caller and stated this
 * substitution as though it held for every one.
 *
 * So: the substitution is ONE CALLER'S HABIT, not a premise this module may assume. Anything here
 * that depends on an extraction value never arriving has to defend itself instead.
 *
 * IT ALSO OWNS THE TICK LADDER (`boundarySteps`), because that is the same knowledge asked a second
 * way. `namesAPeriod` is the single membership rule for "this unit denotes a period", and the
 * question a thinned axis asks — how many buckets apart may two drawn labels be — is keyed on it.
 * `echarts/axisLabelFit.ts` stays free of any temporal knowledge as a result: it takes the answer as
 * an input it cannot manufacture, which is what makes "an ordinal axis is never thinned" structural
 * rather than a rule someone must remember. It used to ask a second question here — how wide can a
 * label at this unit get — and the note above `boundarySteps` records why it stopped.
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
 *           function, not a change to this one, because the axis and the tick chooser both measure
 *           what THIS one returns and would be sizing one string while drawing another.
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
 * EXPORTED for `trend-comparison.ts`, which needs the identical parse for a different question:
 * upstream's `computeComparisonPeriodsAgo` and `areDatesTheSame` are written throughout against
 * `dayjs.parseZone`, and a "previous month" that lands on the wrong month because the viewer sits
 * west of the data is the same defect this function exists for, arriving through arithmetic
 * instead of through a format string.
 *
 * This is not a nicety. A bucket labelled `2026-01-01T13:00:00Z` is the 13:00 bucket of the query
 * that produced it; handing it to a bare `dayjs()` renders it at the browser's offset, so the same
 * chart names the same bucket "1:00 PM" in London and "8:00 PM" in Saigon — and on a day or month
 * axis a westward offset moves the label to the WRONG PERIOD entirely, which is the same class of
 * defect this module exists to fix. Naive strings ("2026-01-01") have no zone to preserve and are
 * parsed locally, which reproduces the wall clock written in them.
 */
export function parseInOwnZone(value: unknown) {
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
 *
 * `compact` IS METABASE'S "Compact" x-axis mode, and it is A TOKEN SWAP ON THE PATTERN rather than
 * a second table of formats. That is not a shortcut — it is what Metabase does, verbatim:
 *
 *     const getMonthFormat = (options) =>
 *       options.compact || options.date_abbreviate ? "MMM" : "MMMM";
 *     const getDayFormat = (options) =>
 *       options.compact || options.date_abbreviate ? "ddd" : "dddd";
 *     (`frontend/src/metabase/visualizations/lib/formatting/date.tsx:714-718`)
 *
 * and `formatDate` in egret's own `format.ts:103` already implements the identical swap for
 * `date_abbreviate`. Expressing it the same way here buys two things a second table would not: a
 * unit with no month token (a year, a quarter) is a no-op FOR FREE rather than through two more
 * entries that could drift, and the day-of-month and the year are untouched by construction, so a
 * compact month is the same format shortened rather than a different format.
 *
 * `dddd` is swapped too even though no `PERIOD_FORMAT` pattern contains one. Carried because it is
 * half of Metabase's rule and the cost is nine characters; dropping it would make the day the next
 * person adds a weekday pattern the day this quietly stops matching upstream.
 */
export function periodName(unit: string | undefined, value: unknown, compact = false): string {
  // Same nullish answer as `formatValue` (`format.ts:80`) so an empty cell reads the same wherever
  // it is drawn.
  if (value == null) return "";
  // Naming a period from an extraction unit would be nonsense, so this refuses to.
  //
  // THIS BRANCH IS REACHED IN PRODUCTION, which it was not when the comment here began "an
  // extraction unit's value is a finished word before it ever reaches a formatter". That was true
  // of the only caller there was: `options/cartesian.ts` substitutes through `temporalLabel`
  // (`displayCats`) before the axis sees a value. `options/waterfall.ts`, the second caller, does
  // NOT — so a `day_of_week` waterfall calls this with a raw `"3"` and leaves through this line.
  // The module header carries the general form: the substitution is one caller's habit and not a
  // premise anything here may assume.
  //
  // What it prevents is specific and silent: dayjs accepts an extraction value as a date.
  // `dayjs("3").isValid()` is `true`, and it formats as "March 2001". `PERIOD_FORMAT` has no entry
  // for an extraction token either, so the two guards still overlap and neither is alone — but the
  // day anyone adds a fallback past that lookup, a `day_of_week` of 3 stops reading "Wed" and
  // starts reading a year nobody's data covers. No test can tell the two guards apart while they
  // agree, which is why the reason is written out here instead.
  if (unit == null || isExtractUnit(unit)) return String(value);
  const raw = PERIOD_FORMAT[unit];
  if (raw == null) return String(value);
  // AFTER both guards, so compact cannot smuggle a value past them: an extraction unit and an
  // unknown unit still leave through the raw string whatever the flag says.
  const fmt = compact ? raw.replace(/MMMM/g, "MMM").replace(/dddd/g, "ddd") : raw;
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
 * the call — the axis formatter still calls `periodName` unguarded. What the FORMATTING callers
 * need it for is to know whether a period name is IN PLAY at all: the trend card because its own
 * fallback is `formatValue` rather than the raw string (a date column with no format there still
 * reads "March 1, 2026", and dropping that would be a regression), and both of them because a
 * stored column format only has to clear a higher bar when there is a period name for it to
 * outrank.
 *
 * A THIRD KIND OF CALLER arrived with the trend's comparisons and is asking something else again:
 * `trend-comparison.ts`'s `comparableUnit` uses this to decide whether a period can be SUBTRACTED,
 * not merely rendered. That is the same membership question reached from the arithmetic side, and
 * routing it through here is what stops a "previous month" being offered at a granularity the
 * caption could only print as a raw bucket value.
 *
 * A type predicate rather than a bare `boolean` so `boundarySteps` can use it as its own membership
 * check — and narrow `unit` to `string` for the `dayjs` `diff` inside it — instead of restating the
 * same lookup.
 */
export function namesAPeriod(unit: string | undefined): unit is string {
  return unit != null && PERIOD_FORMAT[unit] != null;
}

/*
 * THIS FILE ALSO EXPORTED `widestPeriodName` UNTIL PR 03's LAST COMMIT — the longest name a unit
 * could produce, built by mapping a copy of Metabase's `getRepresentativeDates` through the caller's
 * own label closure. Its only consumer was the tick chooser, which measured it once and charged it
 * at every rung of the stride ladder, and that turned out to be measuring a string the axis does not
 * draw: twelve monthly buckets at stride 12 draw "January 2026", not "September 2026". The chooser
 * now slices each rung's drawn labels out of the array it is already handed, so nothing was left for
 * this to decide and it is deleted rather than kept as an unused bound. `axisLabelFit.ts`'s
 * `strideThatFits` carries the browser measurement and the argument, including what the unit's
 * worst case was buying (a stride that did not move with the data's range) and why a category axis
 * that is rebuilt on every resize and every result does not need it.
 */

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
