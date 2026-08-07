import { use } from "echarts/core";
import { SVGRenderer } from "echarts/renderers";
// SunburstChart, not PieChart: egret's pie IS a sunburst now (one nested series, like Metabase's
// — see options/pie.ts). PieChart is gone because nothing builds a `pie` series any more, and a
// registered-but-unused chart type is dead bundle weight that also hides a regression: a stray
// `type: "pie"` would silently keep working instead of failing loudly.
import { BarChart, LineChart, ScatterChart, SunburstChart, GaugeChart, FunnelChart, BoxplotChart, SankeyChart, MapChart } from "echarts/charts";
// TitleComponent is here for the CARTESIAN SPLIT PANELS and nothing else (`options/cartesian.ts`,
// `panelTitles`): each panel is named above itself, which is what egret has where Metabase renders a
// React legend over the canvas. No chart writes a chart-level `title` — the card's own heading does
// that job — so a `title` key in an option is always a panel's.
import {
  GridComponent, TooltipComponent, LegendComponent, DatasetComponent, GraphicComponent, MarkLineComponent, VisualMapComponent,
  TitleComponent,
} from "echarts/components";
// The aria component is not re-exported from "echarts/components", and this deep entry
// self-registers on import. Without it every `aria` option — the screen-reader description
// AND the decal patterns — is silently ignored, which is exactly what happened here until
// the accessibility audit went looking for the label and found none.
import "echarts/lib/component/aria";
// LabelLayout is listed here for the same reason `aria` is imported above, one rung further down:
// echarts DOES install it itself, as a top-level `use(installLabelLayout)` in `lib/export/core.js`
// — but that file is not in the package's `sideEffects` allowlist (`index*.js`, `lib/echarts.js`,
// `lib/chart/*`, `lib/component/*`, `extension/**`, `theme/*`, `i18n/*`), so Rollup deletes the
// call from a production build and LabelManager never reaches the bundle. Vitest and `vite dev`
// do not tree-shake, so both run it happily and the loss is invisible until you grep `dist`.
//
// DO NOT REMOVE THIS AS REDUNDANT. Without it every `labelLayout` option in `options/` — the
// `fit` density the cartesian, waterfall and box-plot panels offer by name, and the pie's and
// sankey's overlap backstops — is silently ignored in the shipped app only.
import { LabelLayout } from "echarts/features";

let registered = false;

/** Register only the ECharts modules this app uses (tree-shaken). Idempotent. */
export function registerEChartsModules() {
  if (registered) return;
  use([
    SVGRenderer,
    BarChart, LineChart, ScatterChart, SunburstChart, GaugeChart, FunnelChart, BoxplotChart, SankeyChart, MapChart,
    GridComponent, TooltipComponent, LegendComponent, DatasetComponent, GraphicComponent, MarkLineComponent, VisualMapComponent,
    TitleComponent,
    LabelLayout,
  ]);
  registered = true;
}
