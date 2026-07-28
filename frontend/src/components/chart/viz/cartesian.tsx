import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { FieldZone } from "@/components/chart/config/FieldZone";
import {
  SeriesOptions, AxesOptions, OrientationOption, AxisRangeOptions, FormatOptions, GoalOptions, TrendOption,
} from "@/components/chart/settings/CartesianDisplaySection";
import { SeriesOrderSection } from "@/components/chart/settings/SeriesOrderSection";
import { cartesianSeriesNames } from "@/components/chart/echarts/options/cartesian";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from "@/components/ui/select";
import type { CartesianConfig, CartesianViz, QuerySpec } from "@/api/charts";
import { cartesianFromParams, cartesianToSettings, CARTESIAN_PREFIX } from "./cartesian-codec";
import type { SharedQueryState, VizControl, VizControlProps, VizDescriptor } from "./types";

const LABEL_CLASS = "text-xs font-medium uppercase tracking-wide text-muted-foreground";

function XAxisControl({ config, onChange, columns, unitRow }: VizControlProps<CartesianConfig>) {
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>X-axis</Label>
        <Select value={config.xAxis ?? ""} onValueChange={(v) => onChange({ xAxis: v || undefined })}>
          <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
          <SelectContent>
            {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      {unitRow(config.xAxis)}
    </>
  );
}

function XMetricControl({ config, onChange, metrics }: VizControlProps<CartesianConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>X metric (optional)</Label>
      <Select value={config.xMetric ?? "__none"} onValueChange={(v) => onChange({ xMetric: v === "__none" ? undefined : v })}>
        <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="__none">None — use the dimension</SelectItem>
          {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
        </SelectContent>
      </Select>
    </div>
  );
}

function YAxisControl({ config, onChange, metrics }: VizControlProps<CartesianConfig>) {
  return (
    <FieldZone label="Y-axis" selected={config.yMetrics} onChange={(v) => onChange({ yMetrics: v })}
      options={metrics.map((m) => m.metric_name)} />
  );
}

function SeriesControl({ config, onChange, columns }: VizControlProps<CartesianConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Series</Label>
      <Select value={config.series ?? "__none"} onValueChange={(v) => onChange({ series: v === "__none" ? undefined : v })}>
        <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="__none">None</SelectItem>
          {columns.filter((c) => c.column_name !== config.xAxis)
            .map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
        </SelectContent>
      </Select>
      {config.series && config.yMetrics.length > 1 && (
        <p className="text-xs text-muted-foreground">With a series, only the first metric is plotted.</p>
      )}
    </div>
  );
}

function MaxCategoriesControl({ config, onChange }: VizControlProps<CartesianConfig>) {
  return (
    <div className="space-y-1">
      <Label htmlFor="cz-maxcat" className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
        Max categories
      </Label>
      <Input id="cz-maxcat" type="number" min={2} className="h-8 text-sm"
        placeholder="Keep every category"
        value={config.maxCategories ?? ""}
        onChange={(e) => {
          const n = Number(e.target.value);
          onChange({ maxCategories: e.target.value === "" || Number.isNaN(n) ? undefined : n });
        }} />
      <p className="text-xs text-muted-foreground">The rest are summed into “Other”.</p>
    </div>
  );
}

// Series list draws its names the same way the per-series colour/label control does: from the
// last result, not from anything precomputed — the shell must not know cartesian-specific shape.
function SeriesOrderControl({ config, onChange, resultColumns, rows }: VizControlProps<CartesianConfig>) {
  const names = cartesianSeriesNames(resultColumns, rows, config);
  if (names.length === 0) return null;
  return (
    <SeriesOrderSection names={names} order={config.seriesOrder} hidden={config.hiddenSeries} onChange={onChange} />
  );
}

/**
 * The Display tab's controls, grouped under named headings and scoped to what this kind
 * actually plots — bar gets orientation and stacking, line does not get orientation.
 */
function displayControls(kind: CartesianViz): VizControl<CartesianConfig>[] {
  // Row is already the horizontal cartesian kind — it has no orientation to choose, so unlike
  // OrientationOption's internal isBar gate (which just renders nothing for row), the control
  // itself is not declared for row at all.
  const showOrientation = kind === "bar";
  return [
    {
      id: "seriesOptions", section: "display", group: "Series",
      render: ({ config, onChange, resultColumns, rows }: VizControlProps<CartesianConfig>) => (
        <SeriesOptions kind={kind} config={config} onChange={onChange}
          seriesNames={cartesianSeriesNames(resultColumns, rows, config)} />
      ),
    },
    { id: "seriesOrder", section: "display", group: "Series", render: SeriesOrderControl },
    ...(showOrientation ? [{
      id: "orientation", section: "display" as const, group: "Axes",
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <OrientationOption kind={kind} config={config} onChange={onChange} />
      ),
    }] : []),
    {
      id: "axes", section: "display", group: "Axes",
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <AxesOptions kind={kind} config={config} onChange={onChange} />
      ),
    },
    {
      id: "range", section: "display", group: "Axes", advanced: true,
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <AxisRangeOptions kind={kind} config={config} onChange={onChange} />
      ),
    },
    {
      id: "format", section: "display", group: "Format",
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <FormatOptions kind={kind} config={config} onChange={onChange} />
      ),
    },
    {
      id: "goal", section: "display", group: "Format", advanced: true,
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <GoalOptions kind={kind} config={config} onChange={onChange} />
      ),
    },
    {
      id: "trend", section: "display", group: "Format", advanced: true,
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <TrendOption kind={kind} config={config} onChange={onChange} />
      ),
    },
  ];
}

export function cartesianDescriptor(kind: CartesianViz): VizDescriptor<CartesianConfig> {
  const controls: VizControl<CartesianConfig>[] = [
    { id: "x", section: "data", render: XAxisControl },
    ...(kind === "scatter" ? [{ id: "xMetric", section: "data" as const, render: XMetricControl }] : []),
    { id: "y", section: "data", render: YAxisControl },
    { id: "series", section: "data", render: SeriesControl },
    // Scatter's x-axis is a metric, not a category to fold into "Other" — nothing to bind
    // maxCategories to there, and the fold code is never reached for a metric-vs-metric plot.
    ...(kind !== "scatter" ? [{ id: "maxCategories", section: "data" as const, render: MaxCategoriesControl }] : []),
    ...displayControls(kind),
  ];

  return {
    key: kind,
    meta: CHART_TYPES.find((t) => t.key === kind)!,
    prefix: CARTESIAN_PREFIX,
    defaultConfig: { yMetrics: [] },
    fromParams: cartesianFromParams,
    toSettings: cartesianToSettings,
    requiredFields: (cfg) => {
      const out: string[] = [];
      if (!cfg.xAxis) out.push("X-axis");
      if (!cfg.yMetrics.length) out.push("a metric");
      return out;
    },
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => {
      const dims = [cfg.xAxis, cfg.series].filter((d): d is string => !!d);
      const mets = kind === "scatter" && cfg.xMetric ? [cfg.xMetric, ...cfg.yMetrics] : cfg.yMetrics;
      return {
        metrics: mets, dimensions: dims, columns: [], filters: shared.filters, orderby: [],
        row_limit: shared.rowLimit, rollup_groups: [],
        temporal_units: pruneUnits(shared.temporalUnits, dims),
      };
    },
    controls,
  };
}
