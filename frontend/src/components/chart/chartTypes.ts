import {
  Table2, LayoutGrid, BarChart3, BarChartHorizontal, LineChart, AreaChart, ScatterChart, Spline, PieChart, Hash,
  type LucideIcon,
} from "lucide-react";
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
  { key: "bar", name: "Bar", description: "Compare a metric across a category, optionally broken out by a series.", icon: BarChart3 },
  { key: "row", name: "Row", description: "Horizontal bars, sorted by value — good for ranking many categories.", icon: BarChartHorizontal },
  { key: "line", name: "Line", description: "Trend a metric over an ordered category such as time.", icon: LineChart },
  { key: "area", name: "Area", description: "Line chart with the area below filled; good for stacked totals.", icon: AreaChart },
  { key: "scatter", name: "Scatter", description: "Plot points for a metric against a category.", icon: ScatterChart },
  { key: "combo", name: "Combo", description: "Mix bar and line series on shared axes.", icon: Spline },
  { key: "pie", name: "Pie", description: "Show each category's share of a single metric.", icon: PieChart },
  { key: "scalar", name: "Big number", description: "A single aggregated value, large and formatted.", icon: Hash },
];
