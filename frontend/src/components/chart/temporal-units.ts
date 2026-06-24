export type TemporalGroup = "trunc" | "extract";
export interface TemporalUnitDef {
  token: string;
  label: string;
  group: TemporalGroup;
}

/** The Metabase temporal-unit set. Tokens MUST match the backend TemporalUnit::parse (sub-project A). */
export const TEMPORAL_UNITS: TemporalUnitDef[] = [
  { token: "minute", label: "Minute", group: "trunc" },
  { token: "hour", label: "Hour", group: "trunc" },
  { token: "day", label: "Day", group: "trunc" },
  { token: "week", label: "Week", group: "trunc" },
  { token: "month", label: "Month", group: "trunc" },
  { token: "quarter", label: "Quarter", group: "trunc" },
  { token: "year", label: "Year", group: "trunc" },
  { token: "minute_of_hour", label: "Minute of hour", group: "extract" },
  { token: "hour_of_day", label: "Hour of day", group: "extract" },
  { token: "day_of_week", label: "Day of week", group: "extract" },
  { token: "day_of_month", label: "Day of month", group: "extract" },
  { token: "day_of_year", label: "Day of year", group: "extract" },
  { token: "week_of_year", label: "Week of year", group: "extract" },
  { token: "month_of_year", label: "Month of year", group: "extract" },
  { token: "quarter_of_year", label: "Quarter of year", group: "extract" },
];

const EXTRACT_TOKENS = new Set(
  TEMPORAL_UNITS.filter((u) => u.group === "extract").map((u) => u.token),
);

export function isExtractUnit(token: string | undefined): boolean {
  return token != null && EXTRACT_TOKENS.has(token);
}

const DOW = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MON = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** Display label for a (possibly extraction-grouped) dimension value. Total: never throws;
 *  unknown tokens, truncation, and out-of-range values fall back to String(value). */
export function temporalLabel(token: string | undefined, value: unknown): string {
  const n = typeof value === "number" ? value : Number(value);
  if (token === "day_of_week" && Number.isInteger(n) && n >= 1 && n <= 7) return DOW[n - 1];
  if (token === "month_of_year" && Number.isInteger(n) && n >= 1 && n <= 12) return MON[n - 1];
  if (token === "quarter_of_year" && Number.isInteger(n) && n >= 1 && n <= 4) return `Q${n}`;
  return String(value);
}

/** Keep only the units whose column is in `dims` — the backend rejects a temporal_unit keyed to a
 *  column the query does not select (orphan key -> BadRequest). */
export function pruneUnits(
  units: Record<string, string>,
  dims: string[],
): Record<string, string> {
  return Object.fromEntries(Object.entries(units).filter(([k]) => dims.includes(k)));
}
