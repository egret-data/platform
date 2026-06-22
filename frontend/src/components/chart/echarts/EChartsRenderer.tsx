import { useEffect, useRef } from "react";
import { init, type EChartsCoreOption, type ECharts } from "echarts/core";
import { registerEChartsModules } from "./register";

registerEChartsModules();

export interface EChartsPointClick { name?: string; seriesName?: string; value?: unknown; x: number; y: number }

/**
 * Thin SVG ECharts wrapper, mirroring Metabase's EChartsRenderer:
 * init on mount, setOption on change, resize via ResizeObserver, dispose on unmount.
 */
export function EChartsRenderer({ option, className, onPointClick }: {
  option: EChartsCoreOption;
  className?: string;
  onPointClick?: (p: EChartsPointClick) => void;
}) {
  const elRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<ECharts | null>(null);
  const clickRef = useRef(onPointClick);
  clickRef.current = onPointClick;

  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    const chart = init(el, null, { renderer: "svg" });
    chartRef.current = chart;
    chart.setOption(option, true);
    chart.on("click", (params: unknown) => {
      const p = params as { name?: string; seriesName?: string; value?: unknown; event?: { event?: { clientX?: number; clientY?: number } } };
      clickRef.current?.({ name: p.name, seriesName: p.seriesName, value: p.value, x: p.event?.event?.clientX ?? 0, y: p.event?.event?.clientY ?? 0 });
    });
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
