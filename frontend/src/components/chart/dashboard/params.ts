import type { ChartFilter, ValueFilterOp, Chart, ChartParams, VizType } from "@/api/charts";
import type { DashboardParameter } from "@/api/dashboards";

export function slugify(name: string, existing: string[]): string {
  const base =
    name.trim().toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "") || "param";
  if (!existing.includes(base)) return base;
  let i = 2;
  while (existing.includes(`${base}_${i}`)) i++;
  return `${base}_${i}`;
}

export const isBlank = (v: unknown) => v === undefined || v === null || v === "";

const RELATIVE_LABELS: Record<string, string> = {
  last7: "Last 7 days", last30: "Last 30 days", last90: "Last 90 days", last365: "Last 365 days",
};

/** A short human summary of a parameter's current value for the filter pill. */
export function summarize(param: DashboardParameter, value: unknown): string {
  if (isBlank(value)) return "";
  if (Array.isArray(value)) {
    const vals = value.filter((v) => !isBlank(v));
    if (vals.length === 0) return "";
    if (vals.length === 1) return String(vals[0]);
    if (vals.length === 2 && (param.operator === "between" || param.operator === "range")) {
      return `${vals[0]} – ${vals[1]}`;
    }
    return `${vals.length} selections`;
  }
  if (param.type === "boolean") {
    return value === true || value === "true" ? "True" : "False";
  }
  if (param.type === "date" && param.operator === "all-options") {
    const v = value as AllOptVal | undefined;
    if (!v || !v.mode) return "";
    if (v.mode === "relative") {
      const unit = v.unit ?? "day";
      if (v.dir === "current") return `Current ${unit}`;
      const word = v.dir === "next" ? "Next" : "Past";
      const plural = (v.n ?? 0) === 1 ? unit : `${unit}s`;
      return `${word} ${v.n} ${plural}`;
    }
    if (v.op === "between") return `${v.from} – ${v.to ?? ""}`.trim();
    const opWord = v.op === "after" ? "After" : v.op === "before" ? "Before" : "On";
    return v.from ? `${opWord} ${v.from}` : "";
  }
  if (param.type === "date" && param.operator === "quarter-year") {
    const [y, q] = String(value).split("-");
    return q ? `Q${q} ${y}` : String(value);
  }
  if (param.type === "date" && param.operator === "relative") {
    return RELATIVE_LABELS[String(value)] ?? String(value);
  }
  return String(value);
}

const RELATIVE_DAYS: Record<string, number> = {
  last7: 7,
  last30: 30,
  last90: 90,
  last365: 365,
};

const iso = (d: Date) => d.toISOString().slice(0, 10);

export type DateUnit = "day" | "week" | "month" | "quarter" | "year";

/** Canonical value shape for a `date` parameter with the `all-options` operator. */
export type DateAllOptionsValue =
  | { mode: "relative"; dir: "past" | "next" | "current"; n: number; unit: DateUnit }
  | { mode: "specific"; op: "on" | "before" | "after" | "between"; from: string; to?: string };

/** Loosened, all-fields-optional read shape used when narrowing an unknown value. */
type AllOptVal = {
  mode?: "relative" | "specific";
  dir?: "past" | "next" | "current";
  n?: number;
  unit?: DateUnit;
  op?: "on" | "before" | "after" | "between";
  from?: string;
  to?: string;
};

function shift(d: Date, n: number, unit: DateUnit): Date {
  const r = new Date(d);
  if (unit === "day") r.setUTCDate(r.getUTCDate() + n);
  else if (unit === "week") r.setUTCDate(r.getUTCDate() + n * 7);
  else if (unit === "month") r.setUTCMonth(r.getUTCMonth() + n);
  else if (unit === "quarter") r.setUTCMonth(r.getUTCMonth() + n * 3);
  else r.setUTCFullYear(r.getUTCFullYear() + n);
  return r;
}

