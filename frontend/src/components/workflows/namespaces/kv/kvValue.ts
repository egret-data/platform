import { JsonNumber, stringifyLossless, type LosslessJson } from "./losslessJson";

/**
 * THE KV FORM'S RULES (W7d spec §4.4), the server's own (W9c).
 *
 * `PUT …/kv/{key}` takes no type. Its `text/plain` body is read as ONE Ion
 * value and typed by what it is (`storage::kv::value::parse_api_body`); any
 * other body is stored whole as a STRING. So each type is sent as text the
 * server reads back as that type, and the form refuses, before sending, a
 * value that would be stored as another type or refused:
 *
 * | type     | body                         | refused here                                   |
 * |----------|------------------------------|------------------------------------------------|
 * | STRING   | `"…"` (a quoted string)      | text shaped like a duration (stored DURATION)  |
 * | NUMBER   | the digits as typed          | not a JSON number; over 1,000 digits           |
 * | BOOLEAN  | `true` / `false`             | anything else                                  |
 * | DATETIME | `2026-10-14T02:30:00.000Z`   | no value; a year outside 1-9999 in UTC         |
 * | DATE     | `2026-10-14`                 | not a real day of years 1-9999                 |
 * | DURATION | `"PT1H30M"` (quoted)         | outside Kestra's pattern (stored STRING)       |
 * | JSON     | the JSON text as typed       | malformed; a scalar (stored as its own type)   |
 *
 * A JSON value goes as typed, not re-serialised: `JSON.parse` rounds numbers
 * past 2^53, and the server keeps every digit. JSON text is Ion text.
 */
export const KV_TYPES = ["STRING", "NUMBER", "BOOLEAN", "DATETIME", "DATE", "DURATION", "JSON"] as const;
export type KvType = (typeof KV_TYPES)[number];

export const isKvType = (t: string): t is KvType => (KV_TYPES as readonly string[]).includes(t);

/** What each type holds, shown under the type picker (FE-FORM-3). */
export const KV_TYPE_HELP: Record<KvType, string> = {
  STRING: "Any text.",
  NUMBER: "An integer or a decimal, every digit kept: 42, -1.5, 6.02e23.",
  BOOLEAN: "true or false.",
  DATETIME: "A date and a time, entered in your time zone and stored in UTC.",
  DATE: "A calendar day.",
  DURATION: "An ISO-8601 duration of days, hours, minutes and seconds: PT1H30M, P1D.",
  JSON: "A JSON object or array.",
};

export type Encoded = { ok: true; body: string } | { ok: false; error: string };

const MAX_DIGITS = 1000;

/**
 * Kestra's DURATION_PATTERN as W9c reads it (`kestra_duration`): `P`, then
 * whole days, then `T` and at least one of whole hours, minutes and seconds,
 * with at least one part, and a total that fits a Java `long` of seconds.
 * A text it matches is stored as a DURATION, whatever the form called it.
 */
