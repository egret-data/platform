import type { ColumnFormat } from "@/components/chart/format-types";
import { parseFormat } from "@/components/dataset/FormatPicker";
import type { DatasetColumn, DatasetMetric } from "@/api/datasets";

/**
 * The dataset's semantic layer as per-column render defaults, keyed by column (or metric) NAME:
 * its format, the rendering its role implies, and its DISPLAY NAME as `column_title`.
 *
 * The display name is what makes a table header and the Preview say "Order size" rather than
 * `order_size` (KI-103), the same name the chart editor's field lists already showed. It is a
 * render-time default only: callers layer it under the chart's own settings and never save it, so
 * renaming a column in the dataset stays live in every chart, and a chart's own column title wins.
 * The data stays keyed by the column name.
 */
export function datasetColumnDefaults(columns: DatasetColumn[], metrics: DatasetMetric[] = []): Record<string, ColumnFormat> {
  const m: Record<string, ColumnFormat> = {};
  for (const c of columns) {
    const cf: ColumnFormat = { ...parseFormat(c.format) };
    // Semantic type gives a default rendering when the column has no explicit format.
    if (!cf.view_as) {
      if (c.semantic_type === "email") cf.view_as = "email";
      else if (c.semantic_type === "url") cf.view_as = "link";
      else if (c.semantic_type === "image") cf.view_as = "image";
    }
    if (c.semantic_type === "currency" && !cf.number_style) { cf.number_style = "currency"; cf.currency = cf.currency ?? "USD"; }
    const title = (c.verbose_name ?? "").trim();
    if (title) cf.column_title = title;
    if (Object.keys(cf).length) m[c.column_name] = cf;
  }
  for (const mt of metrics) {
    const cf: ColumnFormat = { ...parseFormat(mt.format) };
    const title = (mt.verbose_name ?? "").trim();
    if (title) cf.column_title = title;
    if (Object.keys(cf).length) m[mt.metric_name] = cf;
  }
  return m;
}

/**
 * The chart's own column settings over the dataset defaults. A column the chart configured
 * replaces the dataset's format for it wholesale, as it always has; only the display name carries
 * through unless the chart set a title of its own, because a chart that changed a column's
 * decimals did not mean to rename it back to its raw name.
 */
export function withDatasetDefaults(
  defaults: Record<string, ColumnFormat>,
  chart: Record<string, ColumnFormat>,
): Record<string, ColumnFormat> {
  const out: Record<string, ColumnFormat> = { ...defaults, ...chart };
  for (const [name, own] of Object.entries(chart)) {
    const title = defaults[name]?.column_title;
    if (title && !own?.column_title) out[name] = { ...own, column_title: title };
  }
  return out;
}