export function paramToFilters(
  param: DashboardParameter,
  value: unknown,
  column: string,
  now: Date = new Date(),
): ChartFilter[] {
  const mkf = (op: ValueFilterOp, v: unknown): ChartFilter => ({ col: column, op, value: v });

  switch (param.type) {
    case "text": {
      if (isBlank(value)) return [];
      const v = String(value);
      switch (param.operator) {
        case "is":
          return [mkf("eq", v)];
        case "is-not":
          return [mkf("ne", v)];
        case "contains":
          return [mkf("like", `%${v}%`)];
        case "starts-with":
          return [mkf("like", `${v}%`)];
        case "ends-with":
          return [mkf("like", `%${v}`)];
        default:
          return [mkf("eq", v)];
      }
    }

    case "number": {
      if (param.operator === "between") {
        const [min, max] = Array.isArray(value) ? value : [undefined, undefined];
        const out: ChartFilter[] = [];
        if (!isBlank(min)) out.push(mkf("ge", Number(min)));
        if (!isBlank(max)) out.push(mkf("le", Number(max)));
        return out;
      }
      if (isBlank(value)) return [];
      const n = Number(value);
      const opMap: Partial<Record<string, ValueFilterOp>> = {
        "=": "eq",
        "!=": "ne",
        ">=": "ge",
        "<=": "le",
      };
      const op = opMap[param.operator] ?? "eq";
      return [mkf(op, n)];
    }

    case "date": {
      if (param.operator === "all-options") {
        const v = value as AllOptVal | undefined;
        if (!v || !v.mode) return [];
        if (v.mode === "relative") {
          const unit = v.unit ?? "day";
          if (v.dir === "current") {
            return [mkf("ge", iso(now)), mkf("le", iso(now))];
          }
          const n = Number(v.n) || 0;
          const other = v.dir === "next" ? shift(now, n, unit) : shift(now, -n, unit);
          const [lo, hi] = v.dir === "next" ? [now, other] : [other, now];
          return [mkf("ge", iso(lo)), mkf("le", iso(hi))];
        }
        if (isBlank(v.from)) return [];
        const from = String(v.from);
        switch (v.op) {
          case "on":
            return [mkf("eq", from)];
          case "before":
            return [mkf("lt", from)];
          case "after":
            return [mkf("gt", from)];
          case "between": {
            const out: ChartFilter[] = [mkf("ge", from)];
            if (!isBlank(v.to)) out.push(mkf("le", String(v.to)));
            return out;
          }
          default:
            return [mkf("eq", from)];
        }
      }
      if (param.operator === "single") {
        return isBlank(value) ? [] : [mkf("eq", String(value))];
      }
      if (param.operator === "range") {
        const [start, end] = Array.isArray(value) ? value : [undefined, undefined];
        const out: ChartFilter[] = [];
        if (!isBlank(start)) out.push(mkf("ge", String(start)));
        if (!isBlank(end)) out.push(mkf("le", String(end)));
        return out;
      }
      if (param.operator === "month-year") {
        if (isBlank(value)) return [];
        const [y, m] = String(value).split("-").map(Number);
        if (!y || !m) return [];
        // Date.UTC(y, m, 0) = day 0 of month m+1 (m is 1-based) = last day of month m.
        const last = new Date(Date.UTC(y, m, 0)).getUTCDate();
        const mm = String(m).padStart(2, "0");
        return [mkf("ge", `${y}-${mm}-01`), mkf("le", `${y}-${mm}-${String(last).padStart(2, "0")}`)];
      }
      if (param.operator === "quarter-year") {
        if (isBlank(value)) return [];
        const [y, q] = String(value).split("-").map(Number);
        if (!y || !q || q < 1 || q > 4) return [];
        const startM = (q - 1) * 3 + 1;
        const endM = q * 3;
        const lastDay = new Date(Date.UTC(y, endM, 0)).getUTCDate();
        return [
          mkf("ge", `${y}-${String(startM).padStart(2, "0")}-01`),
          mkf("le", `${y}-${String(endM).padStart(2, "0")}-${String(lastDay).padStart(2, "0")}`),
        ];
      }
      // relative
      const days = RELATIVE_DAYS[String(value)];
      if (!days) return [];
      const end = new Date(now);
      const start = new Date(now);
      start.setUTCDate(start.getUTCDate() - days);
      return [mkf("ge", iso(start)), mkf("le", iso(end))];
    }

    case "id":
      return isBlank(value) ? [] : [mkf("eq", value)];

    case "category": {
      const arr = Array.isArray(value)
        ? value.filter((v) => !isBlank(v))
        : isBlank(value)
          ? []
          : [value];
      return arr.length ? [mkf("in", arr)] : [];
    }

    case "boolean": {
      if (isBlank(value)) return [];
      const b = value === true || value === "true";
      return [mkf("eq", b)];
    }

    case "location": {
      const arr = Array.isArray(value)
        ? value.filter((v) => !isBlank(v))
        : isBlank(value)
          ? []
          : [value];
      if (!arr.length) return [];
      return [mkf(param.operator === "is-not" ? "not_in" : "in", arr)];
    }

    default:
      return [];
  }
}

