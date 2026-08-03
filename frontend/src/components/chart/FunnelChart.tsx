import { useEffect, useRef, useState } from "react";
import { EChartsRenderer } from "./echarts/EChartsRenderer";
import { buildCartesianOption } from "./echarts/options/cartesian";
import {
  funnelSteps, stepOpacity, type FunnelBuildConfig, type FunnelStep, type ResultColumn,
} from "./echarts/options/funnel";
import { SERIES_PALETTE } from "./echarts/palette";

/**
 * Metabase-style Funnel — a port of its `FunnelNormal`, not an ECharts `funnel` series.
 *
 * The layout is N columns for N steps, but only N-1 of them hold a band:
 *
 *  - **The first step is an INFO column, not a shape.** It prints the step's name, then its value
 *    large and bold, vertically centred, with the metric's name under it. Nothing is drawn for it,
 *    because a funnel's first step is the whole of the flow — there is no narrowing to show yet.
 *    This is the part the previous rewrite got backwards: it drew a band for step 1 and a flat
 *    sliver for step N, which put a meaningless slab at the wrong end and left the reader without
 *    the one number the rest are shares of.
 *  - **Each later column holds the band ARRIVING at it**, a trapezoid from the previous step's
 *    edge to its own, plus that step's share of the first and its value underneath.
 *
 * Every band is the same brand colour at a falling opacity (`stepOpacity`). A palette per step
 * would say these are categories to compare; they are stages of one flow narrowing.
 */
const FUNNEL_COLOR = SERIES_PALETTE[0];

/**
 * The narrowest card that still gets the funnel's full-size typography. Below it, the type shrinks.
 *
 * Metabase decides this from GRID units — `isNarrow = gridSize.width < 7` — which egret has no
 * notion of, so the threshold had to be measured rather than translated. On Metabase's own
 * dashboard at viewport 1440 the grid is 1048px over 24 columns with a 6px margin, i.e. a card of
 * n columns is `43.875n - 6` px wide. Measured across the flip: size_x 7 renders at **301px and is
 * NOT narrow**; size_x 6 renders at **258px and IS narrow**. Seven columns is therefore 301.125px,
 * and every card below that has fewer than seven — so 301 is the first wide pixel, and this is a
 * `<` comparison. egret's own dashboard card is 184px, well inside the narrow side.
 *
 * A PIXEL THRESHOLD IS A PROXY, and the calibration above pins it to viewport 1440. Metabase's
 * predicate counts GRID COLUMNS, which do not change with the viewport while their pixel width
 * does: the same `MB · Funnel` card measured 345px wide at viewport 1440 and 299px at viewport
 * 962, and rendered WIDE at both — where this constant would call the second one narrow. Matching
 * that exactly would mean plumbing the dashboard's grid units down to the chart, which nothing in
 * egret's chart tree carries today. At egret's own dashboard widths the two agree, and a funnel
 * that shrinks its type a little early on a very narrow window is the harmless side to err on.
 */
const MIN_WIDE_CARD_WIDTH_PX = 301;

/**
 * The card height below which the funnel tightens its outer padding — Metabase's `isShort`,
 * `gridSize.height <= 5`, which measured ~225px of card height at the same viewport.
 *
 * It feeds only `isSmall` (`isShort || isNarrow`), and `isSmall` reaches only the root's padding.
 * The typography is `isNarrow`'s alone; collapsing the two into one flag would shrink the type on
 * a card that is merely short.
 */
const SHORT_CARD_HEIGHT_PX = 225;

/**
 * The funnel's own measured size, from a `ResizeObserver` on its root.
 *
 * `EChartsRenderer` gets its size the same way and for the same reason — it is the only place with
 * real DOM access — but it hands the number to an option BUILDER, which a plain React component
 * has no equivalent of. So the observer lives here rather than being borrowed from there.
 *
 * A zero width means "not laid out yet" (hidden container, first paint, jsdom), never "tiny": the
 * component keeps its full-size typography until a real measurement arrives, which is also how
 * Metabase behaves when `gridSize` is absent.
 */
function useMeasuredSize() {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const read = (width: number, height: number) => {
      if (width <= 0) return;
      setSize((prev) => (prev && prev.width === width && prev.height === height ? prev : { width, height }));
    };
    const rect = el.getBoundingClientRect();
    read(Math.round(rect.width), Math.round(rect.height));
    const ro = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect ?? el.getBoundingClientRect();
      read(Math.round(box.width), Math.round(box.height));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  return {
    ref,
    isNarrow: size != null && size.width < MIN_WIDE_CARD_WIDTH_PX,
    isShort: size != null && size.height <= SHORT_CARD_HEIGHT_PX,
  };
}

