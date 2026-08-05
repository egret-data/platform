/**
 * What the trend card compares its latest number AGAINST — Metabase's "Comparisons", the list that
 * sits on its trend Data tab and that egret did not have.
 *
 * A clean-room port of `SmartScalar/compute.ts:341-655` and the validity/default rules in
 * `SmartScalar/utils.ts`, kept in its own module for the reason those two files are separate
 * upstream: this is arithmetic over rows that are already in the browser, and it must be testable
 * without a card, a card size, or a canvas around it.
 *
 * NO QUERY CHANGE, AND NO BACKEND CHANGE, and that is a measured fact rather than an intention.
 * Every type below is a pure function of the rows the card already fetched — `previousValue` walks
 * back through them, `previousPeriod` and `periodsAgo` search them for a date, `staticNumber` reads
 * only its own setting. egret's trend descriptor asks for the whole grouped series
 * (`viz/simple.tsx`'s `dimsAndMetrics` under the shared `row_limit`, with no trend-specific LIMIT
 * anywhere), so the row a comparison wants is already here. `crates/egret-bi/src/chartparams.rs`
 * validates `params.settings` for SHAPE only and has a test locking that
 * (`accepts_settings_keys_this_build_does_not_know`), so the new stored key needs nothing there.
 *
 * ═══ THE FIFTH TYPE, AND WHY IT IS NOT HERE ═══
 *
 * Metabase has a fifth comparison, `anotherColumn`: compare this row's metric against a DIFFERENT
 * numeric column of the same row. It is left out, and the line is sharper than "it was more work":
 *
 *   - It is the ONLY one of the five that changes the query. The other four read columns the trend
 *     already asks for; `anotherColumn` needs a second metric in the spec, which today is
 *     `dimsAndMetrics([c.dimension], [c.metric], s)` — exactly one metric, by construction.
 *   - Metabase itself hides the option when there is no second numeric column to offer
 *     (`getColumnsForComparison` filters to numeric columns that are not the primary field, and
 *     `getComparisonOptions` only pushes the menu entry when that list is non-empty). Driven on the
 *     reference instance at :3033, a Count-by-month trend's type menu reads exactly
 *     "Previous month / _ months ago / Previous value / Custom value…" — the four below and no
 *     fifth. So on every trend egret can currently build, this list IS Metabase's list.
 *
 * Adding it is therefore a query-shape change plus a column picker, and it belongs with whatever
 * gives the trend a second metric. Named here so the omission is a decision rather than a gap
 * someone rediscovers.
 *
 * Reordering the list by drag is also not here. Metabase's rows carry a drag handle; with a cap of
 * three rows, delete-and-re-add reaches every ordering, and a dnd-kit sortable is a bigger
 * dependency than the ordering it would buy. Rows render in stored order, which is authored order.
 */

import dayjs from "dayjs";
// `.week()` is NOT core dayjs — without this plugin the accessor is `undefined` and
// `areDatesTheSame`'s second guard throws on a week-bucketed trend rather than returning a wrong
// answer. Metabase gets it from a global registration in its own bootstrap; egret has no such
// bootstrap, so the module that needs it registers it. `dayjs.extend` is global and idempotent.
import weekOfYear from "dayjs/plugin/weekOfYear";
// Same story for `.quarter()` and for `subtract(n, "quarter")`. `temporal-period.ts` already
// extends this one; repeating it here costs nothing and stops this module depending on whether
// that one happened to be imported first — the reason it gives for its own duplicate registration.
import quarterOfYear from "dayjs/plugin/quarterOfYear";
import { namesAPeriod, parseInOwnZone } from "./temporal-period";

dayjs.extend(weekOfYear);
dayjs.extend(quarterOfYear);

/** Metabase's `COMPARISON_TYPES` (`SmartScalar/constants.ts`) less `anotherColumn`; see the header. */
export const COMPARISON_TYPES = ["previousPeriod", "periodsAgo", "previousValue", "staticNumber"] as const;

export type TrendComparisonType = (typeof COMPARISON_TYPES)[number];

/** Metabase's `MAX_COMPARISONS` (`SmartScalar/constants.ts:24`). Its "Add comparison" button
 *  disables at three — verified in the reference instance's own panel, not read off the constant. */
