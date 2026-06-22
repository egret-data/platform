import type { ChartFilter, ValueFilterOp, Chart } from "@/api/charts";
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