export function FunnelChart({
  columns, rows, dimension, metric, stepOrder, hiddenSteps, columnSettings, metricTypes, type = "funnel",
}: FunnelBuildConfig & {
  columns: ResultColumn[];
  rows: unknown[][];
  /** Metabase's `funnel.type`. "bar" is a real bar chart of the same steps — what its own
   *  `funnelToBarTransform` produces — for when the steps are not really one flow. */
  type?: "funnel" | "bar";
}) {
  const { ref, isNarrow, isShort } = useMeasuredSize();

  const steps = funnelSteps(columns, rows, {
    dimension, metric, stepOrder, hiddenSteps, columnSettings, metricTypes,
  });

  if (steps.length === 0) {
    return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>;
  }

  const label = `Funnel chart of ${metric ?? "a metric"} by ${dimension ?? "a dimension"}. ${
    steps.map((s) => `${s.name} ${s.label}${s.share ? `, ${s.share} of the first step` : ""}`).join("; ")
  }.`;

  if (type === "bar") {
    return <StepBars steps={steps} columns={columns} dimension={dimension} metric={metric}
      columnSettings={columnSettings} metricTypes={metricTypes} />;
  }

  const [first, ...rest] = steps;

  /**
   * Metabase's typography, as the `em` cascade its own `FunnelNormal.styled.tsx` states, with the
   * `isNarrow` branch of each rule folded in. Written as INLINE styles rather than Tailwind
   * classes for two reasons: an inline style is what actually wins in the browser (it is how
   * Metabase itself overrides these), and it is the only form a jsdom test can read back — a class
   * assertion passes for a class that does nothing.
   *
   * Measured on the live `MB · Funnel` card at viewport 1440, whose root is 14px; egret's own
   * `text-sm` is 13px (`--text-sm: 0.8125rem`), so the same multipliers land 13/14 lower here.
   * Both renders measured, Metabase's on its dashboard and egret's in Chromium:
   *
   * | element              | multiplier    | Metabase wide/narrow | egret wide/narrow |
   * |----------------------|---------------|----------------------|-------------------|
   * | step-1 value         | 1.72 / 1.29   | 24.08 / 18.06        | 22.36 / 16.77     |
   * | metric name          | 1.1825 / 0.86 | 16.555 / 12.04       | 15.3725 / 11.18   |
   * | share percentage     | 1.15 / 0.85   | 16.1 / 11.9          | 14.95 / 11.05     |
   * | step name            | 1 / 1         | 13 / 13              | 13 / 13           |
   * | per-step value       | 1 / 1         | 13 / 13              | 14.95 / 11.05     |
   *
   * Only the first three move with `isNarrow`. The last two are pinned by an inline `0.8125rem`
   * that Metabase applies when `isDashboard`, which beats its emotion rules entirely — so a port
   * that scaled all of the funnel's typography by one factor would not match it at either size.
   * egret has no `isDashboard` signal here and leaves both inheriting the root; the step name then
   * lands on Metabase's 13px exactly, while the per-step value does not (a separate fidelity gap,
   * not something `isNarrow` governs).
   */
  const font = {
    start: "1.72em",
    startValue: isNarrow ? "0.75em" : undefined,
    startMetric: isNarrow ? "0.5em" : "0.6875em",
    info: isNarrow ? "0.85em" : "1.15em",
  };

  return (
    <div
      data-funnel-root
      ref={ref}
      // `isSmall` in Metabase, which is `isShort || isNarrow` and reaches nothing but this padding.
      style={{ padding: isNarrow || isShort ? "0.5rem" : "1rem" }}
      className="flex h-full w-full text-sm text-muted-foreground"
      role="img"
      aria-label={label}
    >
      {/* The first step: its value is what every share below is a share OF, so it is set large and
          centred rather than drawn. `shrink-0` with no width — Metabase sizes this column to its
          own text and gives the whole remaining width to the bands. That is also why its type has
          to shrink on a small card: this column takes what it needs FIRST, so every pixel it keeps
          is one the later columns do not get, and at 184px they were left on their 20px floor. */}
      <div className="flex shrink-0 flex-col border-r">
        <div data-funnel-step-name className="truncate p-[0.5em] text-right" title={first.name}>{first.name}</div>
        <div
          style={{ fontSize: font.start }}
          className="flex min-h-0 flex-1 flex-col justify-center pr-[0.5em] text-right leading-tight"
        >
          <div data-funnel-value style={{ fontSize: font.startValue }}
            className="truncate font-bold tabular-nums text-foreground">{first.label}</div>
          {metric && <div data-funnel-metric style={{ fontSize: font.startMetric }}
            className="truncate" title={metric}>{metric}</div>}
        </div>
        {/* Invisible twin of the label block the other columns end with, so every band's bottom
            edge starts at the same y. It carries the same font-size as the real one, so the two
            keep matching across the narrow flip.

            Metabase's own spacer does NOT match: it omits the `isDashboard` inline size that the
            real block's value carries, which leaves its first column 114.89px tall against the
            graph columns' 110.45px. That ~4.4px misalignment is an upstream bug, not a design, and
            is deliberately not reproduced here. */}
        <div data-funnel-spacer aria-hidden style={{ fontSize: font.info }}
          className="invisible px-[0.5em] pt-[0.5em]">
          <div>&nbsp;</div>
          <div className="mt-[1em]">&nbsp;</div>
        </div>
      </div>

      {rest.map((step, i) => (
        <div key={step.name} className="flex min-w-[20px] flex-1 flex-col border-r">
          <div data-funnel-step-name className="truncate p-[0.5em] text-right" title={step.name}>{step.name}</div>
          <div className="min-h-0 flex-1">
            <Band from={steps[i].fraction} to={step.fraction} opacity={stepOpacity(i, rest.length)} />
          </div>
          <div data-funnel-info style={{ fontSize: font.info }} className="px-[0.5em] pt-[0.5em] text-right">
            <div data-funnel-share className="truncate">{step.share}</div>
            <div data-funnel-step-value className="mt-[1em] truncate tabular-nums">{step.label}</div>
          </div>
        </div>
      ))}
    </div>
  );
}

