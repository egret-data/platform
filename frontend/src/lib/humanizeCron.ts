/**
 * Turn a 5-field crontab into a phrase a person can read, e.g. "Every day at
 * 09:00", "Every Monday to Friday at 08:30" or "Every month on day 1 at
 * 09:00" — for the small, unambiguous set of shapes handled below. It is the
 * ONE formatter for a schedule in the Alerts UI: the list, the detail header
 * and the editor's preview all read through it (KI-038), and it covers every
 * shape the editor's picker (`cronSchedule.ts`) can write. Anything else —
 * several values in the minute or hour field, a day-of-month crossed with a
 * weekday, a range in the day-of-month field — falls back to the RAW
 * expression, verbatim.
 *
 * Why so narrow: a wrong humanisation is worse than none, because this text
 * tells someone when their mail arrives. This function has no relationship
 * to `croner` (the parser that actually drives scheduling, in Rust) — it
 * must never claim to understand a shape it has not been proven against
 * here. When in doubt, return the expression the user actually wrote; the
 * caller should also carry it in a `title` tooltip, so the raw form is one
 * hover away regardless of which branch this returns.
 */
export function humanizeCrontab(expr: string): string {
  const raw = expr.trim();
  const fields = raw.split(/\s+/);
  if (fields.length !== 5) return raw;
  const [minute, hour, dom, month, dow] = fields;

  // A single day of the month (and, for a yearly schedule, a single month)
  // with a bare weekday. A day-of-month crossed with a weekday is a shape
  // cron ORs, which no phrase here says honestly.
  if (dom !== "*" || month !== "*") {
    const m = singleInt(minute, 0, 59);
    const h = singleInt(hour, 0, 23);
    const d = singleInt(dom, 1, 31);
    if (m === null || h === null || d === null || dow !== "*") return raw;
    const time = `${pad(h)}:${pad(m)}`;
    if (month === "*") return `Every month on day ${d} at ${time}`;
    const mo = singleInt(month, 1, 12);
    if (mo === null) return raw;
    return `Every year on ${d} ${MONTH_NAMES[mo - 1]} at ${time}`;
  }

  if (minute === "*" && hour === "*" && dow === "*") return "Every minute";

  const everyNMinutes = minute.match(/^\*\/(\d+)$/);
  if (everyNMinutes && hour === "*" && dow === "*") {
    const n = Number(everyNMinutes[1]);
    if (n < 1 || n > 59) return raw;
    return n === 1 ? "Every minute" : `Every ${n} minutes`;
  }

  // "Every N hours, at minute M" — `15 */6 * * *` fires at 00:15, 06:15,
  // 12:15, 18:15. Added because this shape is in the shipped seed data and
  // showed as a raw expression in the page header, where the reader has no
  // other statement of when the thing runs. Restricted to a bare weekday
  // wildcard: `*/6` hours crossed with a weekday list is a shape this has not
  // been proven against.
  // A bare hour wildcard is the picker's "Hourly": `0 * * * *` fires on the
  // hour, every hour — the seeded "Cancelled orders spike" (KI-038).
  if (hour === "*" && dow === "*") {
    const at = singleInt(minute, 0, 59);
    return at === null ? raw : `Every hour at minute ${at}`;
  }

  const everyNHours = hour.match(/^\*\/(\d+)$/);
  if (everyNHours && dow === "*") {
    const n = Number(everyNHours[1]);
    const at = singleInt(minute, 0, 59);
    if (n < 1 || n > 23 || at === null) return raw;
    return n === 1
      ? `Every hour at minute ${at}`
      : `Every ${n} hours, at minute ${at}`;
  }

  // Everything below needs a single numeric minute and hour — the "daily"
  // and "weekly" forms.
  const m = singleInt(minute, 0, 59);
  const h = singleInt(hour, 0, 23);
  if (m === null || h === null) return raw;
  const time = `${pad(h)}:${pad(m)}`;

  if (dow === "*") return `Every day at ${time}`;

  // One ascending range reads as a span ("Monday to Friday"); anything
  // longer is spelled out day by day.
  const span = dow.match(/^([0-6])-([0-6])$/);
  if (span && Number(span[1]) < Number(span[2])) {
    return `Every ${WEEKDAY_NAMES[Number(span[1])]} to ${WEEKDAY_NAMES[Number(span[2])]} at ${time}`;
  }
  const days = parseWeekdayList(dow);
  if (days === null) return raw;
  return `Every ${formatDayList(days)} at ${time}`;
}

function singleInt(field: string, min: number, max: number): number | null {
  if (!/^\d+$/.test(field)) return null;
  const n = Number(field);
  return n >= min && n <= max ? n : null;
}

function pad(n: number): string {
  return n.toString().padStart(2, "0");
}

const WEEKDAY_NAMES = [
  "Sunday",
  "Monday",
  "Tuesday",
  "Wednesday",
  "Thursday",
  "Friday",
  "Saturday",
];

const MONTH_NAMES = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

/**
 * A comma-separated list of single weekday numbers (0-7, both 0 and 7 mean
 * Sunday — the standard vixie-cron vocabulary) and ascending ranges within
 * 0-6 ("1-3"). Returns the days as 0-6, deduped and sorted, or `null` for
 * anything else: a range that runs backwards or reaches 7, a step, a name, an
 * out-of-bounds number. Those shapes are exactly what this helper refuses to
 * guess at.
 */
function parseWeekdayList(field: string): number[] | null {
  const parts = field.split(",");
  const days = new Set<number>();
  for (const p of parts) {
    const range = p.match(/^([0-6])-([0-6])$/);
    if (range) {
      const [lo, hi] = [Number(range[1]), Number(range[2])];
      if (lo >= hi) return null;
      for (let d = lo; d <= hi; d++) days.add(d);
      continue;
    }
    if (!/^\d+$/.test(p)) return null;
    const n = Number(p);
    if (n < 0 || n > 7) return null;
    days.add(n === 7 ? 0 : n);
  }
  if (days.size === 0) return null;
  return [...days].sort((a, b) => a - b);
}

function formatDayList(days: number[]): string {
  const names = days.map((d) => WEEKDAY_NAMES[d]);
  if (names.length === 1) return names[0];
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  return `${names.slice(0, -1).join(", ")}, and ${names[names.length - 1]}`;
}
