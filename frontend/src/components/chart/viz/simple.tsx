import { PieDisplaySection } from "@/components/chart/settings/PieDisplaySection";
import type { PieConfig } from "@/api/charts";
import { fieldDescriptor } from "./fields";
import type { VizControl, VizControlProps, VizDescriptor } from "./types";

type Cfg = Record<string, unknown>;

/** Distinct slice values of the current result, for the pie's per-slice colour controls. */
function PieDisplayControl({ config, onChange, resultColumns, rows }: VizControlProps<Cfg>) {
  const dimIdx = resultColumns.findIndex((c) => c.name === config.dimension);
  const slices = config.dimension && dimIdx !== -1
    ? Array.from(new Set(rows.map((r) => String((r as unknown[])[dimIdx] ?? "")).filter((v) => v !== "")))
    : [];
  return (
    <PieDisplaySection config={config as PieConfig} sliceNames={slices}
      onChange={(patch) => onChange(patch as Partial<Cfg>)} />
  );
}

const pieDisplay: VizControl<Cfg>[] = [{ id: "display", section: "display", render: PieDisplayControl }];

/**
 * The nine viz types whose configuration is just a handful of field pickers. Declared
 * through `fieldDescriptor` rather than written out, so adding one is a few lines of data.
 */
export const SIMPLE_DESCRIPTORS: VizDescriptor<Cfg>[] = [
  fieldDescriptor({
    key: "pie", prefix: "pie.", legacyKey: "pie", display: pieDisplay,
    fields: [
      { id: "dimension", label: "Dimension", kind: "dimension", required: "a dimension", temporal: true },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.dimension], [c.metric], s),
    legacyFromSpec: (spec) => ({ dimension: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "scalar", prefix: "scalar.", legacyKey: "scalar",
    fields: [{ id: "metric", label: "Metric", kind: "metric", required: "a metric" }],
    buildSpec: (c, s, h) => h.oneRow(c.metric, s),
    legacyFromSpec: (spec) => ({ metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "progress", prefix: "progress.", legacyKey: "progress",
    fields: [
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
      { id: "goal", label: "Goal", kind: "number", placeholder: "Target value" },
    ],
    buildSpec: (c, s, h) => h.oneRow(c.metric, s),
    legacyFromSpec: (spec) => ({ metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "gauge", prefix: "gauge.", legacyKey: "gauge",
    fields: [
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
      { id: "min", label: "Min", kind: "number", placeholder: "0" },
      { id: "max", label: "Max", kind: "number", placeholder: "auto" },
    ],
    buildSpec: (c, s, h) => h.oneRow(c.metric, s),
    legacyFromSpec: (spec) => ({ metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "funnel", prefix: "funnel.", legacyKey: "funnel",
    fields: [
      { id: "dimension", label: "Stage (dimension)", kind: "dimension", required: "a dimension", temporal: true },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.dimension], [c.metric], s),
    legacyFromSpec: (spec) => ({ dimension: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "waterfall", prefix: "waterfall.", legacyKey: "waterfall",
    fields: [
      { id: "xAxis", label: "X-axis (steps)", kind: "dimension", required: "an x-axis", temporal: true },
      { id: "metric", label: "Metric (delta)", kind: "metric", required: "a metric" },
      { id: "showTotal", label: "Show total bar", kind: "boolean" },
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.xAxis], [c.metric], s),
    legacyFromSpec: (spec) => ({ xAxis: spec.dimensions?.[0], metric: spec.metrics?.[0] }),
  }),
  fieldDescriptor({
    key: "smartscalar", prefix: "smartscalar.", legacyKey: "smartscalar",
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
    key: "boxplot", prefix: "boxplot.", legacyKey: "boxplot",
    fields: [
      { id: "category", label: "Category", kind: "dimension", required: "a category", placeholder: "Pick a column…" },
      { id: "value", label: "Value column", kind: "dimension", required: "a value column", placeholder: "Pick a numeric column…" },
    ],
    // Raw rows, no aggregation — the quartiles are computed client-side.
    buildSpec: (c, s, h) => h.rawColumns([c.category, c.value], s),
    legacyFromSpec: (spec) => ({ category: spec.columns?.[0], value: spec.columns?.[1] }),
  }),
  fieldDescriptor({
    key: "sankey", prefix: "sankey.", legacyKey: "sankey",
    fields: [
      { id: "source", label: "Source", kind: "dimension", required: "a source" },
      { id: "target", label: "Target", kind: "dimension", required: "a target", excludes: "source" },
      { id: "metric", label: "Metric", kind: "metric", required: "a metric" },
    ],
    buildSpec: (c, s, h) => h.dimsAndMetrics([c.source, c.target], [c.metric], s),
    legacyFromSpec: (spec) => ({ source: spec.dimensions?.[0], target: spec.dimensions?.[1], metric: spec.metrics?.[0] }),
  }),
];
