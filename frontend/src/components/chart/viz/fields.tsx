import type { ReactNode } from "react";
import { Trash2 } from "lucide-react";
import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { Label } from "@/components/ui/label";
import { DecimalInput } from "@/components/chart/settings/DecimalInput";
import { Checkbox } from "@/components/ui/checkbox";
import { columnLabels, metricLabels, displayName } from "@/components/chart/config/field-names";
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from "@/components/ui/select";
import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, QuerySpec, VizSettings, VizType } from "@/api/charts";
import type { SharedQueryState, VizControl, VizControlProps, VizDescriptor } from "./types";

import { FIELD_LABEL as LABEL_CLASS } from "@/components/chart/settings/FieldLabel";

type Cfg = Record<string, unknown>;

/**
 * Drops the keys a spec-derived config could only fill with nothing — an undefined field or an
 * empty picked-dimensions list. A config recovered from an EMPTY spec then comes back as `{}`,
 * exactly as a brand-new chart's does, so the recovery below cannot make an untouched chart look
 * configured.
 */
function pruneEmpty(cfg: Cfg): Cfg {
  const out: Cfg = {};
  for (const [k, v] of Object.entries(cfg)) {
    if (v === undefined || (Array.isArray(v) && v.length === 0)) continue;
    out[k] = v;
  }
  return out;
}

export interface FieldSpec {
  id: string;
  label: string;
  kind: "dimension" | "metric" | "number" | "boolean" | "dimensions";
  /** Label used in "Add: …" when this field is empty; omit for optional fields. */
  required?: string;
  placeholder?: string;
  /** Show the shared date-grouping row under this field. */
  temporal?: boolean;
  /** Exclude the value of this other field from the options (e.g. sankey source vs target). */
  excludes?: string;
  /** `dimensions` only: how many may be picked. */
  max?: number;
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
  /**
   * Extra controls for the DATA section, rendered after the field pickers.
   *
   * Separate from `display` rather than folded into it, even though a `VizControl` carries its own
   * `section` and either array would work: `display` is passed to nine descriptors and reads as
   * "this viz's display settings" at every one of them. The trend's Comparisons genuinely belong on
   * the Data tab — that is where Metabase puts them, beside the primary-number picker, because a
   * comparison is part of what the card SAYS and not how it looks — and slipping a data control
   * into a list named `display` would make the next reader trust the name over the field.
   */
  data?: VizControl<Cfg>[];
}

const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);
const strs = (xs: unknown[]) => xs.map(str).filter((x): x is string => !!x);

/**
 * `shared.orderby` goes through all three untouched, and none of them derives one of its own —
 * these vizzes order themselves client-side (the pie's ring, the funnel's steps, the box plot's
 * categories) and asking the engine for an order would change which rows survive `row_limit`.
 * Empty in, empty out, which is what every already-saved chart carries.
 *
 * The one descriptor built on these that DOES derive its own is smartscalar, which overrides the
 * result of `dimsAndMetrics` for a reason stated at its declaration in `simple.tsx`.
 */
