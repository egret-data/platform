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
 * words upstream (`options/cartesian.ts:647`).
 */

import dayjs from "dayjs";
// `Q` (quarter) is not in dayjs's core token set, and a quarter bucket has to render "Q3 2026".
// `format.ts` extends the same plugin for the same reason; `dayjs.extend` is global and idempotent,
// so registering it here too costs nothing and stops this module depending on whether some other
// module happened to be imported first.
import advancedFormat from "dayjs/plugin/advancedFormat";
// For `dayjs.utc`, which is how a value carrying an explicit offset keeps its OWN wall clock — see
// `parseInOwnZone`.
import utc from "dayjs/plugin/utc";
import { isExtractUnit } from "./temporal-units";

dayjs.extend(advancedFormat);
dayjs.extend(utc);

/**
 * The dayjs pattern each truncation unit's period is named with, each one Metabase's, from
 * `frontend/src/metabase/visualizations/lib/formatting/date.tsx` unless said otherwise:
 *
 *   year    `DEFAULT_DATE_FORMATS.year`, `:23`.
 *   quarter `DEFAULT_DATE_FORMATS.quarter`, `:24`. Note the space — "Q3 2026". The cljc port says
 *           "Q3 - 2022" (`util/formatting/internal/date_formatters.cljc:36`) and is not what the
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
 */
const PERIOD_FORMAT: Record<string, string> = {
  year: "YYYY",
  quarter: "[Q]Q YYYY",
  month: "MMMM YYYY",
  week: "MMMM D, YYYY",
  day: "MMMM D, YYYY",
  hour: "MMMM D, YYYY, h:mm A",
  minute: "MMMM D, YYYY, h:mm A",
};

/** A trailing `Z` or `±HH:MM`, the two forms that carry a zone. */
const EXPLICIT_OFFSET = /(?:Z|([+-])(\d{2}):?(\d{2}))$/;

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
    const m = EXPLICIT_OFFSET.exec(value);
    if (m) {
      // `dayjs.parseZone` does not exist in dayjs 1.x the way it does in moment; `.utcOffset(n)`
      // on an already-correct instant is the supported way to fix the zone the value RENDERS in.
      if (m[1] == null) return dayjs.utc(value);
      const minutes = (Number(m[2]) * 60 + Number(m[3])) * (m[1] === "-" ? -1 : 1);
      return dayjs(value).utcOffset(minutes);
    }
  }
  return dayjs(value as string | number | Date);
}

/**
 * Name the period a bucket value covers, or return the value unchanged when it does not name one.
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
  // period from it would be nonsense ("3" is a weekday, not a year). `PERIOD_FORMAT` has no entry
  // for one either, so this states the rule rather than being the only thing enforcing it — it is
  // here because the rule is easy to break by adding a fallback below and hard to notice broken.
  if (unit == null || isExtractUnit(unit)) return String(value);
  const fmt = PERIOD_FORMAT[unit];
  if (fmt == null) return String(value);
  const d = parseInOwnZone(value);
  return d.isValid() ? d.format(fmt) : String(value);
}

/**
 * The representative values a unit's widest name is searched over — Metabase's
 * `getRepresentativeDates` (`echarts/cartesian/utils/timeseries.ts:305-349`), whose contract is
 * "the date with the longest string representation for the unit is contained in the array".
 *
 * Its shape is copied, including which axes collapse to a single value at coarse units, because
 * every one of them is an observation about a format rather than about a date: month names differ
 * in length so all twelve are tried; a two-digit day and a two-digit hour are longer than a
 * one-digit one; and AM/PM can differ in length, so both halves of the day are tried at the units
 * that print a time.
 *
 * The reference year is fixed. A year is four digits either way, and the alternative — `dayjs()` —
 * would make the tick layout of a chart depend on the day it is looked at.
 */
const REFERENCE_YEAR = 2026;

function representativeValues(unit: string): string[] {
  const months =
    unit === "year" ? [0] : unit === "quarter" ? [0, 3, 6, 9] : [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
  const coarserThanDay = unit === "year" || unit === "quarter" || unit === "month" || unit === "week";
  // A two-digit day; the exact one does not matter because no format here prints a weekday name.
  const days = coarserThanDay ? [10] : [10, 11, 12, 13, 14, 15, 16];
  const hours = coarserThanDay || unit === "day" ? [10] : [10, 20];

  const out: string[] = [];
  for (const month of months) {
    for (const day of days) {
      for (const hour of hours) {
        // A NAIVE string, deliberately: it has no zone for `parseInOwnZone` to preserve, so it is
        // read as the wall clock written here and the answer does not move with the viewer.
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
 * The longest name this unit CAN produce — not the longest one in the data.
 *
 * The tick chooser sizes an axis against this (PR 03 task 3), the way Metabase measures
 * `getRepresentativeDates` rather than its dataset (`timeseries.ts:280-299`). A month axis whose
 * visible range happens to be May–July would otherwise size itself on "May 2026" and clip
 * "September 2026" the moment the range moved — a chart whose layout depends on which slice of it
 * you are looking at.
 *
 * Longest by CHARACTER COUNT, which is a proxy for width and not the same thing ("WWW" outruns
 * "iiiiii"). It is Metabase's own proxy at the same spot (`timeseries.ts:293-295`), and the caller
 * measures the returned string with the real measurer, so the proxy only chooses BETWEEN
 * candidates — it never decides a width.
 *
 * TOTAL, and "" for anything that is not a truncation unit: a caller measuring "" gets a width of
 * zero, which is a visible nonsense rather than a plausible wrong answer, and the only caller
 * already guards on the unit being a truncation one before asking.
 */
export function widestPeriodName(unit: string | undefined): string {
  if (unit == null || PERIOD_FORMAT[unit] == null) return "";
  return representativeValues(unit).reduce((longest, v) => {
    const name = periodName(unit, v);
    return name.length > longest.length ? name : longest;
  }, "");
}
