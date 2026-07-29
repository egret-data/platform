import { useEffect, useRef } from "react";
import { init, type EChartsCoreOption, type ECharts } from "echarts/core";
import { registerEChartsModules } from "./register";

registerEChartsModules();

export interface EChartsPointClick { name?: string; seriesName?: string; value?: unknown; x: number; y: number }

export interface EChartsSize { width: number; height: number }

/** `option` is normally a plain, already-built ECharts option object — every chart type except
 * pie. Pie is the one builder (see options/pie.ts's `containerSize`) that needs the chart's REAL
 * rendered pixel size to fit its slice labels correctly, and this component is the only place
 * with actual DOM access to measure that — so it ALSO accepts a function, called with the
 * measured size on init and again whenever the observed size changes, and expected to return the
 * option built against that size. */
export type EChartsOptionInput = EChartsCoreOption | ((size: EChartsSize) => EChartsCoreOption);

/**
 * Thin SVG ECharts wrapper, mirroring Metabase's EChartsRenderer:
 * init on mount, setOption on change, resize via ResizeObserver, dispose on unmount.
 */
export function EChartsRenderer({ option, className, onPointClick }: {
  option: EChartsOptionInput;
  className?: string;
  onPointClick?: (p: EChartsPointClick) => void;
}) {
  const elRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<ECharts | null>(null);
  const clickRef = useRef(onPointClick);
  clickRef.current = onPointClick;

  // The chart's last-measured real size — read on init (before ECharts even exists yet) and kept
  // current by the ResizeObserver below, so a function-valued `option` always gets rebuilt
  // against whatever the container's real size is RIGHT NOW rather than whatever it was when
  // this component first mounted. Irrelevant to a plain-object `option` (every other chart
  // type): `resolveOption` below only ever reads it for the function branch.
  const sizeRef = useRef<EChartsSize>({ width: 0, height: 0 });
  const resolveOption = (opt: EChartsOptionInput): EChartsCoreOption =>
    typeof opt === "function" ? opt(sizeRef.current) : opt;

  // The LATEST `option` prop, read from effects that shouldn't themselves re-run on every render
  // just because a resize tick needs "whatever option is current right now".
  const optionRef = useRef(option);
  optionRef.current = option;

  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    sizeRef.current = { width: Math.round(rect.width), height: Math.round(rect.height) };
    const chart = init(el, null, { renderer: "svg" });
    chartRef.current = chart;
    chart.setOption(resolveOption(optionRef.current), true);
    chart.on("click", (params: unknown) => {
      const p = params as { name?: string; seriesName?: string; value?: unknown; event?: { event?: { clientX?: number; clientY?: number } } };
      clickRef.current?.({ name: p.name, seriesName: p.seriesName, value: p.value, x: p.event?.event?.clientX ?? 0, y: p.event?.event?.clientY ?? 0 });
    });

    // A ResizeObserver tick fires on every layout change — including, during an active
    // drag-resize, many times a second. Rebuilding a function-valued option re-runs pie's whole
    // per-slice label-fit pass (real text measurement + geometry for every slice), so that work
    // is guarded on two fronts:
    //  - skipped outright when the measured size hasn't actually changed (rounded to whole
    //    pixels — sub-pixel layout jitter isn't a real resize and shouldn't count as one);
    //  - otherwise coalesced to at most once per animation frame via requestAnimationFrame, so a
    //    drag that fires the observer 60 times/sec still only rebuilds the option once per
    //    frame, always against the LATEST size seen (not every intermediate one).
    // `chart.resize()` itself stays unconditional and un-coalesced on every tick — it's just a
    // DOM-size read + SVG viewport repaint, the same cost this already had before function-valued
    // options existed, and every chart type (not just pie) still needs it every tick.
    let rafId: number | null = null;
    const ro = new ResizeObserver((entries) => {
      chart.resize();
      if (typeof optionRef.current !== "function") return;
      const rect = entries[0]?.contentRect ?? el.getBoundingClientRect();
      const next = { width: Math.round(rect.width), height: Math.round(rect.height) };
      if (next.width === sizeRef.current.width && next.height === sizeRef.current.height) return;
      sizeRef.current = next;
      if (rafId != null) return; // a rebuild against the latest size is already scheduled
      rafId = requestAnimationFrame(() => {
        rafId = null;
        chart.setOption(resolveOption(optionRef.current), true);
      });
    });
    ro.observe(el);
    return () => {
      if (rafId != null) cancelAnimationFrame(rafId);
      ro.disconnect();
      if (!chart.isDisposed()) chart.dispose();
      chartRef.current = null;
    };
  }, []);

  useEffect(() => {
    chartRef.current?.setOption(resolveOption(option), true);
  }, [option]);

  return <div ref={elRef} className={className ?? "h-full w-full"} />;
}
