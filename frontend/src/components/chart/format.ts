import dayjs from "dayjs";
import { isNumericType, isTemporalType, type ColumnFormat } from "./format-types";

function applySeparators(s: string, separators?: string): string {
  if (!separators || separators === ".,") return s;
  const [dec, thou] = separators;
  return s.replace(/,/g, " ").replace(/\./g, dec).replace(/ /g, thou ?? "");
}

function formatNumber(value: number, fmt: ColumnFormat): string {
  let n = value;
  if (typeof fmt.scale === "number") n *= fmt.scale;

  const neg = n < 0 && fmt.negative_in_parentheses;
  const abs = neg ? Math.abs(n) : n;

  let out: string;
  if (fmt.number_style === "scientific") {
    out = abs.toExponential(fmt.decimals ?? 2);
  } else if (fmt.compact) {
    out = new Intl.NumberFormat("en", {
      notation: "compact",
      maximumFractionDigits: fmt.decimals ?? 1,
    }).format(abs);
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
    if (typeof fmt.decimals === "number") {
      opts.minimumFractionDigits = fmt.decimals;
      opts.maximumFractionDigits = fmt.decimals;
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

// Singleton to avoid allocating Intl.NumberFormat on every axis tick.
const compactFormatter = new Intl.NumberFormat("en", {
  notation: "compact",
  maximumFractionDigits: 1,
});

/**
 * A short axis label: 25,468 → "25.5K". Used where the axis is too narrow for the full
 * number; the tooltip and data labels keep the exact value. Compact notation and separators
 * are independent: the notation decides HOW SHORT the label is, and separators decide WHICH
 * CHARACTERS separate the digits. So compactNumber(1234.5, ", ") → "1,2K" (compacted with
 * European decimal separator).
 */
export function compactNumber(v: number, separators?: string): string {
  if (!Number.isFinite(v)) return "";
  if (v === 0) return "0";

  const out = compactFormatter.format(v);
  return applySeparators(out, separators);
}
