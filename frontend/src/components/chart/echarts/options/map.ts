import type { EChartsCoreOption } from "echarts/core";
import type { ResultColumn } from "@/components/chart/ChartTable";

export interface MapBuildConfig {
  echartsName: string;
  region?: string;
  metric?: string;
  normalize: (v: unknown) => string;
}

/** Build a choropleth `map` option. The series data names are normalized region keys that match
 *  the registered geojson's `properties.key`; unmatched/NaN rows are dropped. */
export function buildMapOption(columns: ResultColumn[], rows: unknown[][], cfg: MapBuildConfig): EChartsCoreOption {
  const idx = (name?: string) => columns.findIndex((c) => c.name === name);
  const ri = idx(cfg.region);
  const mi = idx(cfg.metric);
  const data =
    ri < 0 || mi < 0
      ? []
      : rows
          .map((r) => ({ name: cfg.normalize(r[ri]), value: Number(r[mi]) }))
          .filter((d) => d.name !== "" && Number.isFinite(d.value));
  const values = data.map((d) => d.value);
  const min = values.length ? Math.min(...values) : 0;
  const max = values.length ? Math.max(...values) : 1;
  return {
    tooltip: {
      trigger: "item",
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      formatter: (p: any) =>
        p.value == null || Number.isNaN(p.value) ? String(p.name ?? "") : `${p.name}: ${Number(p.value).toLocaleString()}`,
    },
    visualMap: {
      min,
      max,
      left: 8,
      bottom: 8,
      calculable: true,
      inRange: { color: ["#e6f4f1", "#0f9b8e"] },
      text: ["High", "Low"],
    },
    series: [
      {
        type: "map",
        map: cfg.echartsName,
        nameProperty: "key",
        roam: false,
        emphasis: { label: { show: false } },
        data,
      },
    ],
  };
}