const helpers: Helpers = {
  dimsAndMetrics: (dims, metrics, shared) => {
    const d = strs(dims);
    return {
      metrics: strs(metrics), dimensions: d, columns: [],
      filters: shared.filters, orderby: shared.orderby, row_limit: shared.rowLimit, rollup_groups: [],
      temporal_units: pruneUnits(shared.temporalUnits, d),
    };
  },
  rawColumns: (cols, shared) => ({
    metrics: [], dimensions: [], columns: strs(cols),
    filters: shared.filters, orderby: shared.orderby, row_limit: shared.rowLimit, rollup_groups: [],
  }),
  // The scalar family reduces to a single aggregate row.
  oneRow: (metric, shared) => ({
    metrics: strs([metric]), dimensions: [], columns: [],
    filters: shared.filters, orderby: shared.orderby, row_limit: 1, rollup_groups: [],
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
        {/* DecimalInput rather than `type="number"`. This descriptor's only user today is the
            progress chart's Goal, and a target of 0.85 or 1250.5 is ordinary — but the rule is
            about the KIND: a numeric field with no integer constraint has to be able to hold "4."
            while it is being typed, which a number input cannot (it reports itself empty for
            exactly that long). See DecimalInput. If a future field here is genuinely integer-only,
            it needs its own descriptor kind, not this one. */}
        <DecimalInput id={`f-${field.id}`} className="h-8 text-sm" placeholder={field.placeholder}
          value={config[field.id] as number | undefined}
          onChange={set} />
      </div>
    );
  }
  const options = field.kind === "metric"
    ? metrics.map((m) => m.metric_name)
    : columns.map((c) => c.column_name).filter((n) => !field.excludes || n !== config[field.excludes]);
  // Datasets have carried `verbose_name` for a long time and offer a UI to set it, but every field
  // picker read the machine name, so naming a column "Total Sales" changed nothing here. Value
  // stays the machine name — only the label changes.
  //
  // THIS WAS A HAND-ROLLED `displayName`, and #444 filed it as one of three "genuinely different
  // shapes" needing their own refactor. It is not different: it is a lookup by name against the
  // one map `columnLabels`/`metricLabels` build, with the identical blank-and-whitespace guard
  // (`?.trim() || n`) spelled out longhand. Folding it in needed no flags and no options object,
  // which is the test this codebase applies to a share — the recorded failure is a shared function
  // that grew a parameter per caller, not one that absorbed a duplicate unchanged.
  //
  // Two `find`s per option became one map for the list, which is incidental but real: the pie's
  // dimension picker builds this once for up to three slots over every column in the dataset.
  const labels = field.kind === "metric" ? metricLabels(metrics) : columnLabels(columns);
  const labelOf = (n: string) => displayName(n, labels);
  if (field.kind === "dimensions") {
    return <DimensionsControl field={field} config={config} options={options} labelOf={labelOf} onChange={onChange} unitRow={unitRow} />;
  }
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>{field.label}</Label>
        <Select value={(config[field.id] as string) ?? ""} onValueChange={(v) => set(v || undefined)}>
          <SelectTrigger className="h-8 w-full text-sm">
            <SelectValue placeholder={field.placeholder ?? (field.kind === "metric" ? "Pick a metric…" : "Pick a dimension…")} />
          </SelectTrigger>
          <SelectContent>
            {options.map((n) => <SelectItem key={n} value={n}>{labelOf(n)}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      {field.temporal && unitRow(config[field.id] as string | undefined)}
    </>
  );
}

/**
 * A `dimensions` field: an ordered list of up to `field.max` column names, rendered as one
 * picker per already-chosen column plus a single empty "add" picker while under the limit.
 * Every emit replaces the whole array — there is no partial-patch path — because the stored
 * shape (and every reader of it, e.g. pie's nested rings) is the array itself, in order.
 */
function DimensionsControl({ field, config, options, labelOf, onChange, unitRow }: {
  field: FieldSpec;
  config: Cfg;
  options: string[];
  /** Machine name -> display name; see `labelOf` in the caller. */
  labelOf: (n: string) => string;
  onChange: (patch: Partial<Cfg>) => void;
  unitRow: (col?: string) => ReactNode;
}) {
  const max = field.max ?? Infinity;
  const chosen = Array.isArray(config[field.id]) ? (config[field.id] as string[]) : [];
  const emit = (next: string[]) => onChange({ [field.id]: next } as Partial<Cfg>);
  // One slot per chosen column, plus a trailing empty "add" slot while under `max`.
  const slots: (string | undefined)[] = chosen.length < max ? [...chosen, undefined] : chosen;
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>{field.label}</Label>
      {slots.map((value, i) => {
        // A column already picked by another slot is excluded here, so the same column
        // can't be chosen twice — but THIS slot's own current value stays offered, or
        // reopening it would make its own pick look like it vanished from the list.
        const takenElsewhere = new Set(chosen.filter((_, j) => j !== i));
        const avail = options.filter((n) => !takenElsewhere.has(n));
        return (
          <div key={i} className="flex items-center gap-1">
            <Select value={value ?? ""} onValueChange={(v) => { if (v) { const next = [...chosen]; next[i] = v; emit(next); } }}>
              <SelectTrigger className="h-8 w-full text-sm">
                <SelectValue placeholder={value === undefined ? "Add a dimension…" : (field.placeholder ?? "Pick a dimension…")} />
              </SelectTrigger>
              <SelectContent>
                {avail.map((n) => <SelectItem key={n} value={n}>{labelOf(n)}</SelectItem>)}
              </SelectContent>
            </Select>
            {value !== undefined && (
              <button type="button" aria-label={`Remove ${value}`}
                className="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-destructive"
                onClick={() => emit(chosen.filter((_, j) => j !== i))}>
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            )}
          </div>
        );
      })}
      {/* Keyed by index, not by column name: the picker cannot produce a duplicate, but stored
          settings are not whitelisted server-side, so a malformed array can — and a duplicate
          value key is unsupported in React. */}
      {field.temporal && chosen.map((c, i) => <div key={i}>{unitRow(c)}</div>)}
    </div>
  );
}

/**
 * Builds a descriptor for a viz whose whole configuration is a short list of field
 * pickers. Nine of the nineteen viz types are exactly that, so they are declared rather
 * than hand-written — the same move Metabase and Superset make with their settings and
 * control-panel definitions, without building a general widget framework for it.
 */
export function fieldDescriptor(spec: FieldDescriptorSpec): VizDescriptor<Cfg> {
  const { key, prefix, fields, legacyKey, legacyFromSpec, display = [], data = [] } = spec;
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
        // No slice at all under this prefix: same recovery as v1. The API accepts
        // `settings: {}` — keys are open by design — so a chart written by anything but this
        // editor arrives with its fields only in the spec, and returning `{}` opened it on the
        // "set up your chart" placeholder. Empty spec in, empty config out, so a brand-new
        // chart is untouched.
        if (Object.keys(out).length === 0 && legacyFromSpec && p.spec) return pruneEmpty(legacyFromSpec(p.spec));
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
    // `dimensions` stores an array, so "unset" must also catch the emptied-out `[]` a user
    // gets to by removing their last pick — plain falsiness (`!cfg[f.id]`) misses it, since
    // an empty array is truthy in JS.
    requiredFields: (cfg) => fields.filter((f) => {
      if (!f.required) return false;
      const v = cfg[f.id];
      return !v || (Array.isArray(v) && v.length === 0);
    }).map((f) => f.required!),
    buildSpec: (cfg, shared) => spec.buildSpec(cfg, shared, helpers),
    controls: [
      ...fields.map((f): VizControl<Cfg> => ({
        id: f.id,
        section: "data",
        render: (props) => <FieldControl field={f} {...props} />,
      })),
      ...data,
      ...display,
    ],
  };
}
