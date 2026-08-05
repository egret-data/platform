export type NumberStyle = "decimal" | "percent" | "currency" | "scientific";

export interface ColumnFormat {
  number_style?: NumberStyle;
  currency?: string;
  currency_style?: "symbol" | "code" | "name" | "narrowSymbol";
  decimals?: number;
  scale?: number;
  prefix?: string;
  suffix?: string;
  number_separators?: string; // "[decimal][thousands]", default ".,"
  compact?: boolean;
  negative_in_parentheses?: boolean;
  date_style?: string;
  date_separator?: "/" | "-" | ".";
  date_abbreviate?: boolean;
  time_enabled?: "minutes" | "seconds" | "milliseconds" | null;
  time_style?: "h:mm A" | "HH:mm";
  column_title?: string;
  // "Display as" — render the cell value as a link / email / image instead of plain text.
  view_as?: "auto" | "text" | "link" | "email" | "image";
  link_text?: string;  // link only: display-text template; {{value}} → the cell value
  link_url?: string;   // link only: href template; {{value}} → the cell value (defaults to the value)
  show_mini_bar?: boolean;  // numeric: draw an in-cell bar sized to the column's extent
}

export type FormattingOperator =
  | "=" | "!=" | "<" | ">" | "<=" | ">="
  | "contains" | "does-not-contain" | "starts-with" | "ends-with"
  | "is-true" | "is-false" | "is-null" | "not-null";

export interface SingleColorRule {
  columns: string[];
  type: "single";
  operator: FormattingOperator;
  value?: string | number;
  color: string;
  highlight_row?: boolean;
}
export interface ColorRangeRule {
  columns: string[];
  type: "range";
  colors: string[];
  min_type: "custom" | "all" | null;
  max_type: "custom" | "all" | null;
  min_value?: number;
  max_value?: number;
}
export type Rule = SingleColorRule | ColorRangeRule;

// Covers SQL type names (Trino: bigint, double…) and MySQL-protocol enum names
// returned by StarRocks (MYSQL_TYPE_LONGLONG / LONG / SHORT / TINY / NEWDECIMAL…).
const NUMERIC_RE = /int|double|decimal|real|numeric|float|bigint|long|short|tiny/i;
const TEMPORAL_RE = /date|time|timestamp/i;

export function isNumericType(type: string): boolean {
  return NUMERIC_RE.test(type);
}
export function isTemporalType(type: string): boolean {
  return TEMPORAL_RE.test(type) && !NUMERIC_RE.test(type);
}

/**
 * A boolean column, as the ENGINE names it.
 *
 * Anchored (`^…$`) rather than the loose `.test()` the two above use, and that is the whole design:
 * `isNumericType`'s pattern is a substring match, so an unanchored `bool` here would be a second
 * loose matcher racing the first over names neither of them was written for. There is exactly one
 * spelling to recognise — Trino reports `boolean` and `ColumnMeta.ty` passes the engine's own type
 * name straight through — so this recognises that and nothing else.
 *
 * DELIBERATELY NOT USED BY `formatValue`. A boolean is neither numeric nor temporal there, and it
 * falls out of the bottom as `String(value)` — "true"/"false" — which is what it should read as.
 * This exists for the conditional-formatting operator list, which is the one place the DISTINCTION
 * between a boolean and a string changes what the panel may offer.
 *
 * StarRocks is a known asymmetry and not one this can close: its connector reports the MySQL
 * protocol type, and `BOOLEAN` and `TINYINT` are the same wire type there (`MYSQL_TYPE_TINY`), so a
 * StarRocks boolean arrives numeric-looking and `isNumericType` claims it first. `typegen.rs`
 * records the same limitation on the backend side.
 */
const BOOLEAN_RE = /^bool(ean)?$/i;
export function isBooleanType(type: string): boolean {
  return BOOLEAN_RE.test(type.trim());
}
