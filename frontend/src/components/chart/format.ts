import dayjs from "dayjs";
// `Q` (quarter) is not in dayjs's core token set. A pivot bucketed by quarter has to be able to
// render "Q2 2026" — the label Metabase shows — so the plugin is registered here, once, next to
// the only formatter that uses it.
import advancedFormat from "dayjs/plugin/advancedFormat";
import { isNumericType, isTemporalType, type ColumnFormat } from "./format-types";

dayjs.extend(advancedFormat);

/**
 * Re-point the `en`-formatted string's `.` and `,` at the separators the column asked for.
 *
 * `formatNumber` below always builds with `new Intl.NumberFormat("en", …)` precisely so that the
 * two separators are KNOWN characters this can rewrite — the same trick, and the same comment, as
 * Metabase's `numberFormatterForOptions` ("always use 'en' locale so we have known number
 * separators we can replace"). The stored value is Metabase's two-character
 * `"[decimal][thousands]"`; a ONE-character value means "no thousands separator at all", which is
 * how `100000.00` is expressed and why `thou` is allowed to be undefined.
 *
 * ONE PASS, WITH NO SCRATCH MARKER — and that is the whole point of this function's shape. It used
 * to run three passes using a SPACE as the intermediate (`,`→" ", `.`→dec, " "→thou), which is
 * correct only while no space in the string means anything. `currencyDisplay: "name"` puts real
 * ASCII spaces in — `"100,000.00 US dollars"` — so under the `100.000,00` option a live bar chart's
 * y-axis drew `100.000,00.US.dollars`, measured in the browser on `MB By Category` with Display set
 * to "dollars". The two styles added in this PR would each have extended the same defect in their
 * own way: `"."` maps the marker to the empty string (`100000.00USdollars`) and `".’"` maps it to
 * the apostrophe (`100’000.00’US’dollars`).
 *
 * A per-character map cannot have that failure mode, because no output character is ever fed back
 * in as an input. This is character-for-character Metabase's `replaceNumberSeparators`
 * (`frontend/src/metabase/utils/formatting/numbers.tsx`), including `?? ""` for the one-character
 * case; copying it rather than inventing a fourth spelling is deliberate, since the two functions
 * have to agree about what a stored `number_separators` means for a chart exported from either app.
 *
 * `currencyDisplay: "code"` was never affected: ICU separates the code from the number with U+00A0,
 * not U+0020, so the old space-marker pass walked straight past it. Recorded because it is the
 * reason the defect looked narrower than it was and only ever showed up under one Display option.
 */
function applySeparators(s: string, separators?: string): string {
  if (!separators || separators === ".,") return s;
  const [dec, thou] = separators;
  return s.replace(/[.,]/g, (ch) => (ch === "," ? thou ?? "" : dec));
}

/**
 * The stored `decimals`, made safe to hand to `Intl.NumberFormat` / `Number#toExponential`.
 *
 * BOTH APIS THROW OUT OF RANGE — `RangeError: …FractionDigits value is out of range`, naming
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
 *
 * THE CEILING IS OURS, NOT INTL'S — and it is 20, not 100. This clamp used to be 100 because that
 * is what the local engine accepted, and it is not a portable number: ECMA-402 specified
 * `maximum/minimumFractionDigits` as 0..20 until the ES2023 amendment raised it to 0..100, so the
 * ceiling is a property of whatever V8/JSC/SpiderMonkey the page is running on. Measured, on the
 * same probe (`new Intl.NumberFormat("en", {min,max: d})`, largest `d` that does not throw):
 *
 *   Node 18 / V8 10.2  → 20      Node 20 / V8 11.3  → 20
 *   Node 22 / V8 12.4  → 100     Node 24 / V8 13.6  → 100
 *
 * So a chart with `decimals: 50` rendered on a new engine and threw `RangeError` on an older one —
 * the SAME total card loss this function exists to prevent, just moved from "a bad stored value" to
 * "a bad browser". Clamping to a ceiling this module owns is what makes the output a function of
 * the stored settings alone. 20 rather than 100 because it is the value every engine can represent:
 * clamping to the lower bound renders everywhere, clamping to the higher one only renders on the
 * engines that have caught up. Nothing real is lost — `FormatSection` offers 0..10, and no axis
 * label is legible past 20 fraction digits anyway.
 *
 * `Number#toExponential` is 0..100 on all four engines above (it has been since ES2018) and so is
 * not the constraint here; one constant covering all three branches is still the point, because
 * three per-branch guards is three chances for the next branch to forget.
 */
const MAX_FRACTION_DIGITS = 20;
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

/**
 * Whether a column holds IDENTIFIERS — `orderkey`, `customer_id` — rather than quantities (KI-058).
 *
 * Recognised by two facts together: an INTEGER type (an identifier is never fractional, and a
 * `double` named `…key` is a measure) and a name that says identifier — `id`, `…_id`, camel-case
 * `…Id`, or `…key` (TPC-H's `orderkey`/`custkey`, and `…_key`). egret has no identifier semantic
 * type, and adding one would leave every existing dataset unrecognised until someone set it, so
 * the name decides. A wrong guess costs only the thousands separator, and the user's own
 * Separator style (on the chart or in the dataset) overrides it either way.
 */
