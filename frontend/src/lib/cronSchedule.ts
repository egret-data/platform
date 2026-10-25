/**
 * A small structured schedule vocabulary, and the two conversions between it
 * and a 5-field crontab.
 *
 * The storage format stays `crontab` — nothing about the scheduler changes.
 * What this adds is a shape the UI can render as dropdowns, so someone who
 * cannot write a step expression can still say "every 6 hours at quarter past".
 * Metabase does exactly this (`metabase/util/cron.clj`) and never shows a cron
 * expression at all; Superset keeps the raw field as an override. egret keeps
 * the override too, which is what `toCron`/`fromCron` returning `null` is for.
 *
 * The two functions are deliberately NOT inverses over all cron expressions —
 * only over the shapes below. `fromCron` returns `null` for anything it does
 * not recognise, and the caller must then fall back to the raw editor rather
 * than guess. That asymmetry is the point: a picker that silently rewrites an
 * expression it half-understood would change when someone's mail arrives.
 */

export type SchedulePeriod = "minutes" | "hourly" | "daily" | "weekly" | "monthly";

export interface StructuredSchedule {
  period: SchedulePeriod;
  /** `minutes`: the step. `hourly`: the step in hours. */
  every: number;
  /** Minute past the hour, for every period except `minutes`. */
  minute: number;
  /** Hour of day, for `daily` / `weekly` / `monthly`. */
  hour: number;
  /** 0 = Sunday … 6 = Saturday. `weekly` only. */
  weekday: number;
  /** Day of month, 1–28. `monthly` only — capped at 28 so a schedule cannot
   *  silently skip February. */
  day: number;
}

export const DEFAULT_SCHEDULE: StructuredSchedule = {
  period: "daily",
  every: 1,
  minute: 0,
  hour: 9,
  weekday: 1,
  day: 1,
};

export const WEEKDAYS = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
];

/** Structured → crontab. Total: every valid `StructuredSchedule` has one. */
export function toCron(s: StructuredSchedule): string {
  switch (s.period) {
    case "minutes":
      return `*/${clamp(s.every, 1, 59)} * * * *`;
    case "hourly":
      return s.every === 1
        ? `${clamp(s.minute, 0, 59)} * * * *`
        : `${clamp(s.minute, 0, 59)} */${clamp(s.every, 2, 23)} * * *`;
    case "daily":
      return `${clamp(s.minute, 0, 59)} ${clamp(s.hour, 0, 23)} * * *`;
    case "weekly":
      return `${clamp(s.minute, 0, 59)} ${clamp(s.hour, 0, 23)} * * ${clamp(s.weekday, 0, 6)}`;
    case "monthly":
      return `${clamp(s.minute, 0, 59)} ${clamp(s.hour, 0, 23)} ${clamp(s.day, 1, 28)} * *`;
    default: {
      // A period outside the union only arrives through a cast (a picker's
      // `onValueChange(v as ...)`). Name it here rather than return
      // `undefined` and fail later, unexplained, wherever the cron is read.
      const unknown: never = s.period;
      throw new Error(`Unknown schedule period: ${JSON.stringify(unknown)}`);
    }
  }
}

/**
 * Crontab → structured, or `null` when the expression is outside the
 * vocabulary above (a list, a range, a month restriction, a step in a field
 * that does not take one here).
 */
export function fromCron(expr: string): StructuredSchedule | null {
  const fields = expr.trim().split(/\s+/);
  if (fields.length !== 5) return null;
  const [minute, hour, dom, month, dow] = fields;
  if (month !== "*") return null;

  // Every N minutes: a step in the minute field, wildcards elsewhere.
  const step = minute.match(/^\*\/(\d+)$/);
  if (step && hour === "*" && dom === "*" && dow === "*") {
    const n = Number(step[1]);
    if (n < 1 || n > 59) return null;
    return { ...DEFAULT_SCHEDULE, period: "minutes", every: n };
  }

  const m = int(minute, 0, 59);
  if (m === null) return null;

  // Hourly: a fixed minute, with the hour either a wildcard or a step.
  if (dom === "*" && dow === "*") {
    if (hour === "*") return { ...DEFAULT_SCHEDULE, period: "hourly", every: 1, minute: m };
    const hourStep = hour.match(/^\*\/(\d+)$/);
    if (hourStep) {
      const n = Number(hourStep[1]);
      if (n < 2 || n > 23) return null;
      return { ...DEFAULT_SCHEDULE, period: "hourly", every: n, minute: m };
    }
  }

  const h = int(hour, 0, 23);
  if (h === null) return null;

  if (dom === "*" && dow === "*") {
    return { ...DEFAULT_SCHEDULE, period: "daily", minute: m, hour: h };
  }
  if (dom === "*") {
    const d = int(dow, 0, 6);
    if (d === null) return null;
    return { ...DEFAULT_SCHEDULE, period: "weekly", minute: m, hour: h, weekday: d };
  }
  if (dow === "*") {
    const d = int(dom, 1, 28);
    if (d === null) return null;
    return { ...DEFAULT_SCHEDULE, period: "monthly", minute: m, hour: h, day: d };
  }
  // Both a day-of-month AND a weekday: cron ORs them, which no dropdown here
  // can express honestly.
  return null;
}

function int(field: string, lo: number, hi: number): number | null {
  if (!/^\d+$/.test(field)) return null;
  const n = Number(field);
  return n >= lo && n <= hi ? n : null;
}

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, Math.round(n)));
}

/**
 * Timezone names for the picker, newest-browser first with a small fallback.
 *
 * `Intl.supportedValuesOf` is the only way to get the real IANA list without
 * shipping one, but it is absent in older engines and in some test
 * environments — hence the fallback, which is short on purpose: a stub list
 * pretending to be complete would be worse than an obviously partial one.
 */
export function timezones(): string[] {
  const intl = Intl as typeof Intl & { supportedValuesOf?: (k: string) => string[] };
  if (typeof intl.supportedValuesOf === "function") {
    try {
      return intl.supportedValuesOf("timeZone");
    } catch {
      /* fall through */
    }
  }
  return [
    "UTC",
    "Asia/Ho_Chi_Minh",
    "Asia/Bangkok",
    "Asia/Singapore",
    "Asia/Tokyo",
    "Europe/London",
    "Europe/Paris",
    "America/New_York",
    "America/Los_Angeles",
  ];
}

/** The viewer's own zone, used as the default for a new schedule. */
export function localTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}
