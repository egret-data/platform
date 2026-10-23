import type { ChartFilter, ValueFilterOp, Chart, AnyChartParams, VizType, QuerySpec } from "@/api/charts";
import { isParamsV2 } from "@/api/charts";
import type { DashboardParameter } from "@/api/dashboards";
import { TEMPORAL_UNITS } from "@/components/chart/temporal-units";

export function slugify(name: string, existing: string[]): string {
  const base =
    name.trim().toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "") || "param";
  if (!existing.includes(base)) return base;
  let i = 2;
  while (existing.includes(`${base}_${i}`)) i++;
  return `${base}_${i}`;
}

/**
 * The slug a filter should have after an edit in its settings panel (KI-065). The slug is the
 * filter's name in URLs (`?region=…`) and in `{{region}}` on text, heading and link cards.
 *
 * - An explicit edit of the slug wins, made URL-safe and unique among `otherSlugs`.
 * - A filter that is not saved yet, whose slug is still the one derived from its name, follows
 *   its name: "Filter" → "Order status" gives `order_status`, not `filter`.
 * - A saved filter keeps its slug when renamed, so links already shared keep working.
 */
export function nextParameterSlug(
  prev: DashboardParameter,
  next: DashboardParameter,
  otherSlugs: string[],
  saved: boolean,
): string {
  if (next.slug !== prev.slug) return slugify(next.slug, otherSlugs);
  if (!saved && next.name !== prev.name && prev.slug === slugify(prev.name, otherSlugs)) {
    return slugify(next.name, otherSlugs);
  }
  return prev.slug;
}

/** `value` with every `{{from}}` (spaces inside the braces allowed) written as `{{to}}`, in every
 *  string it holds: a text or heading card's text, a link card's URL and label, a click-behaviour
 *  URL template. Used when a filter's slug changes, so the references on the same dashboard
 *  follow it. Another slug that merely starts with `from` (`{{from_2}}`) is left alone. */
export function renameSlugReferences<T>(value: T, from: string, to: string): T {
  const escaped = from.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`\\{\\{\\s*${escaped}\\s*\\}\\}`, "g");
  const walk = (v: unknown): unknown => {
    if (typeof v === "string") return v.replace(re, `{{${to}}}`);
    if (Array.isArray(v)) return v.map(walk);
    if (v && typeof v === "object") return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, walk(x)]));
    return v;
  };
  return walk(value) as T;
}

export const isBlank = (v: unknown) => v === undefined || v === null || v === "";

const RELATIVE_LABELS: Record<string, string> = {
  last7: "Last 7 days", last30: "Last 30 days", last90: "Last 90 days", last365: "Last 365 days",
};

/** A short human summary of a parameter's current value for the filter pill. */
export function summarize(param: DashboardParameter, value: unknown): string {
  if (isBlank(value)) return "";
  if (param.type === "temporal_unit") {
    return TEMPORAL_UNITS.find((u) => u.token === value)?.label ?? String(value);
  }
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
        case "does-not-contain":
          return [mkf("not_like", `%${v}%`)];
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

    case "id": {
      // A single id keeps emitting `eq` — that is what every saved dashboard's id filter does and
      // what `params.test.ts` pins. Only an author who turned "People can pick" to Multiple values
      // can produce more than one, and that becomes `in`.
      const arr = Array.isArray(value)
        ? value.filter((v) => !isBlank(v))
        : isBlank(value)
          ? []
          : [value];
      if (arr.length === 0) return [];
      return arr.length === 1 ? [mkf("eq", arr[0])] : [mkf("in", arr)];
    }

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

/**
 * The dashboard-parameter-derived filters for one card: the delta a card's mapped
 * parameters add on top of its saved chart spec (what `applyParamsToChart` appends
 * to `spec.filters`). Temporal-unit params re-grid a column rather than filter, so
 * they contribute NO filters here. This is the exact list the view-mode data path
 * sends to the object-scoped card-data endpoint, which validates each `col` against
 * the dataset's real columns and merges them into the persisted query server-side.
 */
export function paramFiltersForCard(
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
  mappings: Record<string, string>,
  now: Date = new Date(),
): ChartFilter[] {
  const extra: ChartFilter[] = [];
  for (const param of parameters) {
    const column = mappings[param.id];
    if (!column) continue;
    if (param.type === "temporal_unit") continue; // re-grids, not a filter
    extra.push(...paramToFilters(param, values[param.slug], column, now));
  }
  return extra;
}

/**
 * The dashboard-parameter-derived temporal-unit overrides for one card: for each
 * `temporal_unit` param mapped to a column that the card's saved spec ALREADY groups
 * temporally (`spec.temporal_units[column]` is set), the resolved unit token that
 * re-grids that column. This is the exact map the view-mode data path sends to the
 * object-scoped card-data endpoint, which validates each token against the
 * `TemporalUnit` enum and merges it into the persisted query server-side (an override
 * replaces the saved unit for that column). Non-temporal params contribute nothing.
 */
export function temporalOverridesForCard(
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
  mappings: Record<string, string>,
  spec: QuerySpec,
): Record<string, string> {
  const temporalOverrides: Record<string, string> = {};
  for (const param of parameters) {
    if (param.type !== "temporal_unit") continue;
    const column = mappings[param.id];
    if (!column) continue;
    const raw = values[param.slug];
    const unit = typeof raw === "string" && raw ? raw : param.default;
    const current = spec.temporal_units ?? {};
    if (typeof unit === "string" && unit && column in current) {
      temporalOverrides[column] = unit; // re-grid an already-grouped column
    }
  }
  return temporalOverrides;
}

export function applyParamsToChart(
  chart: Chart,
  parameters: DashboardParameter[],
  values: Record<string, unknown>,
  mappings: Record<string, string>,
  now: Date = new Date(),
): Chart {
  const extra = paramFiltersForCard(parameters, values, mappings, now);
  const temporalOverrides = temporalOverridesForCard(parameters, values, mappings, chart.params.spec);
  if (extra.length === 0 && Object.keys(temporalOverrides).length === 0) return chart;
  const spec = { ...chart.params.spec, filters: [...chart.params.spec.filters, ...extra] };
  if (Object.keys(temporalOverrides).length > 0) {
    spec.temporal_units = { ...(chart.params.spec.temporal_units ?? {}), ...temporalOverrides };
  }
  return { ...chart, params: { ...chart.params, spec } };
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
    return formatParamValue(bySlug.get(slug)!, values[slug]);
  });
}

