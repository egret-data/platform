import dayjs from "dayjs";
// `Q` (quarter) is not in dayjs's core token set. A pivot bucketed by quarter has to be able to
// render "Q2 2026" — the label Metabase shows — so the plugin is registered here, once, next to
// the only formatter that uses it.
import advancedFormat from "dayjs/plugin/advancedFormat";
import { isNumericType, isTemporalType, type ColumnFormat } from "./format-types";

dayjs.extend(advancedFormat);

function applySeparators(s: string, separators?: string): string {
  if (!separators || separators === ".,") return s;
  const [dec, thou] = separators;
  return s.replace(/,/g, " ").replace(/\./g, dec).replace(/ /g, thou ?? "");
}

/**
 * The stored `decimals`, made safe to hand to `Intl.NumberFormat` / `Number#toExponential`.
 *
 * BOTH APIS THROW OUTSIDE 0..100 — `RangeError: …FractionDigits value is out of range`, naming
 * whichever option the branch set first — and `formatNumber` passed the stored number straight in
 * on all three of its branches. That throw is not cosmetic: `baseOption` formats the widest axis
 * tick while measuring the gutter, so it fires during the OPTION BUILD, inside React's render, and
 * `ChartErrorBoundary` replaces the whole card with "Couldn't render this chart:
 * maximumFractionDigits value is out of range" — measured in the browser on a bar chart whose
 * revenue column was given -1. A chart SAVED that way is un-renderable on every load afterwards,
 * until someone edits the stored JSON.
 *
 * `FormatSection` now clamps what a user can type, and that is the fix for new edits — but it
 * cannot reach a chart already saved, a `column_settings` written by anything else, or a value the
 * API accepted (chart `params` validation checks the SHAPE of `column_settings`, not this field's
 * range). So the formatter refuses to be the thing that takes a card down, which is the same rule
 * the scatter's `sizeOf` states for the same reason: it runs per label on whatever the query and
 * the stored settings happen to hold, and anything thrown there is invisible and total.
 *
 * WHAT IT DOES NOT DO: round. Intl TRUNCATES a fractional count itself (`2.5` formats to 2 digits),
 * measured, and that is not a defect — so `Math.trunc` keeps that behaviour rather than quietly
 * changing what a legal-but-odd stored value renders as. `NaN` is out of range to Intl too and
 * lands on 0, the same answer as a negative.
 */
const MAX_FRACTION_DIGITS = 100;
function safeDecimals(fmt: ColumnFormat): number | undefined {
  if (typeof fmt.decimals !== "number") return undefined;
  if (!Number.isFinite(fmt.decimals)) return 0;
  return Math.min(MAX_FRACTION_DIGITS, Math.max(0, Math.trunc(fmt.decimals)));
}

function formatNumber(value: number, fmt: ColumnFormat): string {
  // Read ONCE, here, and every branch below uses this rather than `fmt.decimals` — the guard has to
  // hold on all three, and three separate calls is three chances for the next branch to forget.
  const decimals = safeDecimals(fmt);
  let n = value;
  if (typeof fmt.scale === "number") n *= fmt.scale;

  const neg = n < 0 && fmt.negative_in_parentheses;
  const abs = neg ? Math.abs(n) : n;

  let out: string;
  if (fmt.number_style === "scientific") {
    out = abs.toExponential(decimals ?? 2);
  } else if (fmt.compact) {
    const opts: Intl.NumberFormatOptions = {
      notation: "compact",
      maximumFractionDigits: decimals ?? 1,
    };
    // Compacting shortens the number; it should not also strip what the number MEANS.
    if (fmt.number_style === "currency") {
      opts.style = "currency";
      opts.currency = fmt.currency ?? "USD";
      opts.currencyDisplay = fmt.currency_style ?? "symbol";
    } else if (fmt.number_style === "percent") {
      opts.style = "percent";
    }
    out = new Intl.NumberFormat("en", opts).format(abs);
  } else {
    const style = fmt.number_style ?? "decimal";
    const opts: Intl.NumberFormatOptions = { useGrouping: true };
    if (style === "currency") {
      opts.style = "currency";
      opts.currency = fmt.currency ?? "USD";
      opts.currencyDisplay = fmt.currency_style ?? "symbol";
    } else if (style === "percent") {
      opts.style = "percent";
    }
    if (decimals != null) {
      opts.minimumFractionDigits = decimals;
      opts.maximumFractionDigits = decimals;
    } else {
      opts.maximumFractionDigits = style === "currency" ? 2 : 3;
    }
    out = new Intl.NumberFormat("en", opts).format(abs);
  }

  out = applySeparators(out, fmt.number_separators);
  if (neg) out = `(${out})`;
  return `${fmt.prefix ?? ""}${out}${fmt.suffix ?? ""}`;
}

