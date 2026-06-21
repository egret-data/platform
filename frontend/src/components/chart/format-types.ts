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
