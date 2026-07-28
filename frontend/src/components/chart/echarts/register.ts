import { use } from "echarts/core";
import { SVGRenderer } from "echarts/renderers";
import { BarChart, LineChart, ScatterChart, PieChart, GaugeChart, FunnelChart, BoxplotChart, SankeyChart, MapChart } from "echarts/charts";
import {
  GridComponent, TooltipComponent, LegendComponent, DatasetComponent, GraphicComponent, MarkLineComponent, VisualMapComponent,
} from "echarts/components";
// The aria component is not re-exported from "echarts/components", and this deep entry
// self-registers on import. Without it every `aria` option — the screen-reader description
// AND the decal patterns — is silently ignored, which is exactly what happened here until
// the accessibility audit went looking for the label and found none.
import "echarts/lib/component/aria";

let registered = false;

/** Register only the ECharts modules this app uses (tree-shaken). Idempotent. */
export function registerEChartsModules() {
  if (registered) return;
  use([
    SVGRenderer,
    BarChart, LineChart, ScatterChart, PieChart, GaugeChart, FunnelChart, BoxplotChart, SankeyChart, MapChart,
    GridComponent, TooltipComponent, LegendComponent, DatasetComponent, GraphicComponent, MarkLineComponent, VisualMapComponent,
  ]);
  registered = true;
}
