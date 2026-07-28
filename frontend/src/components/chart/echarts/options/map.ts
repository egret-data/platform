import type { EChartsCoreOption } from "echarts/core";
import type { ResultColumn } from "@/components/chart/ChartTable";
import { ariaDescription, baseOption, itemTooltip } from "../base";

export interface MapBuildConfig {
  echartsName: string;
  region?: string;
  metric?: string;
  normalize: (v: unknown) => string;
  /** Formats the metric for the tooltip; defaults to a plain locale number. */
  format?: (v: unknown) => string;
  /** Fixed text labels at geo coordinates [lon, lat] (e.g. archipelagos). */
  annotations?: { name: string; coord: [number, number] }[];
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
  const annotations = cfg.annotations ?? [];
  // Annotation labels are drawn by a scatter series on a hidden `geo` (which provides the lon/lat→
  // pixel projection). The choropleth `map` series and the geo share an identical layout so the
  // label points land in the right place.
  const layout = { layoutCenter: ["50%", "50%"], layoutSize: "92%" };
  return {
    ...baseOption(),
    aria: ariaDescription({
      kind: "Map", metric: cfg.metric, dimension: cfg.region,
      format: cfg.format ?? ((v) => Number(v).toLocaleString()),
      points: data.map((d) => ({ name: d.name, value: d.value })),
    }),
    // The map used to print raw `toLocaleString()` numbers, ignoring the chart's column
    // formats entirely — it is the one builder that never formatted its metric.
    tooltip: itemTooltip(cfg.format ?? ((v) => Number(v).toLocaleString())),
    visualMap: {
      min,
      max,
      left: 8,
      bottom: 8,
      calculable: true,
      inRange: { color: ["#e6f4f1", "#0f9b8e"] },
      text: ["High", "Low"],
      seriesIndex: 0, // colour the choropleth (map) series only, not the annotation scatter
    },
    ...(annotations.length > 0
      ? { geo: { map: cfg.echartsName, roam: false, show: false, ...layout } }
      : {}),
    series: [
      {
        type: "map",
        map: cfg.echartsName,
        nameProperty: "key",
        roam: false,
        emphasis: { label: { show: false } },
        ...(annotations.length > 0 ? layout : {}),
        data,
      },
      ...(annotations.length > 0
        ? [
            {
              type: "scatter",
              coordinateSystem: "geo",
              symbol: "circle",
              symbolSize: 5,
              itemStyle: { color: "#0f766e" },
              silent: true,
              label: { show: true, position: "right", formatter: "{b}", fontSize: 11, fontWeight: "bold", color: "#0f766e" },
              data: annotations.map((a) => ({ name: a.name, value: a.coord })),
            },
          ]
        : []),
    ],
  };
}
