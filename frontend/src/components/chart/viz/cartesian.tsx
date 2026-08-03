import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { FieldZone } from "@/components/chart/config/FieldZone";
import {
  SeriesOptions, AxesOptions, OrientationOption, AxisRangeOptions, FormatOptions, GoalOptions, TrendOption,
} from "@/components/chart/settings/CartesianDisplaySection";
import { colorsForKeys } from "@/components/chart/echarts/keyColors";
import { SeriesListSection } from "@/components/chart/settings/SeriesListSection";
import { SeriesLineStyleSection } from "@/components/chart/settings/SeriesLineStyleSection";
import { cartesianSeriesNames, stackModeOf } from "@/components/chart/echarts/options/cartesian";
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

/** Metabase's "Bubble size": a third metric sizing each point. Only meaningful on a true
 *  metric-vs-metric scatter, so it hides until an x metric is chosen. */
function BubbleMetricControl({ config, onChange, metrics }: VizControlProps<CartesianConfig>) {
  if (!config.xMetric) return null;
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Bubble size</Label>
      <Select value={config.bubbleMetric ?? "__none"} onValueChange={(v) => onChange({ bubbleMetric: v === "__none" ? undefined : v })}>
        <SelectTrigger aria-label="Bubble size" className="h-8 text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="__none">None — every point the same size</SelectItem>
          {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
        </SelectContent>
      </Select>
    </div>
  );
}

/** Machine name -> display name, from the dataset's own `verbose_name`. */
function labelMap(items: { verbose_name?: string }[], nameOf: (i: never) => string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const i of items) {
    const v = i.verbose_name?.trim();
    if (v) out[nameOf(i as never)] = v;
  }
  return out;
}

