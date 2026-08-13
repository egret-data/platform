/**
 * Turn a 5-field crontab into a phrase a person can read, e.g. "Every day at
 * 09:00" or "Every Monday at 08:30" — for the small, unambiguous set of
 * shapes handled below. Anything else — a step in the hour field, a
 * day-of-month or month restriction, a weekday range, several values in the
 * minute or hour field — falls back to the RAW expression, verbatim.
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

  // Both restrictions this helper does not attempt: any day-of-month or
  // month field beyond a bare wildcard.
  if (dom !== "*" || month !== "*") return raw;

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

/**
 * A comma-separated list of single weekday numbers (0-7, both 0 and 7 mean
 * Sunday — the standard vixie-cron vocabulary). Returns the days as 0-6,
 * deduped and sorted, or `null` for anything else: a range ("1-5"), a step, a
 * name, an out-of-bounds number. Those shapes are exactly what this helper
 * refuses to guess at.
 */
function parseWeekdayList(field: string): number[] | null {
  const parts = field.split(",");
  const days = new Set<number>();
  for (const p of parts) {
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
