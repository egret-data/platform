import { PieDisplaySection } from "@/components/chart/settings/PieDisplaySection";
import { GaugeSegmentsSection } from "@/components/chart/settings/GaugeSegmentsSection";
import { ScalarDisplaySection } from "@/components/chart/settings/ScalarDisplaySection";
import { WaterfallBarsSection, WaterfallFormatSection } from "@/components/chart/settings/WaterfallDisplaySection";
import { ChartAxesSection } from "@/components/chart/settings/ChartAxesSection";
import { BoxplotDisplaySection } from "@/components/chart/settings/BoxplotDisplaySection";
import { SankeyDisplaySection } from "@/components/chart/settings/SankeyDisplaySection";
import { AXES_GROUP } from "@/components/chart/StudioPanel";
import type { BoxplotConfig, GaugeSegment, PieConfig, SankeyConfig, ScalarConfig, WaterfallConfig } from "@/api/charts";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SERIES_PALETTE } from "@/components/chart/echarts/palette";
import { pieSliceRows } from "@/components/chart/echarts/options/pie";

/** Metabase’s `color("accent1")`, the progress bar’s default. */
const PROGRESS_DEFAULT_COLOR = SERIES_PALETTE[1];
import { Switch } from "@/components/ui/switch";
import { SeriesListSection } from "@/components/chart/settings/SeriesListSection";
import { fieldDescriptor } from "./fields";
import type { VizControl, VizControlProps, VizDescriptor } from "./types";

type Cfg = Record<string, unknown>;

/** Ring 0's slices of the current result, for the pie's per-slice controls.
 *
 * Derived by `pieSliceRows`, the pie builder's OWN pass, so the list reads in the order the chart
 * draws and knows which slices the "Other" fold swallowed. It used to be the distinct raw values of
 * the first dimension in row order, with no measure — a list the chart never draws (see
 * `pieSliceRows` for what that cost).
 *
 * Reads the FIRST picked dimension only: the innermost ring is what `sliceColors`/`sliceNames` key
 * against, matching Metabase's `pie.rows`.
 *
 * `dimensions` is stored settings rather than typed input, so a pre-migration row can hand back a
 * bare string — the same guard the descriptor's `buildSpec` makes, for the same reason. */
function PieDisplayControl({ config, onChange, resultColumns, rows }: VizControlProps<Cfg>) {
  const dimensions = Array.isArray(config.dimensions) ? (config.dimensions as string[]) : [];
  const sliceRows = pieSliceRows(resultColumns, rows, { ...(config as PieConfig), dimensions });
  return (
    <PieDisplaySection
      config={config as PieConfig}
      // The empty slice (`aggregateRing` collapses null, undefined and "" into one) is left out, as
      // it always has been: it has no name to label a row with. It still takes part in the pass
      // above, so it counts towards the total the fold is judged against, exactly as it does in the
      // chart.
      slices={sliceRows.map((s) => s.key).filter((k) => k !== "")}
      foldedSlices={sliceRows.filter((s) => s.folded).map((s) => s.key).filter((k) => k !== "")}
      onChange={(patch) => onChange(patch as Partial<Cfg>)} />
  );
}

const pieDisplay: VizControl<Cfg>[] = [{ id: "display", section: "display", render: PieDisplayControl }];

/** The gauge's current value, derived the same way `buildGaugeOption` does, so the editor's
 * default three zones (when nothing is stored) size themselves to what actually renders. */
function GaugeDisplayControl({ config, onChange, resultColumns, rows }: VizControlProps<Cfg>) {
  const mi = typeof config.metric === "string" ? resultColumns.findIndex((c) => c.name === config.metric) : -1;
  const raw = mi >= 0 && rows.length > 0 ? (rows[0] as unknown[])[mi] : undefined;
  const value = raw != null && Number.isFinite(Number(raw)) ? Number(raw) : 0;
  return (
    <GaugeSegmentsSection
      segments={config.segments as GaugeSegment[] | undefined}
      value={value}
      onChange={(segments) => onChange({ segments } as Partial<Cfg>)}
    />
  );
}

const gaugeDisplay: VizControl<Cfg>[] = [{ id: "display", section: "display", render: GaugeDisplayControl }];

