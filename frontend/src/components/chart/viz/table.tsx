import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { FIELD_LABEL as LABEL_CLASS } from "@/components/chart/settings/FieldLabel";
import { Label } from "@/components/ui/label";
import { CheckList } from "@/components/chart/config/CheckList";
import { FieldZone } from "@/components/chart/config/FieldZone";
import { isParamsV2 } from "@/api/charts";
import type { QuerySpec } from "@/api/charts";
import { columnLabels, metricLabels } from "@/components/chart/config/field-names";
import type { SharedQueryState, VizControlProps, VizDescriptor } from "./types";

export interface TableConfig { columns: string[]; metrics: string[] }
export interface ObjectConfig { columns: string[] }


const toggle = (list: string[], name: string) =>
  list.includes(name) ? list.filter((x) => x !== name) : [...list, name];

function TableMetricsControl({ config, onChange, metrics }: VizControlProps<TableConfig>) {
  return (
    <div className="space-y-1">
      {/* ONE LINE, NOT THREE. This block used to open with a two-line instruction and then, on the
          common case of a dataset that defines no metrics, an italic "No metrics on this dataset."
          — four lines of prose before the reader reached anything they could act on. The sentence
          still has to be said (a table with a metric aggregates and one without does not, and
          nothing else on screen says so), so it is said once and short. */}
      {/* `FieldZone`, not the checkbox list this used to be — the same card rows, grip and dashed
          add-row the column list beside it now uses. The two lists answer the same kind of question
          ("which fields does this table select") and sat one heading apart looking like different
          kinds of control. `FieldZone` is the pivot's own component, so this is a third call site
          rather than a third spelling. */}
      <FieldZone
        label="Metrics"
        selected={config.metrics}
        options={metrics.map((m) => m.metric_name)}
        onChange={(next) => onChange({ metrics: next })}
        labels={metricLabels(metrics)}
      />
      {/* AFTER the zone, not before it. `FieldZone` draws its own "Metrics" heading, so a sentence
          above the component landed above that heading — the explanation arriving before the thing
          it explains. */}
      <p className="text-xs text-muted-foreground">With none, the table lists raw rows.</p>
    </div>
  );
}

/**
 * THE COLUMN CHECKLIST THAT USED TO BE HERE IS GONE, and only the temporal bucketing is left.
 *
 * It listed the same fields, written to the same `config.columns` array, as the Display tab's
 * `TableColumnsSection` — whose own header describes folding three parallel lists into one, and
 * whose `Add or remove columns` panel is built from THIS EXACT ARRAY (`ChartExplore.tsx`'s
 * `tableQueryFields`, whose comment already called them "the same fields, one click apart"). The
 * fold shipped; this list was simply never removed.
 *
 * Display keeps the list rather than Data because `CardVizPanel` — a dashboard card's editor —
 * renders `SharedDisplaySections` and has NO Data tab, and the table declares nothing
 * `cardOverridable`. Moving the list here would take hiding, reordering and per-column formatting
 * away from every dashboard card.
 *
 * WHAT STAYS, AND WHY IT STAYS ON THIS TAB. The bucket picker changes the SQL — it is the query's
 * `temporal_units`, not a presentation choice — so it belongs beside Metrics, not beside the
 * column list. It is offered only once the table aggregates, because on a raw row list there is
 * no grouping for a unit to apply to.
 */
function TableGroupingControl({ config, unitRow }: VizControlProps<TableConfig>) {
  if (config.metrics.length === 0) return null;
  return <>{config.columns.map((c) => <div key={c}>{unitRow(c)}</div>)}</>;
}

function ObjectFieldsControl({ config, onChange, columns }: VizControlProps<ObjectConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Fields</Label>
      {/* The object-detail viz picks from the same dataset columns through the same widget, so it
          gets the same names — it was never a separate decision, only a separate call site. */}
      <CheckList idPrefix="obj" empty="Select a dataset to see columns."
        options={columns.map((c) => c.column_name)} selected={config.columns}
        labels={columnLabels(columns)}
        onToggle={(n) => onChange({ columns: toggle(config.columns, n) })}
        onSetAll={(next) => onChange({ columns: next })} />
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
      { id: "grouping", section: "data", render: TableGroupingControl },
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