export function isKestraDuration(text: string): boolean {
  const m = /^P(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$/.exec(text);
  if (!m || text === "P" || text.endsWith("T")) return false;
  const [, d, h, min, s] = m;
  let total = 0n;
  for (const [digits, per] of [[d, 86400n], [h, 3600n], [min, 60n], [s, 1n]] as const) {
    if (digits !== undefined) total += BigInt(digits) * per;
  }
  return total <= 9223372036854775807n;
}

function encodeNumber(raw: string): Encoded {
  const text = raw.trim();
  const m = /^-?(\d+)(?:\.(\d+))?(?:[eE][+-]?(\d+))?$/.exec(text);
  if (!m || /^-?0\d/.test(text)) {
    return { ok: false, error: "Enter a number, such as 42, -1.5 or 6.02e23 (digits, an optional point and exponent; no spaces or separators)." };
  }
  const [, int, frac = "", exp = ""] = m;
  if (int.length + frac.length > MAX_DIGITS || exp.length > MAX_DIGITS) {
    return { ok: false, error: "A number has at most 1,000 digits. Shorten it, or store it as a STRING." };
  }
  return { ok: true, body: text };
}

/** Years 1-9999, as the server's DATETIME and DATE hold them. */
const inYearRange = (iso: string) => /^\d{4}-/.test(iso) && !iso.startsWith("0000-");

function encodeDatetime(local: string): Encoded {
  const at = local ? new Date(local) : null;
  if (!at || Number.isNaN(at.getTime()) || !/^\d{4,}-\d{2}-\d{2}T\d{2}:\d{2}/.test(local.replace(/^\+/, ""))) {
    return { ok: false, error: "Enter a date and a time." };
  }
  const iso = at.toISOString();
  if (!inYearRange(iso)) {
    return { ok: false, error: `This is ${iso} in UTC: a DATETIME holds the years 1 to 9999 in UTC. Choose another date.` };
  }
  return { ok: true, body: iso };
}

function encodeDate(text: string): Encoded {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
  const refused = { ok: false as const, error: "Enter a date of the years 1 to 9999, such as 2026-10-14." };
  if (!m) return refused;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const day = new Date(0);
  day.setUTCFullYear(y, mo - 1, d);
  if (y < 1 || day.getUTCFullYear() !== y || day.getUTCMonth() !== mo - 1 || day.getUTCDate() !== d) return refused;
  return { ok: true, body: text };
}

function encodeJson(raw: string): Encoded {
  const text = raw.trim();
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (e) {
    return { ok: false, error: `This is not valid JSON (${e instanceof Error ? e.message : "it could not be read"}). Fix it and save again.` };
  }
  if (parsed === null || typeof parsed !== "object") {
    return {
      ok: false,
      error: "A JSON value here is an object ({…}) or an array ([…]). For a single number, text, true or false, choose that type.",
    };
  }
  return { ok: true, body: text };
}

/** The `PUT` body for `text` entered as `type`, or why the form refuses it. */
export function encodeValue(type: KvType, text: string): Encoded {
  switch (type) {
    case "STRING":
      if (isKestraDuration(text)) {
        return {
          ok: false,
          error: `"${text}" reads as a duration, so egret would store it as a DURATION, not a STRING. Choose the DURATION type, or change the text.`,
        };
      }
      return { ok: true, body: JSON.stringify(text) };
    case "NUMBER":
      return encodeNumber(text);
    case "BOOLEAN":
      return text === "true" || text === "false" ? { ok: true, body: text } : { ok: false, error: "Choose true or false." };
    case "DATETIME":
      return encodeDatetime(text);
    case "DATE":
      return encodeDate(text);
    case "DURATION":
      return isKestraDuration(text)
        ? { ok: true, body: JSON.stringify(text) }
        : {
            ok: false,
            error: "Enter a duration such as PT1H30M or P1D: P, whole days, then T and whole hours, minutes or seconds.",
          };
    case "JSON":
      return encodeJson(text);
  }
}

const two = (n: number) => String(n).padStart(2, "0");

/** A stored DATETIME (UTC) as `datetime-local` text in the viewer's zone, milliseconds when it has any. */
function localDatetime(utc: string): string {
  const at = new Date(utc);
  if (Number.isNaN(at.getTime())) return "";
  const base = `${String(at.getFullYear()).padStart(4, "0")}-${two(at.getMonth() + 1)}-${two(at.getDate())}T${two(at.getHours())}:${two(at.getMinutes())}`;
  const s = at.getSeconds();
  const ms = at.getMilliseconds();
  if (ms) return `${base}:${two(s)}.${String(ms).padStart(3, "0")}`;
  return s ? `${base}:${two(s)}` : base;
}

/** A value as `GET …/kv/{key}` sent it (read by `parseLossless`), as the form's text for `type`. */
export function valueToText(type: KvType, value: LosslessJson): string {
  if (value === null) return "";
  if (type === "JSON") return stringifyLossless(value, 2);
  if (type === "DATETIME" && typeof value === "string") return localDatetime(value);
  if (value instanceof JsonNumber) return value.text;
  if (typeof value === "object") return stringifyLossless(value);
  return String(value);
}

/** A value for reading (the view drawer): JSON indented, a number with its digits. */
export function valueForDisplay(type: KvType, value: LosslessJson): string {
  if (value === null) return "null";
  if (type === "JSON" || typeof value === "object") return stringifyLossless(value, 2);
  return String(value);
}

/** The key rule (W9c `KvKey::parse`): `[a-zA-Z0-9][a-zA-Z0-9._-]*`, at most 350 bytes. */
export function checkKey(key: string): string | null {
  if (!key) return "Enter a key.";
  if (!/^[a-zA-Z0-9]/.test(key)) return "A key starts with a letter or a digit (A-Z, a-z, 0-9).";
  if (!/^[a-zA-Z0-9._-]*$/.test(key)) return "A key holds only letters, digits, dots, dashes and underscores.";
  if (key.length > 350) return `A key is at most 350 characters; this one has ${key.length}.`;
  return null;
}

/** The longest TTL the server takes: 365,250 days (W9c `MAX_TTL_DAYS`). */
const MAX_TTL_SECONDS = 365_250 * 86_400;

/**
 * A custom TTL, judged as the `ttl` header is (`parse_ttl`, `Ttl::Header`):
 * `java.time.Duration` text of days, hours, minutes and seconds, not
 * negative, at most 1,000 years. Zero is refused here although the server
 * takes it: the key would expire as it is written.
 */
export function checkTtl(text: string): string | null {
  const t = text.trim();
  if (t.startsWith("-")) return "A TTL cannot be negative. Enter how long the key lives, such as PT1H.";
  const n = "(\\d+(?:\\.\\d+)?)";
  const m = new RegExp(`^P(?:${n}D)?(?:T(?:${n}H)?(?:${n}M)?(?:${n}S)?)?$`, "i").exec(t);
  if (!m || !m.slice(1).some((x) => x !== undefined)) {
    return "Enter an ISO-8601 duration of days, hours, minutes or seconds, such as PT30M, PT1H or P7D.";
  }
  const [, d = "0", h = "0", min = "0", s = "0"] = m;
  const seconds = Number(d) * 86_400 + Number(h) * 3_600 + Number(min) * 60 + Number(s);
  if (seconds === 0) return "A TTL of zero expires the key at once. Choose Never, or a longer TTL.";
  if (seconds > MAX_TTL_SECONDS) return "A TTL is at most 365,250 days (1,000 years). Choose a shorter one, or Never.";
  return null;
}

/** The time left until `expiresAt`, as a TTL in whole seconds (rounded up); null once it has passed. */
export function remainingTtl(expiresAt: string, now: number = Date.now()): string | null {
  const ms = Date.parse(expiresAt) - now;
  if (!(ms > 0)) return null;
  return `PT${Math.ceil(ms / 1000)}S`;
}

/**
 * A description is sent in a header, which holds one line: no line break and
 * no control character (`fetch` refuses them, and strips the ends).
 */
export function checkDescription(text: string): string | null {
  // A tab is allowed; every other C0 control character and DEL is not.
  const control = [...text].some((c) => {
    const code = c.charCodeAt(0);
    return (code < 0x20 && c !== "\t") || code === 0x7f;
  });
  return control ? "A description is one line of text, without line breaks or control characters." : null;
}

/**
 * Header text as its UTF-8 bytes, one character per byte. A header carries
 * bytes (`fetch` refuses a character past U+00FF), and the server reads the
 * `description` header's bytes as UTF-8 (`header_text`), so a description in
 * any language reaches it intact.
 */
export function headerText(text: string): string {
  return Array.from(new TextEncoder().encode(text), (b) => String.fromCharCode(b)).join("");
}

/** The expression a flow reads the key with. */
export const kvExpression = (key: string) => `{{ kv('${key}') }}`;
