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

/** `option` is either a plain, already-built ECharts option object or a FUNCTION of the chart's
 * measured size. The function form exists for the builders that need the REAL rendered pixel size
 * to lay themselves out — pie, to fit its slice labels (see options/pie.ts's `containerSize`), and
 * the three that draw a bottom category axis (the cartesian family, the box plot and the
 * waterfall), each of which decides its x-axis label treatment from it — draw every category name
 * upright, rotate every one, or hide the axis, never a subset (see `echarts/axisLabelFit.ts`; the
 * old count-and-length guess is gone from all three, and `__tests__/sizeAware.test.ts` now pins
 * that the size changes the label decision and NOTHING else). Gauge and sankey still take the size
 * unread. This component is the only place with actual DOM access to measure
 * that, so it accepts the function and calls it with the measured size on init and again whenever
 * the observed size changes, expecting the option built against that size back.
 *
 * The list of function-valued builders grows; nothing here needs to know which they are, since
 * `resolveOption` branches on `typeof` rather than on chart type. */
export type EChartsOptionInput = EChartsCoreOption | ((size: EChartsSize) => EChartsCoreOption);

/**
 * Thin SVG ECharts wrapper, mirroring Metabase's EChartsRenderer:
 * init on mount, setOption on change, resize via ResizeObserver, dispose on unmount.
 */
export function EChartsRenderer({ option, className, onPointClick, onPointHover, overlay, highlightDataIndex, onFinished }: {
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
  /** ECharts' own `finished` event — fires once the chart has actually painted, including its
   * mount animation, as opposed to `rendered` which fires on every frame of one. The print route
   * (`routes/print/PrintObject.tsx`) is the reason this exists: it screenshots a chrome-free page
   * and needs to know when every chart on it is done drawing rather than guessing with a fixed
   * wait, which captures a half-drawn chart on an unlucky run. Not deduped or unsubscribed after
   * the first call — ECharts fires it again after a resize or a theme change, and a caller that
   * only wants the first one (the print route does, via a ref-guarded flag) is free to ignore the
   * rest. */
  onFinished?: () => void;
}) {
  const elRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<ECharts | null>(null);
  const clickRef = useRef(onPointClick);
  clickRef.current = onPointClick;
  const hoverRef = useRef(onPointHover);
  hoverRef.current = onPointHover;
  const finishedRef = useRef(onFinished);
  finishedRef.current = onFinished;

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
  /**
   * The option object most recently handed to ECharts — the resolved one, so a function-valued
   * builder is not re-run just to read something off it. `legendselectchanged` is the only reader.
   */
  const resolvedRef = useRef<EChartsCoreOption | null>(null);
  const applyOption = (chart: ECharts) => {
    const resolved = resolveOption(optionRef.current);
    resolvedRef.current = resolved;
    chart.setOption(resolved, true);
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
    chart.on("finished", () => finishedRef.current?.());

    /**
     * WHAT THE BUILDER DERIVED FROM THE SERIES LIST HAS TO FOLLOW THE LEGEND. ECharts' legend hides
     * a series without the option being rebuilt, so anything baked at build time from that list
     * goes stale the moment anyone clicks one.
     *
     * TWO THINGS ARE NOW IN THAT CLASS, and they share this one subscription deliberately: the
     * description (`__ariaFor`, #412) and the value axis's title (`__axisNamesFor`, measured on the
     * same deployed "MB · Combo" — with `orders` clicked off, the left axis lost its numbers and
     * kept its "orders" title). They are the same bug seen from two components, so a second event
     * handler for the second one would be two half-answers to one question; the patch below is
     * assembled from whichever the builder offers and applied in a single merge. Both were
     * confirmed in a browser on the deployed chart before either was written — the description
     * still read "Combo chart of orders … 2 series; January 2026 205, …" with `orders` hidden.
     *
     * THE RE-DERIVATION BELONGS TO THE BUILDER, which is the only thing that knows what either was
     * made of, and both arrive as functions ON the option object — read HERE, off the object this
     * component holds, rather than out of ECharts' model: `setOption` deep-copies what it is given
     * and a function would not survive the trip.
     *
     * MERGE MODE, always. A notMerge apply would reset the legend selection that just changed,
     * which is the state both patches describe.
     */
    chart.on("legendselectchanged", (params: unknown) => {
      const selected = (params as { selected?: Record<string, boolean> }).selected;
      if (selected == null) return;
      type LegendAware = {
        __ariaFor?: (s?: Record<string, boolean>) => unknown;
        __axisNamesFor?: (s?: Record<string, boolean>) => Record<string, unknown>;
      };
      const built = resolvedRef.current as LegendAware | null;
      const patch: Record<string, unknown> = {};
      if (typeof built?.__ariaFor === "function") patch.aria = built.__ariaFor(selected);
      if (typeof built?.__axisNamesFor === "function") Object.assign(patch, built.__axisNamesFor(selected));
      // Nothing to say — every other builder. Applying an empty option is not free (ECharts still
      // walks its components) and would repaint on every legend click for no reason.
      if (Object.keys(patch).length === 0) return;
      chart.setOption(patch as EChartsCoreOption);
    });

    // A ResizeObserver tick fires on every layout change — including, during an active
    // drag-resize, many times a second. Rebuilding a function-valued option re-runs the whole
    // builder; pie is the motivating example and still the dearest (its per-slice label-fit pass
    // is real text measurement + geometry for every slice in every ring), but it is no longer the
    // only one taking this path — the cartesian family, the gauge and the sankey are
    // function-valued too. So that work is guarded on two fronts:
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

  // `data-chart-image` (`CHART_IMAGE_ATTR` in export-chart.ts): this <div> holds the chart's one
  // <svg>, which is the image `.png` exports.
  return <div ref={elRef} className={className ?? "h-full w-full"} data-chart-image="" />;
}
