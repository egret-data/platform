import { useEffect, useRef } from "react";
import { init, type EChartsCoreOption, type ECharts } from "echarts/core";
import { registerEChartsModules } from "./register";
import { onThemeChange } from "./palette";

registerEChartsModules();

export interface EChartsPointClick {
  name?: string;
  seriesName?: string;
  value?: unknown;
  /** Root-to-node path, tree series only (`SunburstSeriesModel.getDataParams` adds it via
   * `wrapTreePathInfo`). Its first entry is the sunburst's own VIRTUAL root — the node ECharts
   * synthesises from `series.data` and never draws — so a clicked slice's ring index is
   * `treePathInfo.length - 2`. The pie is the one chart that reads it (see ChartRenderer): with a
   * single series there is no per-ring `seriesName` left to attribute a click by. */
  treePathInfo?: { name?: string }[];
  /** Index of the series the clicked element belongs to. Lets a consumer recover fields the option
   * put ON THE SERIES rather than on the datum — the cartesian drill reads `__raw` this way,
   * because `seriesName` is the DISPLAY label and a renamed series must still filter by its
   * column's real value. */
  seriesIndex?: number;
  /** The datum OBJECT this element was built from, untouched — the same field the hover path has
   * always forwarded. The pie's drill needs it: a click reports the slice's display NAME, and the
   * raw dimension path it stands for lives on the datum (see options/pie.ts' `pieSliceDrillPath`). */
  data?: unknown;
  /** The datum's index within its series. The box plot's drill needs it: a `boxplot` element
   * reports no usable `name`, so the clicked category is `xAxis.data[dataIndex]`. */
  dataIndex?: number;
  /** Which KIND of element was clicked, for series that draw more than one. The sankey is the only
   * chart with two: `"node"` and `"edge"`, which drill to different things — a node to itself, an
   * edge to the pair of ends it joins. */
  dataType?: string;
  x: number;
  y: number;
}

/** The slice (or point) the pointer is currently on. Same identity fields as a click, minus the
 * cursor position — nothing that reacts to a hover needs it, which is also why these come from
 * `mouseover`/`mouseout` rather than `mousemove`: one event per element ENTERED, not one per
 * pixel travelled, so no consumer has to defend against pointer-rate updates. */