export const MAX_COMPARISONS = 3;

export interface TrendComparison {
  /** Stable across edits, so React can key the rows and a reorder cannot make one row inherit
   *  another's half-typed input. Metabase mints a uuid; this uses whatever the editor supplies. */
  id: string;
  type: TrendComparisonType;
  /**
   * ONE FIELD SERVING TWO TYPES, which is Metabase's shape and is kept rather than split:
   * `periodsAgo` reads it as a count of periods, `staticNumber` as the number itself. Splitting it
   * into `periods` and `number` would read better and would make a settings blob written by egret
   * unreadable by Metabase and vice versa, which is the whole point of copying the key names.
   */
  value?: number;
  /** `staticNumber` only — what the caption calls it. Required for that type; see `isValid`. */
  label?: string;
}

/**
 * The period name each unit contributes to a "vs previous …" caption. Metabase writes
 * "vs. previous month" rather than a bare "vs. previous" (`formatPreviousPeriodOptionName`,
 * `SmartScalar/utils.ts`) — the reader should not have to remember which granularity the chart was
 * grouped by to know what it is being compared against.
 *
 * Lived in `SmartScalarChart.tsx` until this module existed; moved rather than copied, because the
 * settings panel needs the same words to label its type menu ("Previous month") and two lists of
 * unit names that must agree is a thing that stops agreeing.
 */
const PERIOD_NAME: Record<string, string | undefined> = {
  minute: "minute", hour: "hour", day: "day", week: "week",
  month: "month", quarter: "quarter", year: "year",
};

/**
 * The units a period comparison can be computed at — the same seven, asked twice.
 *
 * GATED ON `namesAPeriod` FIRST, deliberately. That is `temporal-period.ts`'s single membership
 * rule for "this unit denotes a period", derived there from the format table so it cannot drift
 * from what `periodName` will actually render. Asking it first means a unit this module could
 * subtract but the card could not NAME never reaches a caption: the failure mode of getting that
 * backwards is a correct number under a caption reading its raw bucket value.
 *
 * THE TWO GUARDS OVERLAP TODAY AND NEITHER IS ALONE, which mutation proved rather than suggested:
 * deleting the `namesAPeriod` line survived the whole suite, because the seven names below already
 * exclude everything it would have rejected. That makes it an EQUIVALENT MUTANT and not a hole —
 * no fixture can tell the guards apart while they agree, exactly as `periodName`'s own pair of
 * guards records about itself. What the gate buys is the DIRECTION of the drift: if a unit is ever
 * removed from `PERIOD_FORMAT` (so the card can no longer name it) without being removed here, the
 * gate degrades this to "no period comparison" rather than to a comparison captioned with a raw
 * bucket value. The list below cannot express that, because it must be a literal tuple to give
 * `ComparableUnit` its type.
 */
const COMPARABLE_UNITS = ["minute", "hour", "day", "week", "month", "quarter", "year"] as const;
type ComparableUnit = (typeof COMPARABLE_UNITS)[number];

export function comparableUnit(unit: string | undefined): ComparableUnit | undefined {
  if (!namesAPeriod(unit)) return undefined;
  return (COMPARABLE_UNITS as readonly string[]).includes(unit) ? (unit as ComparableUnit) : undefined;
}

/** "previous month" — the caption tail for a one-period-back comparison, or undefined when the
 *  chart has no unit and so has no period to name. */
export function previousPeriodName(unit: string | undefined): string | undefined {
  const u = comparableUnit(unit);
  return u ? `previous ${PERIOD_NAME[u]}` : undefined;
}

/** The series a comparison is computed over. Deliberately NOT the chart's props: this takes column
 *  INDICES and a formatter, so it can be exercised without a React tree. */
export interface TrendSeries {
  rows: unknown[][];
  /** Dimension column index, or -1 when the chart has no dimension configured. */
  di: number;
  /** Metric column index. */
  mi: number;
  /** The dimension's temporal unit, when it has one. */
  unit?: string;
  /**
   * Names the period a raw dimension value covers — THE CARD'S OWN LABELLER, passed in rather than
   * rebuilt here. The caption under the number and the caption beside a comparison have to agree
   * about granularity and about the user's stored column format; the way to guarantee that is for
   * there to be one function, not two that are currently the same.
   */
  periodLabel: (v: unknown) => string;
}

