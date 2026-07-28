import { isCartesianViz, isParamsV2 } from "@/api/charts";
import type { AnyChartParams, ChartParams, VizType } from "@/api/charts";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import { cartesianFromSettings } from "./cartesian-codec";
import { pivotFromParams } from "./pivot";
import { mapDescriptor } from "./map";

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
  if (viz === "pivot_table") {
    // v1 kept the pivot layout loose at the top level; PivotTable still reads it there.
    const p = pivotFromParams(params);
    out.rows = p.rows; out.cols = p.cols; out.metrics = p.metrics;
    out.showRowTotals = p.showRowTotals; out.showColTotals = p.showColTotals; out.condense = p.condense;
    out.collapsedRows = p.collapsedRows; out.pivotSort = p.sort;
  }
  if (viz === "map") out.map = mapDescriptor().fromParams(params);
  // `table` and `object` need nothing here: v1 carried no config for them, and ChartTable
  // and ObjectDetail take their columns as props from the result, not from params.
  return out;
}