export interface EChartsPointHover {
  name?: string;
  seriesName?: string;
  value?: unknown;
  /** The datum OBJECT the option handed ECharts for this element (`getDataParams`' own `data`),
   * untouched. `value` above is what ECharts LAID THE ELEMENT OUT WITH, which for the pie is not
   * always what the slice is worth — a sub-0.5% slice is deliberately drawn bigger so it can be
   * pointed at, and carries its true figure as `rawValue` on this object. See options/pie.ts's
   * MIN_DRAWN_SLICE_FRACTION. */
  data?: unknown;
  /** Root-to-node path, tree series only — see `EChartsPointClick.treePathInfo`. */
  treePathInfo?: { name?: string }[];
}

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
export function EChartsRenderer({ option, className, onPointClick, onPointHover, overlay, highlightDataIndex }: {
  option: EChartsOptionInput;
  className?: string;
  onPointClick?: (p: EChartsPointClick) => void;
  /** Called with the series element under the pointer, and with `null` when it leaves. */
  onPointHover?: (p: EChartsPointHover | null) => void;
  /**
   * A small partial option applied in MERGE mode on top of `option`, and re-stated after every
   * full (notMerge) apply — so it survives a resize, a theme flip and an option change.
   *
   * This exists for one reason: the pie's centre readout has to change on every hover, and its
   * option is the expensive one to rebuild (a per-slice `measureText` fit pass over every ring).
   * Re-applying that whole option per hover would also tear the series down and re-init it, which
   * would drop ECharts' own emphasis/blur state mid-hover. An overlay touches only the keys it
   * names — for the pie, `graphic`, whose elements carry stable ids so they merge in place.
   *
   * It cannot UN-say anything (there is no "unmerge"), so a caller that ever supplies one should
   * keep supplying one, stating the resting value rather than dropping to `undefined`.
   */
  overlay?: EChartsOptionInput;
  /** Datum to hold in ECharts' emphasis state — the legend's half of a two-way hover. Downplayed
   * again when it changes or the chart unmounts. */
  highlightDataIndex?: number;
}) {
  const elRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<ECharts | null>(null);
  const clickRef = useRef(onPointClick);
  clickRef.current = onPointClick;
  const hoverRef = useRef(onPointHover);
  hoverRef.current = onPointHover;

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
  const overlayRef = useRef(overlay);
  overlayRef.current = overlay;

  /** A full (notMerge) apply of the current option, with the overlay re-stated on top of it.
   * Every path that re-applies the whole option goes through here — a notMerge apply resets the
   * keys the overlay owns back to whatever the option says, so anything that skipped this would
   * snap the pie's hole back to its resting text in the middle of a hover. */
  const applyOption = (chart: ECharts) => {
    chart.setOption(resolveOption(optionRef.current), true);
    if (overlayRef.current) chart.setOption(resolveOption(overlayRef.current));
  };

  useEffect(() => {
    const el = elRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    sizeRef.current = { width: Math.round(rect.width), height: Math.round(rect.height) };
    const chart = init(el, null, { renderer: "svg" });
    chartRef.current = chart;
    applyOption(chart);
    chart.on("click", (params: unknown) => {
      const p = params as {
        name?: string; seriesName?: string; value?: unknown; data?: unknown; seriesIndex?: number;
        dataIndex?: number; dataType?: string;
        treePathInfo?: { name?: string }[];
        event?: { event?: { clientX?: number; clientY?: number } };
      };
      clickRef.current?.({
        name: p.name, seriesName: p.seriesName, value: p.value, data: p.data,
        seriesIndex: p.seriesIndex, dataIndex: p.dataIndex, dataType: p.dataType,
        treePathInfo: p.treePathInfo,
        x: p.event?.event?.clientX ?? 0, y: p.event?.event?.clientY ?? 0,
      });
    });

    // Scoped to ECharts' `series` query so nothing else in the canvas can pass for a data point —
    // in particular the pie's own centre `graphic`, which sits exactly where a pointer heading
    // for the middle of the chart crosses.
    chart.on("mouseover", "series", (params: unknown) => {
      const p = params as EChartsPointHover;
      hoverRef.current?.({
        name: p.name, seriesName: p.seriesName, value: p.value, data: p.data, treePathInfo: p.treePathInfo,
      });
    });
    chart.on("mouseout", "series", () => hoverRef.current?.(null));

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
        applyOption(chart);
      });
    });
    ro.observe(el);

    // ECharts bakes concrete colours into the SVG it draws, so a theme switch does nothing to an
    // already-drawn chart. React won't help here either: ThemeProvider's `children` element is
    // referentially stable, so toggling the theme never re-renders the chart tree. Re-applying
    // the option is what makes the switch land — by this point palette.ts has already dropped its
    // resolved-colour cache, so the rebuild picks up the new theme's values.
    const offTheme = onThemeChange(() => {
      applyOption(chart);
    });

    return () => {
      offTheme();
      if (rafId != null) cancelAnimationFrame(rafId);
      ro.disconnect();
      if (!chart.isDisposed()) chart.dispose();
      chartRef.current = null;
    };
  }, []);

  useEffect(() => {
    const chart = chartRef.current;
    if (chart) applyOption(chart);
    // `option` alone: the overlay has its own effect below, and applyOption reads the current one
    // off a ref, so a new overlay never needs to drag the expensive option apply along with it.
  }, [option]);

  // The overlay on its own — the hover path. This is the ONE apply that runs at hover frequency,
  // and it is a merge of a couple of `graphic` elements: no series diff, no re-init, and nothing
  // that disturbs the emphasis/blur state ECharts is holding for the slice under the pointer.
  useEffect(() => {
    if (overlay) chartRef.current?.setOption(resolveOption(overlay));
  }, [overlay]);

  // Metabase's own `higlightChartOnLegendHover`, prop-shaped: highlight on the way in, downplay
  // on the way out (including on unmount), addressing the datum by INDEX rather than by name —
  // see ChartRenderer for why a sunburst cannot safely be addressed by name.
  useEffect(() => {
    const chart = chartRef.current;
    if (!chart || highlightDataIndex == null) return;
    chart.dispatchAction({ type: "highlight", seriesIndex: 0, dataIndex: highlightDataIndex });
    return () => {
      if (!chart.isDisposed()) chart.dispatchAction({ type: "downplay", seriesIndex: 0, dataIndex: highlightDataIndex });
    };
  }, [highlightDataIndex]);

  return <div ref={elRef} className={className ?? "h-full w-full"} />;
}
