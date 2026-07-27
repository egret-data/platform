import { useEffect, useRef } from "react";
import { init, type EChartsCoreOption, type ECharts } from "echarts/core";
import { registerEChartsModules } from "./register";

registerEChartsModules();

export interface EChartsPointClick { name?: string; seriesName?: string; value?: unknown; x: number; y: number }

const prefersReducedMotion = () =>
  typeof window !== "undefined" && !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

/**
 * Apply cross-cutting accessibility to every ECharts chart in one place:
 *  - `aria.enabled` makes ECharts emit a screen-reader description of the chart
 *    (series, values) into the SVG — canvas/SVG charts are otherwise opaque to AT.
 *  - honour prefers-reduced-motion by disabling entrance/transition animation.
 * A caller's own `aria`/`animation` still take precedence when set.
 */
function withA11y(option: EChartsCoreOption): EChartsCoreOption {
  const o = option as Record<string, unknown>;
  const reduce = prefersReducedMotion();
  return {
    ...o,
    animation: reduce ? false : ((o.animation as boolean | undefined) ?? true),
    aria: { enabled: true, ...((o.aria as object | undefined) ?? {}) },
  } as unknown as EChartsCoreOption;
}

/**
 * Thin SVG ECharts wrapper, mirroring Metabase's EChartsRenderer:
 * init on mount, setOption on change, resize via ResizeObserver, dispose on unmount.
 * `ariaLabel` gives assistive tech a one-line summary of what the chart shows.
 */
export function EChartsRenderer({ option, className, ariaLabel, onPointClick }: {
  option: EChartsCoreOption;
  className?: string;
  ariaLabel?: string;
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
    chart.setOption(withA11y(option), true);
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
    chartRef.current?.setOption(withA11y(option), true);
  }, [option]);

  return <div ref={elRef} role="img" aria-label={ariaLabel ?? "Chart"} className={className ?? "h-full w-full"} />;
}
