import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, CartesianConfig, VizSettings } from "@/api/charts";

/** Settings namespace shared by every cartesian kind, so bar→line keeps its config. */
export const CARTESIAN_PREFIX = "cartesian.";

/** Config keys persisted for the cartesian family. Order is irrelevant; completeness is not. */
const KEYS = [
  "xAxis", "xMetric", "yMetrics", "series", "stacked", "horizontal",
  "showLabels", "showLegend", "xLabel", "yLabel", "seriesTypes", "rightAxisMetrics",
  "logScale", "normalized", "goalValue", "goalLabel", "trendLine",
  "showStackTotal", "seriesColors", "seriesLabels", "yMin", "yMax",
  "xScale", "compactValues", "maxCategories",
  "seriesOrder", "hiddenSeries", "labelDensity",
  "xAxisTicks", "yAxisTicks", "yTickCount",
  "lineShape", "missing", "markers", "seriesLine",
] as const satisfies readonly (keyof CartesianConfig)[];

/** The `cartesian.*` slice of a v2 settings map. Undefined values are dropped, not stored as null. */
export function cartesianToSettings(cfg: CartesianConfig): VizSettings {
  const out: VizSettings = {};
  for (const k of KEYS) {
    const v = cfg[k];
    if (v !== undefined) out[CARTESIAN_PREFIX + k] = v;
  }
  return out;
}

export function cartesianFromSettings(settings: VizSettings): CartesianConfig {
  const out = { yMetrics: [] } as CartesianConfig;
  for (const k of KEYS) {
    const v = settings[CARTESIAN_PREFIX + k];
    if (v !== undefined) (out as unknown as Record<string, unknown>)[k] = v;
  }
  return out;
}

/**
 * Config from either stored shape. The v1 branch is the fallback chain that used to live
 * in ChartExplore's hydration effect: older charts predate the `cartesian` key and only
 * carry their metrics in the spec.
 */
export function cartesianFromParams(params: AnyChartParams): CartesianConfig {
  if (isParamsV2(params)) return cartesianFromSettings(params.settings);
  return params.cartesian ?? { yMetrics: params.spec?.metrics ?? [] };
}