const INTEGER_TYPE_RE = /^(tinyint|smallint|int|integer|bigint|largeint)\b/i;
export function isIdentifierColumn(name: string, columnType: string): boolean {
  if (!INTEGER_TYPE_RE.test(columnType.trim())) return false;
  return /^id$|_id$|key$/i.test(name) || /[a-z0-9]Id$/.test(name);
}

/**
 * The format an identifier column is drawn with: no thousands separator (`29989`, not `29,989`)
 * unless a separator style was chosen for it. Any other column's format is returned unchanged.
 */
export function withIdentifierDefault(name: string, columnType: string, fmt?: ColumnFormat): ColumnFormat | undefined {
  if (fmt?.number_separators != null || !isIdentifierColumn(name, columnType)) return fmt;
  return { ...fmt, number_separators: "." };
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

/**
 * A DIMENSION CELL AS THE TEXT A CHART DRAWS FOR IT — the format when the column has one, the raw
 * value when it does not.
 *
 * WHY THIS EXISTS AS ONE FUNCTION. `styleColumnsFor` fell through to `return resultColumns` for the
 * funnel, the box plot, the sankey and the map, so all four handed their DIMENSION a Formatting
 * accordion while their builders read `columnSettings` for the metric alone — a Date style typed
 * into the panel, stored in the chart, and drawn nowhere. Measured in the browser on all three that
 * draw a label. Metabase formats every one of them (`FunnelNormal`'s `formatDimension`,
 * `echarts/boxplot/model`'s `createXAxisFormatter`, `sankey/model/formatters`' `node`), so the fix
 * is to read the format, not to withdraw the control — except on the map, whose region name is a
 * geojson matching key rather than a label and which therefore narrows instead
 * (`settings/SharedDisplaySections.tsx`).
 *
 * Three builders needed the same rule at once, and the rule has a trap in it, so it is written once
 * here rather than three times there.
 *
 * THE TRAP, which `CONSULTED_BY_FORMAT_DATE` above exists for and which has cost this codebase an
 * axis label once already: `FormatSection`'s `patch` spreads into `{ ...fmt, ...p }`, so merely
 * RENAMING a column stores `{ column_title: "Month" }` — a truthy object that says nothing about
 * dates. Handing that to `formatValue` on a temporal column takes `formatDate`'s DEFAULT style and
 * silently turns "2026-01-01" into "January 1, 2026". A title typed and then deleted leaves
 * `{ column_title: undefined }`, which is truthy too. So on a temporal column the format has to
 * SPECIFY date rendering before it wins; `if (fmt)` is the wrong question and was asked twice
 * before anyone noticed.
 *
 * ON A NON-TEMPORAL COLUMN ANY STORED FORMAT WINS, which is the same shape `waterfall.ts`'s
 * `drawLabel` already uses and is deliberately not guarded further. One consequence worth stating:
 * a NUMERIC dimension carrying only a `column_title` now renders through `formatNumber`, so "1234"
 * becomes "1,234". That is what Metabase draws for the same column — it formats the dimension
 * unconditionally — and it is reachable only on a column the reader has already opened and edited.
 * Narrowing it further would need a second `satisfies`-checked field list for numbers and text, and
 * a second mechanism for "does this format say anything" is how the two would come to disagree.
 *
 * WHAT IT IS NOT: a bucketing rule. `periodName` / `namesAPeriod` turn a month bucket into
 * "January 2026" and belong to the charts that already carry a temporal unit (the cartesian, the
 * waterfall, the smart scalar); none of the three callers here has ever named a period, so adding
 * one would restyle every existing funnel, box plot and sankey rather than close the defect this is
 * for. `waterfall.ts`'s `drawLabel` is the shape to copy when one of them grows a unit.
 */
export function formatDimensionValue(
  value: unknown,
  columnName: string,
  columnType: string,
  stored: ColumnFormat | undefined,
): string {
  // An identifier keeps no thousands separator unless one was chosen (KI-058), here as in a table
  // cell: without this, a Column title alone sent `orderkey` through `formatNumber` as `29,989`
  // (KI-294). Only on a stored format: with none, the raw value below is already separator-free.
  const fmt = stored && withIdentifierDefault(columnName, columnType, stored);
  const formatWins = fmt != null && (isTemporalType(columnType) ? specifiesDateRendering(fmt) : true);
  if (formatWins) {
    const out = formatValue(value, columnType, fmt);
    // An empty result is not a label. `formatValue` returns "" for a null cell, and so does the
    // fallback below — but a format that produces "" from a real value would blank a step name or
    // an axis tick, which reads as missing data rather than as a formatting choice.
    if (out !== "") return out;
  }
  return String(value ?? "");
}
