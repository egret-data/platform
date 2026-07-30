import type { ChartFilter } from "@/api/charts";

export interface DrillDim {
  col: string;
  value: unknown;
  /** The temporal unit this dimension was bucketed by, when it was. A bucketed value stands for a
   * RANGE, not a point — see `drillFilters`. */
  unit?: string;
}
export interface DrillContext { dims: DrillDim[] }

/** Pivot value cell → the dims of its (rowPath, colPath), skipping null/short segments. */
export function pivotCellDrill(
  rowDims: string[], rowPath: (string | null)[],
  colDims: string[], colPath: (string | null)[],
  temporalUnits: Record<string, string> = {},
): DrillContext {
  const dims: DrillDim[] = [];
  const push = (c: string, v: unknown) => dims.push(temporalUnits[c] ? { col: c, value: v, unit: temporalUnits[c] } : { col: c, value: v });
  rowDims.forEach((c, i) => { if (i < rowPath.length && rowPath[i] != null) push(c, rowPath[i]); });
  colDims.forEach((c, i) => { if (i < colPath.length && colPath[i] != null) push(c, colPath[i]); });
  return { dims };
}

/** Aggregated-table value cell → the dims (dimension columns) of its row. */
export function tableCellDrill(
  dimensionColumns: string[], columns: string[], row: unknown[],
  temporalUnits: Record<string, string> = {},
): DrillContext {
  const dims: DrillDim[] = [];
  for (const c of dimensionColumns) {
    const i = columns.indexOf(c);
    if (i >= 0 && row[i] != null) {
      dims.push(temporalUnits[c] ? { col: c, value: row[i], unit: temporalUnits[c] } : { col: c, value: row[i] });
    }
  }
  return { dims };
}

/** How far one bucket runs, for the truncation units. Extraction units (`day_of_week`, …) are
 * absent on purpose: they yield an integer field, not a point in time, so there is no range. */
const BUCKET_ADVANCE: Record<string, (d: Date) => void> = {
  minute: (d) => d.setUTCMinutes(d.getUTCMinutes() + 1),
  hour: (d) => d.setUTCHours(d.getUTCHours() + 1),
  day: (d) => d.setUTCDate(d.getUTCDate() + 1),
  week: (d) => d.setUTCDate(d.getUTCDate() + 7),
  month: (d) => d.setUTCMonth(d.getUTCMonth() + 1),
  quarter: (d) => d.setUTCMonth(d.getUTCMonth() + 3),
  year: (d) => d.setUTCFullYear(d.getUTCFullYear() + 1),
};

/** `2026-01-01T00:00:00.000Z` → `2026-01-01`, keeping the time part only when there is one. */
function toLiteral(d: Date, hasTime: boolean): string {
  const iso = d.toISOString();
  return hasTime ? iso.slice(0, 19).replace("T", " ") : iso.slice(0, 10);
}

/**
 * Build the filters a drill should apply.
 *
 * A plain dimension is an equality. A **bucketed** one is a half-open range: the cell under a
 * month column holds the whole month, so `= 2026-01-01` answers a different question than the cell
 * did. That mismatch was silent and understated the result — a cell reading 2,084 drilled to a
 * single row of 681, with nothing on screen to say the other 1,403 had been filtered away. Wrong
 * numbers with no error are worse than an error.
 *
 * An extraction unit (`day_of_week` and friends) has no range to build — its value is an integer
 * field, not an instant — so that dimension contributes no filter at all. The result is then
 * BROADER than the cell rather than narrower, which shows more rows than asked for instead of
 * hiding some.
 */
export function drillFilters(ctx: DrillContext): ChartFilter[] {
  const out: ChartFilter[] = [];
  for (const d of ctx.dims) {
    if (!d.unit) {
      out.push({ col: d.col, op: "eq", value: d.value });
      continue;
    }
    const advance = BUCKET_ADVANCE[d.unit];
    if (!advance) continue; // extraction unit — no range exists; widen rather than mislead
    const start = new Date(String(d.value).includes("T") || String(d.value).includes(" ")
      ? String(d.value).replace(" ", "T") + (String(d.value).endsWith("Z") ? "" : "Z")
      : `${String(d.value)}T00:00:00Z`);
    if (Number.isNaN(start.getTime())) {
      out.push({ col: d.col, op: "eq", value: d.value });
      continue;
    }
    const end = new Date(start);
    advance(end);
    const hasTime = d.unit === "minute" || d.unit === "hour";
    out.push({ col: d.col, op: "ge", value: toLiteral(start, hasTime) });
    out.push({ col: d.col, op: "lt", value: toLiteral(end, hasTime) });
  }
  return out;
}