export interface ComparisonResult {
  id: string;
  /** The caption's "vs …" phrase, with no "vs" and no colon — the card owns that punctuation. */
  label: string;
  /**
   * `undefined` MEANS "THIS COMPARISON HAS NO VALUE", and it is the whole point of the module.
   * A `previousPeriod` over a hole in the series resolves to undefined rather than reaching back to
   * whatever row happens to be there, so the card can say "(No data)" instead of stating something
   * false. Metabase's `CHANGE_TYPE_OPTIONS.MISSING` is the same distinction.
   */
  value?: number;
}

/** Whether a row carries a number this chart can read.
 *
 * `!= null && !== ""` IS LOAD-BEARING and is egret's own guard, stronger than upstream's
 * `isEmpty`: `Number(null)` and `Number("")` are both `0`, and `0` is finite — so a trailing period
 * with no data yet passed for a real one, and the card reported a metric as having collapsed to
 * nothing when the truth was that nothing had been recorded. See `SmartScalarChart`'s `valid`,
 * where this was found. */
function hasMetric(row: unknown[] | undefined, mi: number): boolean {
  if (!row) return false;
  const v = row[mi];
  return v != null && v !== "" && Number.isFinite(Number(v));
}

/** Upstream also requires a non-empty DATE before a row can be a comparison, because a row with no
 *  period cannot be named in a caption. Vacuously true when the chart has no dimension at all. */
function hasDate(row: unknown[] | undefined, di: number): boolean {
  if (!row) return false;
  if (di < 0) return true;
  const v = row[di];
  return v != null && v !== "";
}

/**
 * The last row before `beforeIndex` that carries both a date and a number — Metabase's
 * `findPreviousNonEmptyRowIndex` (`visualizations/lib/trend-helpers.ts`), verbatim in behaviour.
 *
 * Used for BOTH ends: the card's own latest row is this asked at `rows.length`, and a
 * `previousValue` comparison is this asked at the latest row's index. One function, because they
 * are one question, and because the alternative — a `filter` into a dense array — loses the index
 * into `rows` that `periodsAgo` needs to bound its search.
 */
export function findPreviousNonEmptyRowIndex(
  rows: unknown[][], di: number, mi: number, beforeIndex: number,
): number {
  for (let i = Math.min(beforeIndex, rows.length) - 1; i >= 0; i--) {
    if (hasMetric(rows[i], mi) && hasDate(rows[i], di)) return i;
  }
  return -1;
}

/**
 * Whether a stored comparison can be drawn — Metabase's `isComparisonValid` (`SmartScalar/utils.ts`).
 *
 * This is not defensive programming. The settings panel writes a row the moment it is added, so a
 * half-finished custom value ("500", no name yet) is a REAL state that reaches this on every
 * keystroke. Drawing it would print "vs : 500"; dropping it makes the card fall back to the default
 * comparison, which is what upstream does and what the user sees while typing.
 */
export function isComparisonValid(c: TrendComparison, unit: string | undefined): boolean {
  if (!c.id) return false;
  if (c.type === "previousValue") return true;
  if (c.type === "staticNumber") {
    return Number.isFinite(c.value) && typeof c.label === "string" && c.label.trim() !== "";
  }
  // previousPeriod and periodsAgo both need a period to count in.
  if (comparableUnit(unit) == null) return false;
  if (c.type === "periodsAgo") return Number.isInteger(c.value) && (c.value as number) >= 1;
  return true;
}

/**
 * What the card compares against when nothing is stored — Metabase's `getDefaultComparison`.
 *
 * THE TWO ARMS ARE THE FIX FOR THE CAPTION THAT LIED. A bucketed trend defaults to
 * `previousPeriod`, which is the caption egret already printed ("vs previous month") now attached
 * to the arithmetic that caption describes. An UNbucketed one has no period to count, so it
 * defaults to `previousValue` — the row walk — and that arm names the period it actually landed on
 * rather than claiming a granularity the chart does not have.
 */
export function defaultComparison(unit: string | undefined): TrendComparison {
  return { id: "default", type: comparableUnit(unit) != null ? "previousPeriod" : "previousValue" };
}

