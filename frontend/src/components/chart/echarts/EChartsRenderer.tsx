import { useEffect, useRef } from "react";
import { init, type EChartsCoreOption, type ECharts } from "echarts/core";
import { registerEChartsModules } from "./register";

registerEChartsModules();

/**
 * Thin SVG ECharts wrapper, mirroring Metabase's EChartsRenderer:
 * init on mount, setOption on change, resize via ResizeObserver, dispose on unmount.
 */
export function EChartsRenderer({ option, className }: { option: EChartsCoreOption; className?: string }) {
  const elRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<ECharts | null>(null);

  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    const chart = init(el, null, { renderer: "svg" });
    chartRef.current = chart;
    chart.setOption(option, true);

    const ro = new ResizeObserver(() => chart.resize());
    ro.observe(el);
    return () => {
      ro.disconnect();
      if (!chart.isDisposed()) chart.dispose();
      chartRef.current = null;
    };
  }, []);

  useEffect(() => {
    chartRef.current?.setOption(option, true);
  }, [option]);

  return <div ref={elRef} className={className ?? "h-full w-full"} />;
}
