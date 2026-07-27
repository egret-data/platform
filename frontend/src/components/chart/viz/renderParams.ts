import { isCartesianViz, isParamsV2 } from "@/api/charts";
import type { AnyChartParams, ChartParams, VizType } from "@/api/charts";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import { cartesianFromSettings } from "./cartesian-codec";

/**
 * The shape every renderer still speaks. v1 params pass through; v2 params are
 * un-flattened here and nowhere else, so ChartRenderer, PivotTable, ChartTable and the
 * dashboard's `vizOverride` merge all keep working on the pre-A2 keys.
 *
 * Only the namespaces migrated so far are un-flattened — PR2 adds pivot/table/object/map
 * and PR3 the remaining nine. This function and its callers disappear in sub-project B.
 */
export function toRenderParams(viz: VizType, params: AnyChartParams): ChartParams {
  if (!isParamsV2(params)) return params;
  const s = params.settings;
  const out: ChartParams = {
    spec: params.spec,
    columnSettings: s.column_settings as Record<string, ColumnFormat> | undefined,
    conditionalFormatting: s.conditional_formatting as Rule[] | undefined,
  };
  if (isCartesianViz(viz)) out.cartesian = cartesianFromSettings(s);
  return out;
}
