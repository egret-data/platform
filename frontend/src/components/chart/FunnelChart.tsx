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

export function FunnelChart({
  columns, rows, dimension, metric, stepOrder, hiddenSteps, columnSettings, metricTypes, type = "funnel",
}: FunnelBuildConfig & {
  columns: ResultColumn[];
  rows: unknown[][];
  /** Metabase's `funnel.type`. "bar" is a real bar chart of the same steps — what its own
   *  `funnelToBarTransform` produces — for when the steps are not really one flow. */
  type?: "funnel" | "bar";
}) {
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

  return (
    <div className="flex h-full w-full p-4 text-sm text-muted-foreground" role="img" aria-label={label}>
      {/* The first step: its value is what every share below is a share OF, so it is set large and
          centred rather than drawn. `shrink-0` with no width — Metabase sizes this column to its
          own text and gives the whole remaining width to the bands. */}
      <div className="flex shrink-0 flex-col border-r">
        <div className="truncate p-[0.5em] text-right" title={first.name}>{first.name}</div>
        <div className="flex min-h-0 flex-1 flex-col justify-center pr-[0.5em] text-right text-[1.72em] leading-tight">
          <div className="truncate font-bold tabular-nums text-foreground">{first.label}</div>
          {metric && <div className="truncate text-[0.6875em]" title={metric}>{metric}</div>}
        </div>
        {/* Invisible twin of the label block the other columns end with, so every band's bottom
            edge starts at the same y. Metabase reserves the space the same way. */}
        <div aria-hidden className="invisible px-[0.5em] pt-[0.5em] text-[1.15em]">
          <div>&nbsp;</div>
          <div className="mt-[1em]">&nbsp;</div>
        </div>
      </div>

      {rest.map((step, i) => (
        <div key={step.name} className="flex min-w-[20px] flex-1 flex-col border-r">
          <div className="truncate p-[0.5em] text-right" title={step.name}>{step.name}</div>
          <div className="min-h-0 flex-1">
            <Band from={steps[i].fraction} to={step.fraction} opacity={stepOpacity(i, rest.length)} />
          </div>
          <div className="px-[0.5em] pt-[0.5em] text-right text-[1.15em]">
            <div className="truncate">{step.share}</div>
            <div className="mt-[1em] truncate tabular-nums">{step.label}</div>
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
