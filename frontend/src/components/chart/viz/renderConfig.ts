import { isParamsV2 } from "@/api/charts";
import type { AnyChartParams, TableState, VizType } from "@/api/charts";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import { getDescriptor } from "./registry";

/**
 * A viz's own configuration, taken from its descriptor. Descriptors read both stored
 * shapes, so this works for a v2 chart and for one that predates the backfill — which is
 * why retiring `toRenderParams` is not a hard cut-over.
 */
export function vizConfig<Cfg = Record<string, unknown>>(viz: VizType, params: AnyChartParams): Cfg {
  const desc = getDescriptor(viz);
  return (desc ? desc.fromParams(params) : {}) as Cfg;
}

/**
 * The settings the shell owns rather than any one viz: number formats, conditional
 * formatting rules and table state. Reads v2's flat keys, falling back to v1's.
 */
export function shellSettings(params: AnyChartParams): {
  columnSettings: Record<string, ColumnFormat>;
  conditionalFormatting: Rule[];
  tableState: TableState;
} {
  if (isParamsV2(params)) {
    const s = params.settings;
    return {
      columnSettings: (s.column_settings as Record<string, ColumnFormat>) ?? {},
      conditionalFormatting: (s.conditional_formatting as Rule[]) ?? [],
      tableState: (s["table.state"] as TableState) ?? {},
    };
  }
  return {
    columnSettings: params.columnSettings ?? {},
    conditionalFormatting: params.conditionalFormatting ?? [],
    tableState: params.tableState ?? {},
  };
}