/** The comparisons to draw: the valid stored ones, or the default when there are none — Metabase's
 *  `getComparisons`. Capped, because a card that draws more caption lines than it has room for is
 *  the narrow-card problem this chart already has once. */
export function resolveComparisons(
  stored: TrendComparison[] | undefined, unit: string | undefined,
): TrendComparison[] {
  const valid = (stored ?? []).filter((c) => isComparisonValid(c, unit)).slice(0, MAX_COMPARISONS);
  return valid.length > 0 ? valid : [defaultComparison(unit)];
}

/**
 * dayjs's own number for a unit, used ONLY by `areDatesTheSame`'s offset guard.
 *
 * A switch rather than `d[unit]()` so that a unit with no accessor is a compile error instead of a
 * `TypeError` in a formatter, and so that ONE QUIRK STAYS VISIBLE: dayjs's `.day()` is the DAY OF
 * WEEK (0–6), not the day of the month. That is what upstream calls and it is carried verbatim,
 * because it is sufficient here — the guard only ever runs after `diff(…, "day") === 0`, which
 * already bounds the two dates to under 24 hours apart, and any two instants that close either fall
 * on the same weekday or on adjacent ones. `.date()` would read more obviously; changing it would
 * be a silent divergence from the code this is a port of, for no behavioural gain.
 */
function unitNumber(d: dayjs.Dayjs, unit: ComparableUnit): number {
  switch (unit) {
    case "minute": return d.minute();
    case "hour": return d.hour();
    case "day": return d.day();
    case "week": return d.week();
    case "month": return d.month();
    case "quarter": return d.quarter();
    case "year": return d.year();
  }
}

function diffIn(a: dayjs.Dayjs, b: dayjs.Dayjs, unit: ComparableUnit): number {
  // `quarter` is a `QUnitType` rather than an `OpUnitType`; the two do not union into anything
  // `diff` accepts, so the call site narrows on it — the same split `temporal-period.ts` records
  // for `add`.
  return unit === "quarter" ? a.diff(b, "quarter") : a.diff(b, unit);
}

/**
 * Whether two bucket dates name the SAME period — Metabase's `areDatesTheSame`
 * (`SmartScalar/compute.ts:627-655`), including the second guard, which is the only real semantic
 * risk in this whole feature and is therefore ported with its reasoning intact.
 *
 * The first guard alone is not enough WHEN THE TWO VALUES CARRY DIFFERENT UTC OFFSETS. Upstream's
 * own worked example: target `2023-12-01T00:00-04:00`, candidate `2023-11-01T00:00-05:00`. The
 * candidate is `2023-11-01T01:00-04:00` in the target's frame, so it is not a FULL month behind —
 * it is 0 < x < 1 months behind — and `diff(…, "month")` truncates that to 0, which reads as "same
 * period". Comparing the unit's own number as well rejects it.
 *
 * This can be reached in egret: a query that spans a DST boundary returns buckets on either side of
 * it with different offsets, and both `parseInOwnZone` and upstream's `parseZone` deliberately
 * preserve those offsets rather than normalising them away. Without the second guard, a monthly
 * trend across such a boundary can match the wrong month and quietly compare against it.
 */
function areDatesTheSame(candidate: dayjs.Dayjs, target: dayjs.Dayjs, unit: ComparableUnit): boolean {
  if (diffIn(target, candidate, unit) !== 0) return false;
  if (unitNumber(target, unit) !== unitNumber(candidate, unit)) return false;
  return true;
}

/**
 * The row exactly `periodsAgo` periods before the latest one, or `undefined` — Metabase's
 * `getRowOfPeriodsAgo`.
 *
 * THE MATCH IS THE POINT, not the bounds. A candidate row is only accepted if `areDatesTheSame`
 * says it IS the target period, so a hole in the series ends the search rather than being stepped
 * over — which is exactly the behaviour egret was missing.
 *
 * THE TWO BOUNDS ARE BOTH EQUIVALENT MUTANTS, and that is stated here because mutation showed it
 * and the next reader would otherwise take them for load-bearing:
 *
 *   - removing the early bail (`diffIn(...) > 0`), and
 *   - unbounding the window (`end = 0`)
 *
 * each survived the whole suite, and the reason is not thin coverage. THE ROWS ARE IN ASCENDING
 * DIMENSION ORDER — the trend descriptor derives that order explicitly and refuses `shared.orderby`
 * so it cannot be overridden, and a GROUP BY makes the dimension unique. Under that precondition,
 * once a candidate is older than the target every earlier row is older still, so no later iteration
 * could have matched anyway: both bounds are pure short-circuits over an answer that does not
 * change. They are kept because upstream keeps them, because the cost of a linear scan of a
 * thousand-row series inside a render is real, and because the day the ordering precondition is
 * relaxed they stop being equivalent — but no assertion should be written to pin them, because an
 * assertion that cannot fail is worse than none.
 */
