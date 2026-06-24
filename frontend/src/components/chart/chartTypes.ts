import {
  Table2, LayoutGrid, BarChart3, BarChartHorizontal, LineChart, AreaChart, ScatterChart, Spline, PieChart, Hash, BarChartHorizontalBig, Gauge, Filter, BarChart4, TrendingUp, Box, Workflow, FileText, Map,
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
  { key: "progress", name: "Progress", description: "A single value against a goal, as a progress bar.", icon: BarChartHorizontalBig },
  { key: "gauge", name: "Gauge", description: "A single value on an arc between a min and max.", icon: Gauge },
  { key: "funnel", name: "Funnel", description: "Stages of a process, each narrower than the last.", icon: Filter },
  { key: "waterfall", name: "Waterfall", description: "Sequential increases and decreases building to a total.", icon: BarChart4 },
  { key: "smartscalar", name: "Trend", description: "The latest value and its change versus the previous period.", icon: TrendingUp },
  { key: "boxplot", name: "Box plot", description: "The distribution of a value (min/quartiles/max) per category.", icon: Box },
  { key: "sankey", name: "Sankey", description: "Flows between source and target categories, weighted by a metric.", icon: Workflow },
  { key: "object", name: "Object detail", description: "Inspect one record as field/value pairs, paging through results.", icon: FileText },
  { key: "map", name: "Map", description: "Shade regions of a map by a metric (US states, world, Vietnam).", icon: Map },
];
