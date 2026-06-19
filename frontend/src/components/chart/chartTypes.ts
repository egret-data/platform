import { Table2, LayoutGrid, type LucideIcon } from "lucide-react";
import type { VizType } from "@/api/charts";

export interface ChartTypeMeta {
  key: VizType;
  name: string;
  description: string;
  icon: LucideIcon;
}

export const CHART_TYPES: ChartTypeMeta[] = [
  {
    key: "table",
    name: "Table",
    description: "Raw rows or an aggregated table with sorting and formatting.",
    icon: Table2,
  },
  {
    key: "pivot_table",
    name: "Pivot Table",
    description: "Cross-tab rows × columns × measures with subtotals and grand totals.",
    icon: LayoutGrid,
  },
];
