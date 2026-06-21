import { use } from "echarts/core";
import { SVGRenderer } from "echarts/renderers";
import { BarChart, LineChart, ScatterChart, PieChart, GaugeChart } from "echarts/charts";
import {
  GridComponent, TooltipComponent, LegendComponent, DatasetComponent, GraphicComponent, MarkLineComponent,
} from "echarts/components";

let registered = false;

/** Register only the ECharts modules this app uses (tree-shaken). Idempotent. */
export function registerEChartsModules() {
  if (registered) return;
  use([
    SVGRenderer,
    BarChart, LineChart, ScatterChart, PieChart, GaugeChart,
    GridComponent, TooltipComponent, LegendComponent, DatasetComponent, GraphicComponent, MarkLineComponent,
  ]);
  registered = true;
}
