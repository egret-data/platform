import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { isNumericType, isTemporalType } from "../../format-types";
import { formatValue } from "../../format";
import { ariaDescription, baseOption, tooltipSurface, TOOLTIP_WEIGHT } from "../base";
import { CHART_THEME, SERIES_PALETTE, COLOR_FALLBACK, resolveColor } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface WaterfallBuildConfig {
  xAxis?: string;
  metric?: string;
  /** Unset means SHOWN — Metabase's `waterfall.show_total` defaults to true. */
  showTotal?: boolean;
  increaseColor?: string;
  decreaseColor?: string;
  totalColor?: string;
  showLabels?: boolean;
  labelDensity?: "fit" | "all";
  compactValues?: boolean;
  goalValue?: number;
  goalLabel?: string;
  xLabel?: string;
  yLabel?: string;
  showXLabel?: boolean;
  showYLabel?: boolean;
  xAxisTicks?: boolean;
  yAxisTicks?: boolean;
  xLabelRotation?: 45 | 90;
  yTickCount?: number;
  yMin?: number;
  yMax?: number;
  logScale?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

/** Metabase's `waterfall.increase_color` default is `accent1`, `decrease_color` is `accent3`, and
 *  egret's palette carries the same eight accents in the same order — so these are the same hexes
 *  it draws, not lookalikes. */
export const INCREASE = SERIES_PALETTE[1]; // #88BF4D green
export const DECREASE = SERIES_PALETTE[3]; // #EF8C8C red
/** Metabase's total bar is `text-primary` — the page's own ink, deliberately not an accent, so the
 *  total reads as a summary of the coloured steps rather than as another step. egret's equivalent
 *  is its `--foreground` token, which is also what makes the bar follow the theme: the hard-coded
 *  navy this replaced stayed navy on a dark card, where Metabase's flips. */
const TOTAL_TOKEN = "var(--foreground)";

/**
 * Metabase-style Waterfall: sequential deltas floating on a running total, with an optional Total
 * bar rising from zero.
 *
 * Built from two stacked bar series — a transparent base that lifts the visible delta to where the
 * previous step left off. **`stackStrategy: "all"` is load-bearing.** ECharts' default is
 * `"samesign"`, under which a value stacks only on the running sum of values with the SAME sign;
 * the moment a running total goes below zero the base turns negative while the delta stays
 * positive, the two stop sharing a stack, and every later bar is drawn from the axis instead of
 * from the step it continues. Measured on `[+10, -30, +5]` at 400px: the −30 step was drawn as a
 * rise from 0 to +30 and the +5 step from 0 to +5, when they are a fall from +10 to −20 and a rise
 * from −20 to −15. Silently wrong, and no assertion about the option's shape can see it — the
 * numbers handed to ECharts were right both before and after.
 */
export function buildWaterfallOption(columns: ResultColumn[], rows: unknown[][], cfg: WaterfallBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const xi = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const increase = cfg.increaseColor ?? INCREASE;
  const decrease = cfg.decreaseColor ?? DECREASE;
  const total = cfg.totalColor ?? resolveColor(TOTAL_TOKEN, COLOR_FALLBACK.axisLabel);

  const categories: string[] = [];
  const base: number[] = [];
  const bars: { value: number; itemStyle: { color: string } }[] = [];
  /** The SIGNED delta each bar stands for. The bar's own value is its magnitude, because the bar is
   *  drawn upward from a floating base — so the sign has to be carried separately or the label and
   *  the screen-reader description both report a fall as a rise. */
  const deltas: number[] = [];
  /** Where the running total stands AFTER each step — the number a waterfall asks you to follow,
   *  and the one the tooltip reports under the step's own change. */
  const runningTotals: number[] = [];

  if (xi != null && mi != null) {
    // Order steps by the x column (temporal chronological, numeric ascending, nominal
    // kept as-is) so the running total builds in a sensible sequence — Metabase parity.
    const xType = columns[xi].type;
    const ordered = isTemporalType(xType)
      ? [...rows].sort((a, b) => Date.parse(String(a[xi])) - Date.parse(String(b[xi])))
      : isNumericType(xType)
        ? [...rows].sort((a, b) => Number(a[xi]) - Number(b[xi]))
        : rows;
    let running = 0;
    for (const r of ordered) {
      const delta = Number(r[mi]);
      if (!Number.isFinite(delta)) continue;
      const before = running;
      const after = running + delta;
      categories.push(String(r[xi] ?? ""));
      base.push(Math.min(before, after));
      bars.push({ value: Math.abs(delta), itemStyle: { color: delta >= 0 ? increase : decrease } });
      deltas.push(delta);
      runningTotals.push(after);
      running = after;
    }
    if (cfg.showTotal !== false && categories.length > 0) {
      categories.push("Total");
      base.push(0);
      bars.push({ value: running, itemStyle: { color: total } });
      deltas.push(running);
      runningTotals.push(running);
    }
  }

  // 10,000 is where a grouped number reaches six characters and "10K" starts winning — the same
  // threshold the cartesian axis uses, so a waterfall and a bar chart of the same data agree.
  const widest = base.reduce((n, b, i) => Math.max(n, Math.abs(b + bars[i].value), Math.abs(b)), 0);
  const compact = cfg.compactValues ?? widest >= 10_000;
  const fmt = (v: unknown, useCompact = false) => {
    if (cfg.metric == null) return String(v);
    const col = cfg.columnSettings?.[cfg.metric];
    return formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", {
      ...col,
      ...(useCompact ? { compact: col?.compact ?? compact } : {}),
    });
  };

  const xTitle = cfg.showXLabel === false ? undefined : (cfg.xLabel ?? cfg.xAxis);
  const yTitle = cfg.showYLabel === false ? undefined : (cfg.yLabel ?? cfg.metric);
  // Long step names are unreadable upright however few of them there are — the same threshold the
  // cartesian axis uses, so a waterfall and a bar chart of the same steps tilt together. An
  // explicit choice from the Axes tab always wins over the guess.
  const longest = categories.reduce((n, c) => Math.max(n, c.length), 0);
  const rotate = cfg.xLabelRotation ?? (longest > 10 && categories.length > 6 ? 30 : 0);

  const deltaSeries: Record<string, unknown> = {
    type: "bar",
    stack: "wf",
    stackStrategy: "all",
    barWidth: CHART_THEME.barWidth,
    data: bars,
    label: {
      show: !!cfg.showLabels,
      position: "top" as const,
      fontSize: CHART_THEME.labelFontSize,
      fontWeight: CHART_THEME.labelWeight,
      color: axisLabelColor,
      // The bar's own value is a magnitude; the label has to name the SIGNED step or a fall of 30
      // is announced as a rise of 30.
      formatter: (p: { dataIndex: number }) => fmt(deltas[p.dataIndex], true),
    },
    labelLayout: { hideOverlap: (cfg.labelDensity ?? "fit") === "fit" },
  };

  // Goal line — a horizontal reference at goalValue, drawn via the visible series' markLine, the
  // same construction the cartesian builder uses so the two look identical on a dashboard.
  if (cfg.goalValue != null) {
    deltaSeries.markLine = {
      silent: true,
      symbol: "none",
      data: [{ yAxis: cfg.goalValue }],
      label: { formatter: cfg.goalLabel ?? "Goal", position: "insideEndTop", color: axisLabelColor },
      lineStyle: { color: axisLabelColor, type: "dashed" },
    };
  }

  return {
    ...baseOption(),
    aria: ariaDescription({
      kind: "Waterfall chart", metric: cfg.metric, dimension: cfg.xAxis, format: (v) => fmt(v),
      points: categories.map((c, i) => ({ name: c, value: deltas[i] })),
    }),
    // An axis title needs room reserved on the edge it renders on, or ECharts draws it into the
    // tick labels and the plot clips it — the same adjustment the cartesian builder makes, and the
    // reason a shared `CHART_THEME.grid` alone is not enough once titles exist.
    grid: {
      ...CHART_THEME.grid,
      // A steeper tilt eats more vertical room than the 30° guess does.
      bottom: (CHART_THEME.grid.bottom as number) + (xTitle ? 22 : 0) + (rotate >= 90 ? 44 : rotate ? 18 : 0),
      left: (CHART_THEME.grid.left as number) + (yTitle ? 26 : 0),
    },
    // The shared `axisTooltip()` reports the series' own value, and the bar's value is a MAGNITUDE
    // — it read "Q2  60" where Q2 is a fall of 60. Metabase's tooltip reads the ORIGINAL signed
    // datum and tints its swatch by direction; this does the same, and adds the running total the
    // step arrives at, which is the number a waterfall is actually asking you to follow.
    tooltip: {
      trigger: "axis",
      axisPointer: { type: "shadow" },
      ...tooltipSurface(),
      formatter: (params: unknown) => {
        const list = Array.isArray(params) ? params : [params];
        const p = list.find((q) => (q as { seriesIndex?: number }).seriesIndex === 1) as
          | { dataIndex: number; name?: string } | undefined;
        if (!p) return "";
        const i = p.dataIndex;
        const delta = deltas[i];
        if (delta == null) return String(p.name ?? "");
        const isTotal = cfg.showTotal !== false && i === categories.length - 1;
        const swatch = bars[i]?.itemStyle.color ?? increase;
        const dot = `<span style="display:inline-block;width:10px;height:10px;border-radius:50%;background:${swatch};margin-right:6px"></span>`;
        const head = `<div style="font-weight:${TOOLTIP_WEIGHT.header}">${categories[i]}</div>`;
        const line = `<div>${dot}<span style="font-weight:${TOOLTIP_WEIGHT.value}">${fmt(delta)}</span></div>`;
        if (isTotal) return head + line;
        return `${head}${line}<div style="font-weight:${TOOLTIP_WEIGHT.name}">Running total: ${fmt(runningTotals[i])}</div>`;
      },
    },
    xAxis: {
      type: "category", data: categories,
      name: xTitle, nameLocation: "middle", nameGap: 28,
      nameTextStyle: { color: axisLabelColor },
      axisLabel: {
        fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, interval: "auto", rotate,
        formatter: (v: string) => {
          const f = cfg.xAxis ? cfg.columnSettings?.[cfg.xAxis] : undefined;
          if (!f || v == null) return String(v ?? "");
          const out = formatValue(v, columns[xi ?? 0]?.type ?? "varchar", f);
          return out === "" ? String(v) : out;
        },
      },
      axisLine: { show: cfg.xAxisTicks !== false, lineStyle: { color: gridLine } },
      axisTick: { show: cfg.xAxisTicks !== false },
    },
    yAxis: {
      type: cfg.logScale ? "log" : "value",
      name: yTitle, nameLocation: "middle", nameGap: 44,
      nameTextStyle: { color: axisLabelColor },
      ...(cfg.yMin != null ? { min: cfg.yMin } : {}),
      ...(cfg.yMax != null ? { max: cfg.yMax } : {}),
      ...(cfg.yTickCount != null ? { splitNumber: cfg.yTickCount } : {}),
      axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: (v: number) => fmt(v, true) },
      axisLine: { show: cfg.yAxisTicks !== false, lineStyle: { color: gridLine } },
      axisTick: { show: cfg.yAxisTicks !== false },
      splitLine: { lineStyle: { color: gridLine } },
    },
    series: [
      {
        type: "bar", stack: "wf", stackStrategy: "all", silent: true,
        itemStyle: { color: "transparent" }, emphasis: { itemStyle: { color: "transparent" } },
        tooltip: { show: false }, data: base,
      },
      deltaSeries,
    ],
  };
}