/** The trend's two Display switches, both Metabase's. */
function TrendDisplayControl({ config, onChange }: VizControlProps<Cfg>) {
  const row = (id: string, label: string, hint: string, key: string) => (
    <div className="space-y-0.5">
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
        <Switch id={id} aria-label={label} checked={!!config[key]}
          onCheckedChange={(v) => onChange({ [key]: v || undefined } as Partial<Cfg>)} />
      </div>
      <p className="text-xs text-muted-foreground">{hint}</p>
    </div>
  );
  return (
    <div className="space-y-2.5">
      {row("ss-switch", "Switch positive / negative colours",
        "For a metric where less is better — cost, churn, time to resolve.", "switchPositiveNegative")}
      {row("ss-compact", "Compact number", "36,513 becomes 36.5k.", "compact")}
    </div>
  );
}

const trendDisplay: VizControl<Cfg>[] = [{ id: "display", section: "display", render: TrendDisplayControl }];

/** The progress bar's colour and its goal-from-a-column, both Metabase's. */
function ProgressDisplayControl({ config, onChange, metrics }: VizControlProps<Cfg>) {
  const color = (config.color as string | undefined) ?? PROGRESS_DEFAULT_COLOR;
  return (
    <div className="space-y-2.5">
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor="pg-color" className="text-sm font-normal">Colour</Label>
        <input
          id="pg-color" type="color" aria-label="Colour"
          className="h-7 w-8 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
          value={color}
          onChange={(e) => onChange({ color: e.target.value } as Partial<Cfg>)}
        />
      </div>
      <div className="space-y-1">
        <Label className="text-sm font-normal">Goal from a metric</Label>
        <Select
          value={(config.goalColumn as string | undefined) ?? "__none"}
          onValueChange={(v) => onChange({ goalColumn: v === "__none" ? undefined : v } as Partial<Cfg>)}
        >
          <SelectTrigger aria-label="Goal from a metric" className="h-8 text-sm"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="__none">None — use the fixed goal</SelectItem>
            {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">A metric whose value is the target, instead of a number typed once.</p>
      </div>
    </div>
  );
}

const progressDisplay: VizControl<Cfg>[] = [{ id: "display", section: "display", render: ProgressDisplayControl }];

/** The funnel's steps, in order, each hideable — Metabase's `funnel.rows`. Names come from the
 *  last result rather than anything precomputed, the same way the cartesian series list works. */
function FunnelStepsControl({ config, onChange, resultColumns, rows }: VizControlProps<Cfg>) {
  const dim = typeof config.dimension === "string" ? config.dimension : undefined;
  const di = dim ? resultColumns.findIndex((c) => c.name === dim) : -1;
  if (di < 0) return null;
  const names = [...new Set(rows.map((r) => String((r as unknown[])[di] ?? "")))].filter((n) => n !== "");
  if (names.length === 0) return null;
  return (
    <div className="space-y-1.5">
      <div className="text-xs text-muted-foreground">Steps</div>
      <SeriesListSection
        names={names}
        order={config.stepOrder as string[] | undefined}
        hidden={config.hiddenSteps as string[] | undefined}
        renameable={false}
        onChange={(patch) => onChange({
          ...(patch.seriesOrder ? { stepOrder: patch.seriesOrder } : {}),
          ...(patch.hiddenSeries ? { hiddenSteps: patch.hiddenSeries } : {}),
        } as Partial<Cfg>)}
      />
      <p className="text-xs text-muted-foreground">
        A funnel reads left to right in this order — it is not re-sorted by size.
      </p>
    </div>
  );
}

/** Metabase's `funnel.type`. */
function FunnelTypeControl({ config, onChange }: VizControlProps<Cfg>) {
  return (
    <div className="space-y-1">
      <Label className="text-sm font-normal">Funnel type</Label>
      <Select value={(config.type as string | undefined) ?? "funnel"}
        onValueChange={(v) => onChange({ type: v === "funnel" ? undefined : v } as Partial<Cfg>)}>
        <SelectTrigger aria-label="Funnel type" className="h-8 text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="funnel">Funnel — one narrowing band</SelectItem>
          <SelectItem value="bar">Bar — a column per step</SelectItem>
        </SelectContent>
      </Select>
    </div>
  );
}

// The steps list sits under DATA, beside the column that produces them, and only the funnel type
// is a display choice — Metabase's own split, checked in its panel.
const funnelDisplay: VizControl<Cfg>[] = [
  { id: "steps", section: "data", render: FunnelStepsControl },
  { id: "type", section: "display", render: FunnelTypeControl },
];

/** Metabase's waterfall has the same Data | Display | Axes panel its cartesian charts do; egret's
 *  had no Display or Axes controls at all. `group: "Axes"` is the shell's cue to give that group
 *  its own tab. */
const waterfallDisplay: VizControl<Cfg>[] = [
  {
    id: "bars", section: "display", group: "Bars",
    render: ({ config, onChange }: VizControlProps<Cfg>) => (
      <WaterfallBarsSection config={config as WaterfallConfig} onChange={onChange as (p: Partial<WaterfallConfig>) => void} />
    ),
  },
  {
    id: "format", section: "display", group: "Format",
    render: ({ config, onChange }: VizControlProps<Cfg>) => (
      <WaterfallFormatSection config={config as WaterfallConfig} onChange={onChange as (p: Partial<WaterfallConfig>) => void} />
    ),
  },
  {
    id: "axes", section: "display", group: AXES_GROUP,
    render: ({ config, onChange }: VizControlProps<Cfg>) => (
      <ChartAxesSection idPrefix="wf" config={config as WaterfallConfig}
        onChange={onChange as (p: Partial<WaterfallConfig>) => void} />
    ),
  },
];

/** The sankey's panel. Metabase gives it Data | Display — no Axes, because a sankey has none. */
const sankeyDisplay: VizControl<Cfg>[] = [
  {
    id: "display", section: "display",
    render: ({ config, onChange }: VizControlProps<Cfg>) => (
      <SankeyDisplaySection config={config as SankeyConfig}
        onChange={onChange as (p: Partial<SankeyConfig>) => void} />
    ),
  },
];

/** The box plot's panel. Metabase gives it Data | Display | Axes, the same three tabs its waterfall
 *  has; egret's had two fields and nothing else. The Axes tab is the SHARED one — a box plot's axes
 *  are the same shape as a waterfall's, so they get the same controls rather than a second copy. */
const boxplotDisplay: VizControl<Cfg>[] = [
  {
    id: "display", section: "display",
    render: ({ config, onChange }: VizControlProps<Cfg>) => (
      <BoxplotDisplaySection config={config as BoxplotConfig}
        onChange={onChange as (p: Partial<BoxplotConfig>) => void} />
    ),
  },
  {
    id: "axes", section: "display", group: AXES_GROUP,
    render: ({ config, onChange }: VizControlProps<Cfg>) => (
      <ChartAxesSection idPrefix="bp" config={config as BoxplotConfig}
        onChange={onChange as (p: Partial<BoxplotConfig>) => void} />
    ),
  },
];

/** The scalar's captions and conditional-colour bands. Unlike the gauge's, this control needs
 * nothing from the result — a band is compared against whatever value renders, not against the
 * columns — so it takes only the config. */
function ScalarDisplayControl({ config, onChange }: VizControlProps<Cfg>) {
  return (
    <ScalarDisplaySection
      config={config as ScalarConfig}
      onChange={(patch) => onChange(patch as Partial<Cfg>)}
    />
  );
}

const scalarDisplay: VizControl<Cfg>[] = [{ id: "display", section: "display", render: ScalarDisplayControl }];

/**
 * The nine viz types whose configuration is just a handful of field pickers. Declared
 * through `fieldDescriptor` rather than written out, so adding one is a few lines of data.
 */
export const SIMPLE_DESCRIPTORS: VizDescriptor<Cfg>[] = [
  fieldDescriptor({
    key: "pie", prefix: "pie.", legacyKey: "pie", display: pieDisplay,
    fields: [
      { id: "dimensions", label: "Dimensions", kind: "dimensions", max: 3, required: "a dimension", temporal: true },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    // `c.dimensions` is settings data, not typed input — a stored string/number/object
    // (pre-migration rows, or anything not yet whitelisted server-side) must not reach
    // `strs()`, which assumes an array and throws on `.map`. Every sibling descriptor is
    // safe here only because it wraps a single field in an array literal; pie is the one
    // descriptor that forwards a stored value directly, so it needs its own guard.
    buildSpec: (c, s, h) => h.dimsAndMetrics(Array.isArray(c.dimensions) ? c.dimensions : [], [c.metric], s),
    legacyFromSpec: (spec) => ({ dimensions: spec.dimensions, metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "scalar", prefix: "scalar.", legacyKey: "scalar", display: scalarDisplay,
    fields: [{ id: "metric", label: "Metric", kind: "metric", required: "a metric" }],
    // A goal read from a metric has to be fetched alongside the value, in the same single row.
    buildSpec: (c, s, h) => {
      const base = h.oneRow(c.metric, s);
      const gc = typeof c.goalColumn === "string" ? c.goalColumn : undefined;
      return gc && gc !== c.metric ? { ...base, metrics: [...base.metrics, gc] } : base;
    },
    legacyFromSpec: (spec) => ({ metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "progress", prefix: "progress.", legacyKey: "progress", display: progressDisplay,
    fields: [
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
      { id: "goal", label: "Goal", kind: "number", placeholder: "Target value" },
    ],
    // A goal read from a metric has to be fetched alongside the value, in the same single row.
    buildSpec: (c, s, h) => {
      const base = h.oneRow(c.metric, s);
      const gc = typeof c.goalColumn === "string" ? c.goalColumn : undefined;
      return gc && gc !== c.metric ? { ...base, metrics: [...base.metrics, gc] } : base;
    },
    legacyFromSpec: (spec) => ({ metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "gauge", prefix: "gauge.", legacyKey: "gauge", display: gaugeDisplay,
    fields: [
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    // A goal read from a metric has to be fetched alongside the value, in the same single row.
    buildSpec: (c, s, h) => {
      const base = h.oneRow(c.metric, s);
      const gc = typeof c.goalColumn === "string" ? c.goalColumn : undefined;
      return gc && gc !== c.metric ? { ...base, metrics: [...base.metrics, gc] } : base;
    },
    legacyFromSpec: (spec) => ({ metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "funnel", prefix: "funnel.", legacyKey: "funnel", display: funnelDisplay,
    fields: [
      { id: "dimension", label: "Stage (dimension)", kind: "dimension", required: "a dimension", temporal: true },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.dimension], [c.metric], s),
    legacyFromSpec: (spec) => ({ dimension: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "waterfall", prefix: "waterfall.", legacyKey: "waterfall", display: waterfallDisplay,
    fields: [
      { id: "xAxis", label: "X-axis (steps)", kind: "dimension", required: "an x-axis", temporal: true },
      { id: "metric", label: "Metric (delta)", kind: "metric", required: "a metric" },
      // "Show total" is a DISPLAY choice and now lives there, beside the colour it paints the bar.
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.xAxis], [c.metric], s),
    legacyFromSpec: (spec) => ({ xAxis: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "smartscalar", prefix: "smartscalar.", legacyKey: "smartscalar", display: trendDisplay,
    fields: [
      { id: "dimension", label: "Time / order dimension", kind: "dimension", required: "a time dimension", temporal: true },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    // Trend compares the last two periods, so the rows must come back in dimension order.
    buildSpec: (c, s, h) => ({
      ...h.dimsAndMetrics([c.dimension], [c.metric], s),
      orderby: typeof c.dimension === "string" && c.dimension ? [{ field: c.dimension, desc: false }] : [],
    }),
    legacyFromSpec: (spec) => ({ dimension: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "boxplot", prefix: "boxplot.", legacyKey: "boxplot", display: boxplotDisplay,
    fields: [
      { id: "category", label: "Category", kind: "dimension", required: "a category", placeholder: "Pick a column…" },
      { id: "value", label: "Value column", kind: "dimension", required: "a value column", placeholder: "Pick a numeric column…" },
    ],
    // Raw rows, no aggregation — the quartiles are computed client-side.
    buildSpec: (c, s, h) => h.rawColumns([c.category, c.value], s),
    legacyFromSpec: (spec) => ({ category: spec.columns?.[0], value: spec.columns?.[1] }),
  }),
  fieldDescriptor({
    key: "sankey", prefix: "sankey.", legacyKey: "sankey", display: sankeyDisplay,
    fields: [
      { id: "source", label: "Source", kind: "dimension", required: "a source" },
      { id: "target", label: "Target", kind: "dimension", required: "a target", excludes: "source" },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.source, c.target], [c.metric], s),
    legacyFromSpec: (spec) => ({ source: spec.dimensions?.[0], target: spec.dimensions?.[1], metric: spec.metrics?.[0] }),
  }),
];