function rowPeriodsAgo(
  series: TrendSeries, latestIndex: number, periodsAgo: number,
  target: dayjs.Dayjs, unit: ComparableUnit,
): unknown[] | undefined {
  const start = latestIndex - 1;
  if (start < 0) return undefined;
  const end = Math.max(start - (periodsAgo - 1), 0);
  for (let i = start; i >= end; i--) {
    const row = series.rows[i];
    const candidate = parseInOwnZone(row?.[series.di]);
    if (areDatesTheSame(candidate, target, unit) && hasMetric(row, series.mi)) return row;
    // Past the target already: every earlier row is earlier still.
    if (diffIn(target, candidate, unit) > 0) return undefined;
  }
  return undefined;
}

/**
 * Compute one comparison against the row at `latestIndex`.
 *
 * Total: every arm returns a `ComparisonResult` with a caption, and only the VALUE is optional. A
 * comparison that cannot find its row still names what it looked for, because "vs previous month:
 * (No data)" tells the reader something and a missing line tells them nothing.
 */
export function computeComparison(
  c: TrendComparison, series: TrendSeries, latestIndex: number,
): ComparisonResult {
  const { rows, di, mi, unit, periodLabel } = series;

  if (c.type === "staticNumber") {
    return { id: c.id, label: c.label ?? "", value: c.value };
  }

  if (c.type === "previousValue") {
    const prev = findPreviousNonEmptyRowIndex(rows, di, mi, latestIndex);
    if (prev === -1) {
      // No earlier row at all. There is nothing to name, so the caption falls back to the bare
      // form rather than inventing a period.
      return { id: c.id, label: "previous" };
    }
    // NAMES THE PERIOD IT ACTUALLY LANDED ON — never "previous month". That is the correction:
    // this is the row walk, and on a gapped series the row it reaches is not the previous period.
    // Metabase captions this arm with the found row's own date too (`vs. Dec 2028` on the driven
    // reference card), so the two agree.
    const label = di >= 0 ? periodLabel(rows[prev][di]) : "previous";
    return { id: c.id, label, value: Number(rows[prev][mi]) };
  }

  // previousPeriod and periodsAgo are ONE computation with a different count, which is also how
  // upstream models it — `computeTrendPeriodsAgo` serves both and reads `value ?? 1`.
  const u = comparableUnit(unit);
  const back = c.type === "periodsAgo" ? (c.value ?? 1) : 1;
  // Unreachable through `resolveComparisons`, which drops both types when there is no unit. Kept
  // because this function is exported and a caller that skipped validation should get an honest
  // "no value" rather than a thrown error inside a render.
  if (u == null || latestIndex < 0) {
    return { id: c.id, label: previousPeriodName(unit) ?? "previous" };
  }

  const latestDate = parseInOwnZone(rows[latestIndex]?.[di]);
  const target = u === "quarter" ? latestDate.subtract(back, "quarter") : latestDate.subtract(back, u);
  const found = rowPeriodsAgo(series, latestIndex, back, target, u);

  // One period back is named ("previous month"); further back is DATED, because "3 months ago" and
  // the month's own name are the same length and only one of them says which month. Upstream draws
  // the same distinction on the same `dateUnitsAgo === 1` test.
  const label = back === 1
    ? (previousPeriodName(unit) ?? "previous")
    // The found row's own date when there is one, else the date that was searched for — so the
    // caption names the period even when nothing was recorded in it.
    : periodLabel(found && di >= 0 ? found[di] : target.format());

  return { id: c.id, label, value: found ? Number(found[mi]) : undefined };
}
