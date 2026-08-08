import type { ReactNode } from "react";
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

/** Machine name -> display name, from the dataset's own `verbose_name`. Datasets have offered
 * these for a long time and the chart studio ignored them, so a column named "Total Sales" still
 * showed as `total_revenue` everywhere a field is picked. */
function columnLabels(columns: { column_name: string; verbose_name?: string }[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const c of columns) if (c.verbose_name) out[c.column_name] = c.verbose_name;
  return out;
}
function metricLabels(metrics: { metric_name: string; verbose_name?: string }[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const m of metrics) if (m.verbose_name) out[m.metric_name] = m.verbose_name;
  return out;
}

/**
 * A zone's grouping controls sit UNDER THAT ZONE.
 *
 * Both zones' unit rows used to be emitted together at the bottom of `ColsControl`, so a temporal
 * field dropped in ROWS got its grouping control rendered after the whole COLUMNS zone — seen live
 * on the kind deployment, where a pivot with `order_month` in Rows read:
 *
 *     ROWS      order_month ×          COLUMNS   Add columns…
 *               Add rows…              UNIT · ORDER_MONTH   [Month]
 *                                      VALUES    Add values…
 *
 * — the control two headings away from the only field it configures. The comment that put them
 * there was solving the right problem (two date dimensions produced two identical "UNIT" rows) with
 * the wrong half of the fix: naming the field was necessary, batching them was not, and the batch
 * is what moved the control away from its field. Naming stays; the batch goes.
 */
function unitRowsFor(fields: string[], unitRow: (col?: string) => ReactNode) {
  return fields.map((c) => <div key={c}>{unitRow(c)}</div>);
}

function RowsControl({ config, onChange, columns, unitRow }: VizControlProps<PivotConfig>) {
  return (
    <>
      <FieldZone label="Rows" selected={config.rows} onChange={(v) => onChange({ rows: v })}
        labels={columnLabels(columns)}
        options={columns.map((c) => c.column_name).filter((n) => !config.cols.includes(n))} />
      {unitRowsFor(config.rows, unitRow)}
    </>
  );
}

function ColsControl({ config, onChange, columns, unitRow }: VizControlProps<PivotConfig>) {
  return (
    <>
      <FieldZone label="Columns" selected={config.cols} onChange={(v) => onChange({ cols: v })}
        labels={columnLabels(columns)}
        options={columns.map((c) => c.column_name).filter((n) => !config.rows.includes(n))} />
      {unitRowsFor(config.cols, unitRow)}
    </>
  );
}

function ValuesControl({ config, onChange, metrics }: VizControlProps<PivotConfig>) {
  return (
    <FieldZone label="Values" selected={config.metrics} onChange={(v) => onChange({ metrics: v })}
      labels={metricLabels(metrics)}
      options={metrics.map((m) => m.metric_name)} />
  );
}

function TotalsControl({ config, onChange, columns }: VizControlProps<PivotConfig>) {
  const row = (id: string, label: string, checked: boolean, key: keyof PivotConfig, hint?: string) => (
    <div className="space-y-0.5">
      <div className="flex items-center justify-between">
        <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
        <Switch id={id} aria-label={label} checked={checked}
          onCheckedChange={(v) => onChange({ [key]: v } as Partial<PivotConfig>)} />
      </div>
      {/* Persistent helper text rather than a tooltip: these three read as near-synonyms, and
          "Condense duplicate totals" is the one nobody can guess from its name. Metabase explains
          it too, behind an info icon; saying it outright costs one line. */}
      {hint && <p className="pr-10 text-xs text-muted-foreground">{hint}</p>}
    </div>
  );
  // Only a dimension that has one nested beneath it produces a subtotal band; the innermost one's
  // groups are leaves. Listing it would offer a switch that does nothing.
  const subtotalDims = config.rows.slice(0, -1);
  const colTotalsOn = config.showColTotals ?? true;
  const labels = columnLabels(columns);

  return (
    <div className="space-y-1">
      <SectionLabel>Totals</SectionLabel>
      <div className="space-y-2.5 pt-1.5">
        {/* "Show …", the way Metabase titles `pivot.show_row_totals` / `pivot.show_column_totals`
            (`PivotTable/settings.ts`) and the way the third switch in this very group already read.
            A bare noun next to a toggle reads as a heading for the row rather than as the thing the
            toggle turns on, which is why two of these three needed the verb and one did not. */}
        {row("show-row-totals", "Show row totals", config.showRowTotals ?? true, "showRowTotals",
          "A Total column down the right-hand side.")}
        {row("show-col-totals", "Show column totals", colTotalsOn, "showColTotals",
          "Subtotal bands per group, and a Grand totals row at the bottom.")}
        {row("condense-totals", "Condense duplicate totals", config.condense ?? true, "condense",
          "Hides a subtotal row when its group has only one row, so the two would read the same.")}

        {colTotalsOn && subtotalDims.length > 0 && (
          <div className="space-y-2 border-t pt-2.5">
            <div className="text-xs text-muted-foreground">Subtotals for</div>
            {subtotalDims.map((name) => (
              <div key={name} className="flex items-center justify-between">
                <Label htmlFor={`subtotal-${name}`} className="truncate text-sm font-normal" title={name}>
                  {labels[name]?.trim() || name}
                </Label>
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
        // `shared.orderby` verbatim, and no implicit default of its own: `pivot/engine.ts`'s
        // `sortTree` already orders every level client-side, so deriving one here would only
        // change which rows survive `row_limit` under a tree that re-sorts them anyway.
        metrics: cfg.metrics, dimensions: dims, columns: [], filters: shared.filters, orderby: shared.orderby,
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