/** One filter value as text for a text or heading card. Dates read the way the filter chip reads
 *  them (`summarize`): a range as "from – to", a relative or all-options pick in words. A plain
 *  object has no text form and prints nothing; `String()` gave it `[object Object]` (KI-063). */
function formatParamValue(param: DashboardParameter, v: unknown): string {
  if (isBlank(v)) return "";
  const isPlainObject = (x: unknown) => typeof x === "object" && x !== null && !Array.isArray(x);
  if (param.type === "date" || (Array.isArray(v) && param.operator === "between")) {
    return isPlainObject(v) && param.operator !== "all-options" ? "" : summarize(param, v);
  }
  if (Array.isArray(v)) {
    return v.filter((x) => !isBlank(x) && !isPlainObject(x) && !Array.isArray(x)).map(String).join(", ");
  }
  return isPlainObject(v) ? "" : String(v);
}

/**
 * Merge a per-card override onto a chart's settings. In v2 an override is a flat partial
 * of `settings`, so merging is by key and needs no per-viz knowledge — the reason A2 chose
 * flat settings in the first place. The query `spec` is never overridden.
 *
 * Overrides stored before migration 0100 are v1-shaped (nested `cartesian`/`pie` objects,
 * `columnSettings`, …); `flattenLegacyOverride` accepts those so a dashboard opened during
 * a deploy cannot break.
 */
export function applyVizOverride(
  chart: { viz_type: VizType; params: AnyChartParams },
  override?: Record<string, unknown>,
): { viz_type: VizType; params: AnyChartParams } {
  if (!override) return { viz_type: chart.viz_type, params: chart.params };
  const { viz_type: ovViz, spec: _ignoredSpec, ...rest } = override as { viz_type?: VizType; spec?: unknown };
  const settings = flattenLegacyOverride(rest);
  const base = isParamsV2(chart.params)
    ? chart.params
    : { version: 2 as const, spec: chart.params.spec, settings: {} };
  return {
    viz_type: ovViz ?? chart.viz_type,
    params: { ...base, settings: { ...base.settings, ...settings } },
  };
}

/** The v1 nested config keys an override could carry, one per viz family. */
const LEGACY_VIZ_KEYS = new Set([
  "cartesian", "pie", "scalar", "progress", "gauge", "funnel", "waterfall",
  "smartscalar", "boxplot", "sankey", "map",
]);

/** v1 override keys → flat settings keys; already-flat keys pass through untouched. */
export function flattenLegacyOverride(override: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(override)) {
    if (k === "columnSettings") { out.column_settings = v; continue; }
    if (k === "conditionalFormatting") { out.conditional_formatting = v; continue; }
    if (k === "tableState") { out["table.state"] = v; continue; }
    // A nested v1 config object becomes `<viz>.<field>`. Matched against the known v1
    // keys, not "is an object": `column_settings` is a flat key that happens to hold one.
    if (LEGACY_VIZ_KEYS.has(k) && v && typeof v === "object" && !Array.isArray(v)) {
      for (const [ik, iv] of Object.entries(v as Record<string, unknown>)) {
        out[`${k}.${ik === "mapKey" ? "key" : ik}`] = iv;
      }
      continue;
    }
    out[k] = v;
  }
  return out;
}
