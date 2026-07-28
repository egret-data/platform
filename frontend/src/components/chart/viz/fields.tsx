import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from "@/components/ui/select";
import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, QuerySpec, VizSettings, VizType } from "@/api/charts";
import type { SharedQueryState, VizControl, VizControlProps, VizDescriptor } from "./types";

const LABEL_CLASS = "text-xs font-medium uppercase tracking-wide text-muted-foreground";

type Cfg = Record<string, unknown>;

export interface FieldSpec {
  id: string;
  label: string;
  kind: "dimension" | "metric" | "number" | "boolean";
  /** Label used in "Add: …" when this field is empty; omit for optional fields. */
  required?: string;
  placeholder?: string;
  /** Show the shared date-grouping row under this field. */
  temporal?: boolean;
  /** Exclude the value of this other field from the options (e.g. sankey source vs target). */
  excludes?: string;
}

export interface Helpers {
  dimsAndMetrics: (dims: unknown[], metrics: unknown[], shared: SharedQueryState) => QuerySpec;
  rawColumns: (cols: unknown[], shared: SharedQueryState) => QuerySpec;
  oneRow: (metric: unknown, shared: SharedQueryState) => QuerySpec;
}

export interface FieldDescriptorSpec {
  key: VizType;
  prefix: string;
  fields: FieldSpec[];
  buildSpec: (cfg: Cfg, shared: SharedQueryState, helpers: Helpers) => QuerySpec;
  /** The v1 nested key this viz used (e.g. `funnel`, `boxplot`, `smartscalar`). */
  legacyKey?: string;
  /** Reads a v1 chart that predates the nested key, from its spec. */
  legacyFromSpec?: (spec: QuerySpec) => Cfg;
  display?: VizControl<Cfg>[];
}

const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);
const strs = (xs: unknown[]) => xs.map(str).filter((x): x is string => !!x);

const helpers: Helpers = {
  dimsAndMetrics: (dims, metrics, shared) => {
    const d = strs(dims);
    return {
      metrics: strs(metrics), dimensions: d, columns: [],
      filters: shared.filters, orderby: [], row_limit: shared.rowLimit, rollup_groups: [],
      temporal_units: pruneUnits(shared.temporalUnits, d),
    };
  },
  rawColumns: (cols, shared) => ({
    metrics: [], dimensions: [], columns: strs(cols),
    filters: shared.filters, orderby: [], row_limit: shared.rowLimit, rollup_groups: [],
  }),
  // The scalar family reduces to a single aggregate row.
  oneRow: (metric, shared) => ({
    metrics: strs([metric]), dimensions: [], columns: [],
    filters: shared.filters, orderby: [], row_limit: 1, rollup_groups: [],
  }),
};

function FieldControl({ field, config, onChange, columns, metrics, unitRow }: {
  field: FieldSpec;
} & VizControlProps<Cfg>) {
  const set = (v: unknown) => onChange({ [field.id]: v } as Partial<Cfg>);
  if (field.kind === "boolean") {
    return (
      <div className="flex items-center gap-2">
        <Checkbox id={`f-${field.id}`} checked={!!config[field.id]} onCheckedChange={() => set(!config[field.id])} />
        <Label htmlFor={`f-${field.id}`} className="cursor-pointer text-sm font-normal">{field.label}</Label>
      </div>
    );
  }
  if (field.kind === "number") {
    return (
      <div className="space-y-1">
        <Label htmlFor={`f-${field.id}`} className={LABEL_CLASS}>{field.label}</Label>
        <Input id={`f-${field.id}`} type="number" className="h-8 text-sm" placeholder={field.placeholder}
          value={(config[field.id] as number | undefined) ?? ""}
          onChange={(e) => {
            const n = Number(e.target.value);
            set(e.target.value === "" || Number.isNaN(n) ? undefined : n);
          }} />
      </div>
    );
  }
  const options = field.kind === "metric"
    ? metrics.map((m) => m.metric_name)
    : columns.map((c) => c.column_name).filter((n) => !field.excludes || n !== config[field.excludes]);
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>{field.label}</Label>
        <Select value={(config[field.id] as string) ?? ""} onValueChange={(v) => set(v || undefined)}>
          <SelectTrigger className="h-8 text-sm">
            <SelectValue placeholder={field.placeholder ?? (field.kind === "metric" ? "Pick a metric…" : "Pick a dimension…")} />
          </SelectTrigger>
          <SelectContent>
            {options.map((n) => <SelectItem key={n} value={n}>{n}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      {field.temporal && unitRow(config[field.id] as string | undefined)}
    </>
  );
}

/**
 * Builds a descriptor for a viz whose whole configuration is a short list of field
 * pickers. Nine of the nineteen viz types are exactly that, so they are declared rather
 * than hand-written — the same move Metabase and Superset make with their settings and
 * control-panel definitions, without building a general widget framework for it.
 */
export function fieldDescriptor(spec: FieldDescriptorSpec): VizDescriptor<Cfg> {
  const { key, prefix, fields, legacyKey, legacyFromSpec, display = [] } = spec;
  return {
    key,
    meta: CHART_TYPES.find((t) => t.key === key)!,
    prefix,
    defaultConfig: {},
    fromParams: (p: AnyChartParams): Cfg => {
      if (isParamsV2(p)) {
        // Every key under this prefix, not just the declared fields: a viz's display
        // settings (pie's donut, legend, slice colours…) are edited by its display
        // control and never appear in `fields`, which only drives the pickers.
        const out: Cfg = {};
        for (const [k, v] of Object.entries(p.settings)) {
          if (k.startsWith(prefix) && v !== undefined) out[k.slice(prefix.length)] = v;
        }
        return out;
      }
      const nested = legacyKey ? (p as unknown as Record<string, Cfg | undefined>)[legacyKey] : undefined;
      if (nested) return { ...nested };
      return legacyFromSpec && p.spec ? legacyFromSpec(p.spec) : {};
    },
    toSettings: (cfg) => {
      const out: VizSettings = {};
      for (const [k, v] of Object.entries(cfg)) if (v !== undefined) out[prefix + k] = v;
      return out;
    },
    requiredFields: (cfg) => fields.filter((f) => f.required && !cfg[f.id]).map((f) => f.required!),
    buildSpec: (cfg, shared) => spec.buildSpec(cfg, shared, helpers),
    controls: [
      ...fields.map((f): VizControl<Cfg> => ({
        id: f.id,
        section: "data",
        render: (props) => <FieldControl field={f} {...props} />,
      })),
      ...display,
    ],
  };
}