function YAxisControl({ config, onChange, metrics }: VizControlProps<CartesianConfig>) {
  return (
    <FieldZone label="Y-axis" selected={config.yMetrics} onChange={(v) => onChange({ yMetrics: v })}
      labels={labelMap(metrics, (m: { metric_name: string }) => m.metric_name)}
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

// Series names come from the last result, not from anything precomputed — the shell must not know
// cartesian-specific shape.
function SeriesListControl({ kind, config, onChange, resultColumns, rows }: VizControlProps<CartesianConfig> & { kind: CartesianViz }) {
  const names = cartesianSeriesNames(resultColumns, rows, config);
  if (names.length === 0) return null;
  /**
   * The swatch each row shows before any override — `buildCartesianOption`'s own answer, asked with
   * the same function and the same keys, so the picker cannot offer a colour the chart does not
   * draw. (It used to re-derive a rule from the row's index and hand a lone series the summarize
   * green, which the renderer stopped drawing once colours keyed on the series' NAME.)
   *
   * ONE CALL FOR THE WHOLE LIST, hoisted out of the rows deliberately — `SeriesListSection` asks for
   * a default once per rendered row, so a per-row call would redo the sort, the hash and the ledger
   * N times per render, on every unrelated edit in the panel. `PieDisplaySection` hoists its own for
   * the same reason. It is also the honest shape: the assignment is a decision about the whole set
   * (a collision moves one key along, a pinned colour takes its slot out of circulation) and cannot
   * be made a series at a time.
   *
   * EVERY name goes in, hidden series included. `applySeriesOrder` drops the hidden ones AFTER
   * `colorsForKeys` has run, so hiding one does not move the rest — the opposite of the pie, whose
   * hidden slices are dropped before its ring is built. `config.seriesColors` goes in as `existing`
   * because the builder passes it there too: a pinned colour consumes its slot, so leaving it out
   * would disagree with the chart about every OTHER series the moment one was pinned.
   */
  const swatchColors = colorsForKeys(names, config.seriesColors);
  return (
    <SeriesListSection
      names={names}
      order={config.seriesOrder}
      hidden={config.hiddenSeries}
      colors={config.seriesColors}
      labels={config.seriesLabels}
      types={config.seriesTypes}
      // Only a combo lets a series choose; every other kind draws what its type says. Hidden while
      // stacked, as Metabase hides it — a stack of mixed shapes is not a thing anyone can read.
      defaultDisplay={kind === "combo" && stackModeOf({ kind, stacked: config.stacked, normalized: config.normalized }) === "none"
        ? (i) => (i === 0 ? "line" : "bar")
        : undefined}
      defaultColor={(name) => swatchColors[name]}
      onChange={onChange}
    />
  );
}

// Per-series dash style and width — declared for line/area only (see displayControls) and
// marked advanced there, so an unbounded per-series list of Selects hides behind "More
// options" instead of always taking panel space.
function SeriesLineStyleControl({ config, onChange, resultColumns, rows }: VizControlProps<CartesianConfig>) {
  const names = cartesianSeriesNames(resultColumns, rows, config);
  if (names.length === 0) return null;
  return <SeriesLineStyleSection names={names} seriesLine={config.seriesLine} onChange={onChange} />;
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
  const isLineish = kind === "line" || kind === "area";
  return [
    {
      id: "seriesOptions", section: "display", group: "Series",
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <SeriesOptions kind={kind} config={config} onChange={onChange} />
      ),
    },
    {
      id: "seriesList", section: "display", group: "Series",
      render: (p: VizControlProps<CartesianConfig>) => <SeriesListControl kind={kind} {...p} />,
    },
    ...(isLineish ? [{
      id: "seriesLineStyle", section: "display" as const, group: "Series", advanced: true,
      render: SeriesLineStyleControl,
    }] : []),
    ...(showOrientation ? [{
      id: "orientation", section: "display" as const, group: "Axes",
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <OrientationOption kind={kind} config={config} onChange={onChange} />
      ),
    }] : []),
    {
      id: "axes", section: "display", group: "Axes",
      // Same `resultColumns` lookup, and for the same reason, as the "range" control below — see
      // the comment there. Here it decides what the X-axis-scale trigger READS on a chart whose
      // scale was resolved rather than stored.
      render: ({ config, onChange, resultColumns }: VizControlProps<CartesianConfig>) => (
        <AxesOptions
          kind={kind} config={config} onChange={onChange}
          xColumnType={resultColumns.find((c) => c.name === config.xAxis)?.type}
        />
      ),
    },
    {
      id: "range", section: "display", group: "Axes", advanced: true,
      // `resultColumns`, not `columns`: the x-axis rotation rule inside turns on the type the
      // BUILDER will resolve against, and the builder reads the result's columns. The dataset's
      // own list is the same types by a different route until a chart is pointed at an expression
      // or the dataset drifts from the last run, at which point the two disagree and the result is
      // the one on screen.
      render: ({ config, onChange, resultColumns }: VizControlProps<CartesianConfig>) => (
        <AxisRangeOptions
          kind={kind} config={config} onChange={onChange}
          xColumnType={resultColumns.find((c) => c.name === config.xAxis)?.type}
        />
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
    ...(kind === "scatter" ? [
      { id: "xMetric", section: "data" as const, render: XMetricControl },
      { id: "bubbleMetric", section: "data" as const, render: BubbleMetricControl },
    ] : []),
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
      const mets = kind === "scatter" && cfg.xMetric
        ? [cfg.xMetric, ...cfg.yMetrics, ...(cfg.bubbleMetric ? [cfg.bubbleMetric] : [])]
        : cfg.yMetrics;
      return {
        metrics: mets, dimensions: dims, columns: [], filters: shared.filters,
        // The query asks for the order the chart draws in, so the ROWS are deterministic — not
        // just the axis. Without it the engine returns the groups in whatever order it likes, and
        // tooltips, drills, exports and whichever rows survive `row_limit` all disagree between
        // one load and the next.
        //
        // Ascending by the dimension, because that is Metabase's own default. A fresh
        // "summarise by" question carries no `order-by` in its MBQL at all; the query processor
        // adds one implicitly, `:asc`, on every breakout column before the query runs
        // (`add-implicit-breakout-order-by`, add_implicit_clauses.clj). Descending by the
        // aggregation is what a question's AUTHOR asks for, never what the builder emits. It is
        // also the order egret already chose in `pivot/engine.ts` and in smartscalar (`simple.tsx`).
        orderby: typeof cfg.xAxis === "string" && cfg.xAxis ? [{ field: cfg.xAxis, desc: false }] : [],
        row_limit: shared.rowLimit, rollup_groups: [],
        temporal_units: pruneUnits(shared.temporalUnits, dims),
      };
    },
    controls,
  };
}