function formatDate(value: unknown, fmt: ColumnFormat): string {
  const d = dayjs(value as string | number | Date);
  if (!d.isValid()) return String(value);
  let dateFmt = fmt.date_style ?? "MMMM D, YYYY";
  if (fmt.date_separator) dateFmt = dateFmt.replace(/\//g, fmt.date_separator);
  if (fmt.date_abbreviate) dateFmt = dateFmt.replace(/MMMM/g, "MMM").replace(/dddd/g, "ddd");
  let timeFmt = "";
  if (fmt.time_enabled) {
    timeFmt = " " + (fmt.time_style ?? "h:mm A");
    if (fmt.time_enabled === "seconds") timeFmt = timeFmt.replace(/mm/, "mm:ss");
    else if (fmt.time_enabled === "milliseconds") timeFmt = timeFmt.replace(/mm/, "mm:ss.SSS");
  }
  return `${fmt.prefix ?? ""}${d.format(dateFmt + timeFmt)}${fmt.suffix ?? ""}`;
}

/**
 * Which `ColumnFormat` fields `formatDate` above actually consults — the seven it reads, and
 * `false` for every other field the type carries.
 *
 * WHY THIS EXISTS. Two callers decide between a stored format and a computed label by asking "did
 * the user set a format for this column?", and both asked it as `if (fmt)` — object truthiness.
 * That is wrong, because the settings panel mints format objects that are SILENT ABOUT DATES:
 * `FormatSection`'s `patch` spreads into `{ ...fmt, ...p }` (`settings/FormatSection.tsx:67`) and
 * `styleColumnsFor` offers the x dimension to it for every cartesian viz and falls through to
 * every column for a smartscalar (`settings/SharedDisplaySections.tsx:27-38`, `:52`). So renaming
 * a month-bucketed date column to "Month" stores `{ column_title: "Month" }` — truthy — and the
 * axis silently reverted from "January 2026" to `formatDate`'s default "January 1, 2026" with
 * nothing in the panel to explain it. Typing a title and deleting it leaves
 * `{ column_title: undefined }`, which is a truthy object too.
 *
 * A `satisfies Record<keyof ColumnFormat, boolean>` ON PURPOSE. This list has to track
 * `formatDate`'s body, and the drift that would break it is a field ADDED to `ColumnFormat`, at
 * which point a hand-written array of seven names goes quietly stale. This makes `tsc` demand a
 * yes/no for the new field instead of defaulting it to "does not matter".
 */
const CONSULTED_BY_FORMAT_DATE = {
  date_style: true,
  date_separator: true,
  date_abbreviate: true,
  time_enabled: true,
  time_style: true,
  prefix: true,
  suffix: true,
  // Read only by `formatNumber`; a temporal column never reaches it.
  number_style: false, currency: false, currency_style: false, decimals: false, scale: false,
  number_separators: false, compact: false, negative_in_parentheses: false,
  // Read by nothing in this file. `column_title` names the COLUMN, not its values; the rest are
  // how a table CELL is decorated (`ChartTable`), which no chart label goes through.
  column_title: false, view_as: false, link_text: false, link_url: false, show_mini_bar: false,
} satisfies Record<keyof ColumnFormat, boolean>;

/**
 * Whether this format says anything about how a DATE renders — i.e. whether `formatDate` would
 * produce anything other than its defaults from it.
 *
 * Truthiness per field, which is how `formatDate` itself reads six of the seven: `date_abbreviate:
 * false` and `time_enabled: null` are the same instruction as saying nothing, and that is exactly
 * what both mean in the panel — a switch toggled on and back off. The seventh, `date_style`, is
 * read there with `??`, so a stored empty string would format as "" upstream while reading as
 * silent here; unreachable from the panel, whose control is a fixed select, and recorded rather
 * than guarded so this stays one line.
 */
export function specifiesDateRendering(fmt: ColumnFormat | undefined): boolean {
  if (fmt == null) return false;
  return Object.entries(CONSULTED_BY_FORMAT_DATE).some(
    ([field, consulted]) => consulted && Boolean(fmt[field as keyof ColumnFormat]),
  );
}

export function formatValue(value: unknown, columnType: string, fmt: ColumnFormat = {}): string {
  if (value == null) return "";
  if (isTemporalType(columnType)) return formatDate(value, fmt);
  if (isNumericType(columnType) || fmt.number_style) {
    const n = typeof value === "number" ? value : Number(value);
    if (Number.isFinite(n) && !(typeof value === "string" && value.trim() === "")) {
      return formatNumber(n, fmt);
    }
  }
  const s = String(value);
  return fmt.prefix || fmt.suffix ? `${fmt.prefix ?? ""}${s}${fmt.suffix ?? ""}` : s;
}
