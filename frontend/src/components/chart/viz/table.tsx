import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { isParamsV2 } from "@/api/charts";
import type { QuerySpec } from "@/api/charts";
import type { SharedQueryState, VizControlProps, VizDescriptor } from "./types";

export interface TableConfig { columns: string[]; metrics: string[] }
export interface ObjectConfig { columns: string[] }

const LABEL_CLASS = "text-xs font-medium uppercase tracking-wide text-muted-foreground";

/** A checkbox list — the shape both of these viz types configure with. */
function CheckList({ idPrefix, empty, options, selected, onToggle }: {
  idPrefix: string; empty: string; options: string[]; selected: string[]; onToggle: (name: string) => void;
}) {
  return (
    <div className="space-y-1 pt-1">
      {options.length === 0 && <p className="text-xs italic text-muted-foreground">{empty}</p>}
      {options.map((name) => (
        <div key={name} className="flex items-center gap-2">
          <Checkbox id={`${idPrefix}-${name}`} checked={selected.includes(name)} onCheckedChange={() => onToggle(name)} />
          <Label htmlFor={`${idPrefix}-${name}`} className="cursor-pointer text-sm font-normal">{name}</Label>
        </div>
      ))}
    </div>
  );
}

const toggle = (list: string[], name: string) =>
  list.includes(name) ? list.filter((x) => x !== name) : [...list, name];

function TableMetricsControl({ config, onChange, metrics }: VizControlProps<TableConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Metrics</Label>
      <p className="text-xs text-muted-foreground">Pick metrics for an aggregated table, or just columns for raw rows.</p>
      <CheckList idPrefix="metric" empty="No metrics on this dataset."
        options={metrics.map((m) => m.metric_name)} selected={config.metrics}
        onToggle={(n) => onChange({ metrics: toggle(config.metrics, n) })} />
    </div>
  );
}

function TableColumnsControl({ config, onChange, columns, unitRow }: VizControlProps<TableConfig>) {
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Columns</Label>
        <CheckList idPrefix="col" empty="Select a dataset to see columns."
          options={columns.map((c) => c.column_name)} selected={config.columns}
          onToggle={(n) => onChange({ columns: toggle(config.columns, n) })} />
      </div>
      {config.metrics.length > 0 && config.columns.map((c) => <div key={c}>{unitRow(c)}</div>)}
    </>
  );
}

function ObjectFieldsControl({ config, onChange, columns }: VizControlProps<ObjectConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Fields</Label>
      <CheckList idPrefix="obj" empty="Select a dataset to see columns."
        options={columns.map((c) => c.column_name)} selected={config.columns}
        onToggle={(n) => onChange({ columns: toggle(config.columns, n) })} />
    </div>
  );
}

export function tableDescriptor(): VizDescriptor<TableConfig> {
  return {
    key: "table",
    meta: CHART_TYPES.find((t) => t.key === "table")!,
    prefix: "table.",
    defaultConfig: { columns: [], metrics: [] },
    fromParams: (p) => {
      // v1 stored no table config at all: the columns were re-derived from the spec. A v2 row
      // whose `settings` never got a `table.*` slice needs the same recovery — the API accepts
      // `settings: {}` (keys are deliberately open), so a chart written by anything but this
      // editor arrives that way, and reading only `settings` opened it on "add at least one
      // column or metric" with its columns sitting untouched in the spec.
      const fromSpec = {
        metrics: p.spec?.metrics ?? [],
        columns: [...(p.spec?.dimensions ?? []), ...(p.spec?.columns ?? [])],
      };
      if (isParamsV2(p)) {
        // `??` on the stored key, not on its emptiness: a metrics-only table stores
        // `table.columns: []` on purpose, and re-deriving would put the columns back.
        return {
          columns: (p.settings["table.columns"] as string[]) ?? fromSpec.columns,
          metrics: (p.settings["table.metrics"] as string[]) ?? fromSpec.metrics,
        };
      }
      return fromSpec;
    },
    toSettings: (cfg) => ({ "table.columns": cfg.columns, "table.metrics": cfg.metrics }),
    // A bare table is valid — with no metric it just lists raw rows — but an EMPTY one is not.
    // Returning [] unconditionally told the runner the chart was ready the moment a dataset was
    // picked, so it fired `columns: [], metrics: []` and the backend answered 400 twice before the
    // user had chosen anything. The preview now asks for a field instead of failing a query.
    requiredFields: (cfg) => (cfg.metrics.length > 0 || cfg.columns.length > 0 ? [] : ["at least one column or metric"]),
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => {
      const agg = cfg.metrics.length > 0;
      const dims = agg ? cfg.columns : [];
      return {
        metrics: cfg.metrics, dimensions: dims, columns: agg ? [] : cfg.columns,
        // The table's own header click sorts the rows ON SCREEN, within the page it already has.
        // `shared.orderby` is the other thing: it sorts in the QUERY, so it decides which rows come
        // back at all under `row_limit`. Both branches carry it — `orderby_sql` resolves a raw
        // column as readily as a dimension or a metric.
        filters: shared.filters, orderby: shared.orderby, row_limit: shared.rowLimit, rollup_groups: [],
        temporal_units: pruneUnits(shared.temporalUnits, dims),
      };
    },
    controls: [
      { id: "metrics", section: "data", render: TableMetricsControl },
      { id: "columns", section: "data", render: TableColumnsControl },
    ],
  };
}

export function objectDescriptor(): VizDescriptor<ObjectConfig> {
  return {
    key: "object",
    meta: CHART_TYPES.find((t) => t.key === "object")!,
    prefix: "object.",
    defaultConfig: { columns: [] },
    fromParams: (p) => {
      if (isParamsV2(p)) {
        // Falls back to `table.columns` so switching table→object keeps the selection,
        // which is what the shared `tableCols` state did before A2 — and then to the spec,
        // for the same reason the table descriptor does.
        const own = p.settings["object.columns"] as string[] | undefined;
        return { columns: own ?? (p.settings["table.columns"] as string[]) ?? p.spec?.columns ?? [] };
      }
      return { columns: p.spec?.columns ?? [] };
    },
    toSettings: (cfg) => ({ "object.columns": cfg.columns }),
    requiredFields: (cfg) => (cfg.columns.length ? [] : ["at least one column"]),
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => ({
      metrics: [], dimensions: [], columns: cfg.columns,
      filters: shared.filters, orderby: shared.orderby, row_limit: shared.rowLimit, rollup_groups: [],
    }),
    controls: [{ id: "fields", section: "data", render: ObjectFieldsControl }],
  };
}