/**
 * Metabase's `funnel.type: "bar"` — a genuine bar chart, axes and legend and all.
 *
 * Each step is its own SERIES, not one series of four points, which is what gives every step its
 * own colour and its own legend entry: `funnelToBarTransform` builds one single-row card per step
 * and stacks them. Reproduced here by widening the result — one column per step, each row carrying
 * a value only in its own column — and stacking, so the four series land in four x positions
 * instead of being grouped side by side inside each one.
 */
function StepBars({ steps, columns, dimension, metric, columnSettings, metricTypes }: {
  steps: FunnelStep[];
  columns: ResultColumn[];
} & Pick<FunnelBuildConfig, "dimension" | "metric" | "columnSettings" | "metricTypes">) {
  const dimName = dimension ?? "step";
  const metricType = (metric ? metricTypes?.[metric] : undefined) ?? "double";
  const format = metric ? columnSettings?.[metric] : undefined;
  const names = steps.map((s) => s.name);

  const wideColumns: ResultColumn[] = [
    { name: dimName, type: columns.find((c) => c.name === dimName)?.type ?? "varchar" },
    ...names.map((name) => ({ name, type: metricType })),
  ];
  const wideRows = steps.map((s, i) => [s.name, ...steps.map((_, j) => (i === j ? s.value : null))]);

  return (
    <EChartsRenderer
      option={buildCartesianOption(wideColumns, wideRows, {
        kind: "bar",
        xAxis: dimName,
        yMetrics: names,
        // Stacked, and the y-axis never split: with one series per step an automatic split would
        // read the steps as two different measures. Metabase pins both the same way.
        stacked: true,
        autoSplitY: false,
        // A pipeline runs Visited → Added to cart → Checkout → Purchased. `steps` is already in
        // that order (`funnel.stepOrder`), and a nominal axis otherwise draws ascending now — which
        // would alphabetise the funnel into Added to cart → Checkout → Purchased → Visited.
        preserveCategoryOrder: true,
        // The axis is still measuring the one metric, whatever the series are called.
        yLabel: metric,
        metricTypes: Object.fromEntries(names.map((n) => [n, metricType])),
        columnSettings: format ? Object.fromEntries(names.map((n) => [n, format])) : undefined,
      })}
    />
  );
}

/**
 * One trapezoid, in a unit viewBox stretched to the column — Metabase's own construction, which is
 * why the shape survives any column width without the slope distorting.
 *
 * An edge of height `f` is centred: it runs from `(1-f)/2` to `1-(1-f)/2`. Measured against the
 * live chart, `0 0.10719 … 0.89281` for `3976/5061 = 0.78562`.
 */
function Band({ from, to, opacity }: { from: number; to: number; opacity: number }) {
  const pts = `0,${(1 - from) / 2} 1,${(1 - to) / 2} 1,${1 - (1 - to) / 2} 0,${1 - (1 - from) / 2}`;
  return (
    <svg data-funnel-band viewBox="0 0 1 1" preserveAspectRatio="none" className="h-full w-full">
      <polygon points={pts} fill={FUNNEL_COLOR} opacity={opacity} />
    </svg>
  );
}

export type { FunnelStep };