export function applyParamsToChart(
  chart: Chart,
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
  mappings: Record<string, string>,
  now: Date = new Date(),
): Chart {
  const extra: ChartFilter[] = [];
  for (const param of parameters) {
    const column = mappings[param.id];
    if (!column) continue;
    extra.push(...paramToFilters(param, values[param.slug], column, now));
  }
  if (extra.length === 0) return chart;
  return {
    ...chart,
    params: {
      ...chart.params,
      spec: {
        ...chart.params.spec,
        filters: [...chart.params.spec.filters, ...extra],
      },
    },
  };
}

/**
 * Filters that constrain a parameter's option list to values consistent with its linked
 * parameters' current values. Each linked param must be mapped on the SAME card (so we can read
 * its column from `mappings`); linked params with no value or no mapping are ignored.
 */
export function computeLinkedFilters(
  param: DashboardParameter,
  mappings: Record<string, string>,
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
  now: Date = new Date(),
): ChartFilter[] {
  const out: ChartFilter[] = [];
  for (const linkedId of param.filteringParameters ?? []) {
    const linked = parameters.find((p) => p.id === linkedId);
    if (!linked) continue;
    const col = mappings[linkedId];
    if (!col) continue;
    const v = values[linked.slug];
    if (isBlank(v)) continue;
    out.push(...paramToFilters(linked, v, col, now));
  }
  return out;
}

export function interpolateParams(
  text: string,
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
): string {
  const bySlug = new Map(parameters.map((p) => [p.slug, p]));
  return text.replace(/\{\{\s*([\w-]+)\s*\}\}/g, (m, slug: string) => {
    if (!bySlug.has(slug)) return m;
    const v = values[slug];
    return v == null ? "" : Array.isArray(v) ? v.join(", ") : String(v);
  });
}

/** Merge a display-only override over a chart's params (never the query `spec`); optionally override viz_type. */
export function applyVizOverride(
  chart: Chart,
  override?: Record<string, unknown>,
): { viz_type: VizType; params: ChartParams } {
  if (!override) return { viz_type: chart.viz_type, params: chart.params };
  const { viz_type: ovViz, spec: _ignoredSpec, ...display } = override as { viz_type?: VizType; spec?: unknown };
  const params: ChartParams = { ...chart.params };
  const baseParams = chart.params as unknown as Record<string, unknown>;
  const target = params as unknown as Record<string, unknown>;
  for (const [k, v] of Object.entries(display)) {
    const baseVal = baseParams[k];
    // deep-merge plain objects (config sub-objects like cartesian/pie); replace arrays/primitives
    target[k] =
      baseVal && typeof baseVal === "object" && !Array.isArray(baseVal) && v && typeof v === "object" && !Array.isArray(v)
        ? { ...(baseVal as object), ...(v as object) }
        : v;
  }
  params.spec = chart.params.spec; // query is always the chart's spec (+ dashboard injection upstream)
  return { viz_type: ovViz ?? chart.viz_type, params };
}
