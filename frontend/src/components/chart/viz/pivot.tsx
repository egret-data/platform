import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { FieldZone } from "@/components/chart/config/FieldZone";
import { SectionLabel } from "@/components/chart/settings/SectionLabel";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, QuerySpec, VizSettings } from "@/api/charts";
import type { SharedQueryState, VizControlProps, VizDescriptor } from "./types";

export interface PivotConfig {
  rows: string[];
  cols: string[];
  metrics: string[];
  showRowTotals?: boolean;
  showColTotals?: boolean;
  condense?: boolean;
  /** Serialized collapsed row-group paths — interactive state, persisted with the chart. */
  collapsedRows?: string[];
  sort?: Record<string, "asc" | "desc">;
  /** Row-dimension name -> whether it shows a subtotal band. Absent = the render-time default
   * (every dimension except the innermost). Narrows `showColTotals`, never widens it. */
  dimTotals?: Record<string, boolean>;
  /** Dragged column widths in px. `left` is indexed by row-dimension position, `value` by column
   * leaf index — the same split Metabase persists as `leftHeaderWidths`/`valueHeaderWidths`. */
  columnWidths?: { left?: Record<number, number>; value?: Record<number, number> };
}

const P = "pivot.";
const KEYS = [
  "rows", "cols", "metrics", "showRowTotals", "showColTotals", "condense", "collapsedRows", "sort",
  "dimTotals", "columnWidths",
] as const;

export function pivotToSettings(cfg: PivotConfig): VizSettings {
  const out: VizSettings = {};
  for (const k of KEYS) {
    const v = cfg[k];
    if (v !== undefined) out[P + k] = v;
  }
  return out;
}

export function pivotFromParams(params: AnyChartParams): PivotConfig {
  if (isParamsV2(params)) {
    const s = params.settings;
    const out = { rows: [], cols: [], metrics: [] } as PivotConfig;
    for (const k of KEYS) {
      const v = s[P + k];
      if (v !== undefined) (out as unknown as Record<string, unknown>)[k] = v;
    }
    return out;
  }
  // v1 kept these eight loose at the top level of params.
  return {
    rows: params.rows ?? [],
    cols: params.cols ?? [],
    metrics: params.metrics ?? [],
    showRowTotals: params.showRowTotals ?? true,
    showColTotals: params.showColTotals ?? true,
    condense: params.condense ?? true,
    collapsedRows: params.collapsedRows ?? [],
    sort: params.pivotSort ?? {},
  };
}

function RowsControl({ config, onChange, columns }: VizControlProps<PivotConfig>) {
  return (
    <FieldZone label="Rows" selected={config.rows} onChange={(v) => onChange({ rows: v })}
      options={columns.map((c) => c.column_name).filter((n) => !config.cols.includes(n))} />
  );
}

function ColsControl({ config, onChange, columns, unitRow }: VizControlProps<PivotConfig>) {
  return (
    <>
      <FieldZone label="Columns" selected={config.cols} onChange={(v) => onChange({ cols: v })}
        options={columns.map((c) => c.column_name).filter((n) => !config.rows.includes(n))} />
      {[...config.rows, ...config.cols].map((c) => <div key={c}>{unitRow(c)}</div>)}
    </>
  );
}

function ValuesControl({ config, onChange, metrics }: VizControlProps<PivotConfig>) {
  return (
    <FieldZone label="Values" selected={config.metrics} onChange={(v) => onChange({ metrics: v })}
      options={metrics.map((m) => m.metric_name)} />
  );
}

function TotalsControl({ config, onChange }: VizControlProps<PivotConfig>) {
  const row = (id: string, label: string, checked: boolean, key: keyof PivotConfig) => (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked}
        onCheckedChange={(v) => onChange({ [key]: v } as Partial<PivotConfig>)} />
    </div>
  );
  // Only a dimension that has one nested beneath it produces a subtotal band; the innermost one's
  // groups are leaves. Listing it would offer a switch that does nothing.
  const subtotalDims = config.rows.slice(0, -1);
  const colTotalsOn = config.showColTotals ?? true;

  return (
    <div className="space-y-1">
      <SectionLabel>Totals</SectionLabel>
      <div className="space-y-2.5 pt-1.5">
        {row("show-row-totals", "Row totals", config.showRowTotals ?? true, "showRowTotals")}
        {row("show-col-totals", "Column totals", colTotalsOn, "showColTotals")}
        {row("condense-totals", "Condense duplicate totals", config.condense ?? true, "condense")}

        {colTotalsOn && subtotalDims.length > 0 && (
          <div className="space-y-2 border-t pt-2.5">
            <div className="text-xs text-muted-foreground">Subtotals for</div>
            {subtotalDims.map((name) => (
              <div key={name} className="flex items-center justify-between">
                <Label htmlFor={`subtotal-${name}`} className="truncate text-sm font-normal">{name}</Label>
                <Switch
                  id={`subtotal-${name}`}
                  aria-label={`subtotals for ${name}`}
                  checked={config.dimTotals?.[name] ?? true}
                  onCheckedChange={(v) =>
                    onChange({ dimTotals: { ...(config.dimTotals ?? {}), [name]: v } })}
                />
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

export function pivotDescriptor(): VizDescriptor<PivotConfig> {
  return {
    key: "pivot_table",
    meta: CHART_TYPES.find((t) => t.key === "pivot_table")!,
    prefix: P,
    defaultConfig: { rows: [], cols: [], metrics: [], showRowTotals: true, showColTotals: true, condense: true },
    fromParams: pivotFromParams,
    toSettings: pivotToSettings,
    requiredFields: (cfg) => {
      const out: string[] = [];
      if (!cfg.rows.length) out.push("a row dimension");
      if (!cfg.metrics.length) out.push("a value");
      return out;
    },
    // Note what is absent: collapsedRows and sort are interactive state, so they stay out
    // of the spec — otherwise collapsing a row group would change the query signature and
    // re-run the query underneath the user.
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => {
      const dims = [...cfg.rows, ...cfg.cols];
      return {
        metrics: cfg.metrics, dimensions: dims, columns: [], filters: shared.filters, orderby: [],
        row_limit: shared.rowLimit,
        rollup_groups: [cfg.rows, cfg.cols].filter((g) => g.length > 0),
        temporal_units: pruneUnits(shared.temporalUnits, dims),
      };
    },
    controls: [
      { id: "rows", section: "data", render: RowsControl },
      { id: "cols", section: "data", render: ColsControl },
      { id: "values", section: "data", render: ValuesControl },
      { id: "totals", section: "display", render: TotalsControl },
    ],
  };
}
