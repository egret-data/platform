import { useEffect, useRef, useState, type ReactNode } from "react";
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

/** One step's contribution to the chart's spoken description, reused as the accessible name of its
 *  own button once the funnel is clickable. */
const stepSpoken = (s: FunnelStep) => `${s.name} ${s.label}${s.share ? `, ${s.share} of the first step` : ""}`;

/**
 * One funnel column — a plain `div` normally, a real `button` when the chart is drillable.
 *
 * A BUTTON RATHER THAN AN `onClick` ON THE DIV, which is what the table and pivot cells do. Those
 * two are inside a `table`, where a screen reader can already reach every cell by its own grid
 * navigation and a click is an extra affordance on something already reachable; a funnel column is
 * a bare div inside what was a `role="img"`, so an `onClick` there would be a drill only a mouse
 * could ever perform. `BigNumber` made the same call for the same reason and is the precedent this
 * follows.
 *
 * `text-left` because a UA styles a button's text `center` and the column's own children set their
 * alignment; without it the un-aligned bits (nothing today, but any future child) would drift.
 * `bg-transparent`/`p-0` are Tailwind preflight's already, and are not repeated here.
 */
function StepColumn({ step, onStepClick, className, children }: {
  step: FunnelStep;
  onStepClick?: (step: FunnelStep, x: number, y: number) => void;
  className: string;
  children: ReactNode;
}) {
  if (!onStepClick) return <div data-funnel-step className={className}>{children}</div>;
  return (
    <button
      type="button"
      data-funnel-step
      // The same name the chart's own summary gives this step, plus what the click will do — the
      // shape `BigNumber`'s "…. Show records" already uses.
      aria-label={`${stepSpoken(step)}. Show records`}
      className={`${className} cursor-pointer text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring`}
      onClick={(e) => onStepClick(step, e.clientX, e.clientY)}
    >
      {children}
    </button>
  );
}

export function FunnelChart({
  columns, rows, dimension, metric, stepOrder, hiddenSteps, columnSettings, metricTypes, type = "funnel",
  onStepClick, onFinished,
}: FunnelBuildConfig & {
  columns: ResultColumn[];
  rows: unknown[][];
  /** Metabase's `funnel.type`. "bar" is a real bar chart of the same steps — what its own
   *  `funnelToBarTransform` produces — for when the steps are not really one flow. */
  type?: "funnel" | "bar";
  /** Present when the chart is drillable — `ChartRenderer` turns the step into a `DrillContext`,
   *  because that is where the dimension column and its temporal unit already are (the same
   *  division of labour the waterfall, box plot and sankey branches use).
   *
   *  It is handed the whole `FunnelStep` rather than its name so that the RAW dimension value is
   *  the only thing this component can pass on: `FunnelStep.name` is `String(row[dimensionIndex])`
   *  (see `funnelSteps`), never a label. Nothing here reads back the text it drew. */
  onStepClick?: (step: FunnelStep, x: number, y: number) => void;
  /** See `EChartsRenderer`'s own doc comment. Forwarded to it below for the "bar" mode, which
   *  draws through `EChartsRenderer`; fired directly for "No data", which never reaches it. */
  onFinished?: () => void;
}) {
  const { ref, isNarrow, isShort } = useMeasuredSize();

  const steps = funnelSteps(columns, rows, {
    dimension, metric, stepOrder, hiddenSteps, columnSettings, metricTypes,
  });

  // Declared before the early return below so the hook still runs on every render, per the rules
  // of hooks. Two of this component's three shapes draw synchronously — "No data" and the default
  // trapezoid funnel (plain markup below, no ECharts involved) — so both fire here; only "bar"
  // mode defers to `StepBars`' own `EChartsRenderer`, which is handed `onFinished` below instead.
  useEffect(() => {
    if (steps.length === 0 || type !== "bar") onFinished?.();
  }, [steps.length, type, onFinished]);

  if (steps.length === 0) {
    return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>;
  }

  const label = `Funnel chart of ${metric ?? "a metric"} by ${dimension ?? "a dimension"}. ${
    steps.map(stepSpoken).join("; ")
  }.`;

  if (type === "bar") {
    return <StepBars steps={steps} columns={columns} dimension={dimension} metric={metric}
      columnSettings={columnSettings} metricTypes={metricTypes} onStepClick={onStepClick} onFinished={onFinished} />;
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
      // `img` COLLAPSES ITS SUBTREE — that is the whole point of it, and it is why an unclickable
      // funnel is spoken as one sentence rather than as thirteen loose fragments. It is also why
      // it cannot stay once the steps become buttons: a control inside `role="img"` is not
      // reachable at all. `group` keeps the same `aria-label` as the chart's name while leaving the
      // children exposed, so the summary survives and the steps become operable. `BigNumber` makes
      // the same trade at the same seam, dropping its own `role="img"` the moment it is clickable.
      role={onStepClick ? "group" : "img"}
      aria-label={label}
    >
      {/* The first step: its value is what every share below is a share OF, so it is set large and
          centred rather than drawn. `shrink-0` with no width — Metabase sizes this column to its
          own text and gives the whole remaining width to the bands. That is also why its type has
          to shrink on a small card: this column takes what it needs FIRST, so every pixel it keeps
          is one the later columns do not get, and at 184px they were left on their 20px floor.

          CLICKABLE, AND METABASE'S IS NOT — a deliberate divergence, measured rather than assumed.
          `FunnelNormal.tsx:140` builds its click infos from `funnelSteps.slice(1)`, and only those
          infos get a `GraphSection`, which is where the handler hangs; the `FunnelStart` block
          (lines 214-234) has none. That is an artifact of the first step having no band to draw,
          not a decision about what the step means: it is a row of the result with a dimension value
          like every other one, and "Visited: 131,320" is the number the whole funnel is a share of,
          so being unable to ask what is behind it is a hole rather than a rule. Same call this file
          already makes about Metabase's mismatched first-column spacer below. */}
      <StepColumn step={first} onStepClick={onStepClick} className="flex shrink-0 flex-col border-r">
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
      </StepColumn>

      {rest.map((step, i) => (
        <StepColumn key={step.name} step={step} onStepClick={onStepClick}
          className="flex min-w-[20px] flex-1 flex-col border-r">
          <div data-funnel-step-name className="truncate p-[0.5em] text-right" title={step.name}>{step.name}</div>
          <div className="min-h-0 flex-1">
            <Band from={steps[i].fraction} to={step.fraction} opacity={stepOpacity(i, rest.length)} />
          </div>
          <div data-funnel-info style={{ fontSize: font.info }} className="px-[0.5em] pt-[0.5em] text-right">
            <div data-funnel-share className="truncate">{step.share}</div>
            <div data-funnel-step-value className="mt-[1em] truncate tabular-nums">{step.label}</div>
          </div>
        </StepColumn>
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
function StepBars({ steps, columns, dimension, metric, columnSettings, metricTypes, onStepClick, onFinished }: {
  steps: FunnelStep[];
  columns: ResultColumn[];
  onStepClick?: (step: FunnelStep, x: number, y: number) => void;
  onFinished?: () => void;
} & Pick<FunnelBuildConfig, "dimension" | "metric" | "columnSettings" | "metricTypes">) {
  const dimName = dimension ?? "step";
  const metricType = (metric ? metricTypes?.[metric] : undefined) ?? "double";
  const format = metric ? columnSettings?.[metric] : undefined;
  /**
   * THE RAW CELL IS THE KEY; THE FORMATTED NAME IS ONLY A LABEL.
   *
   * Every step is its own single-valued series here, so a step's name is doing three jobs at once:
   * the x category, the series id, and the string `onPointClick` matches back to find which step
   * was clicked. Once `funnelSteps` began formatting the name (see its `raw` field) two steps could
   * share one — a date style of "MMMM YYYY" collapses every day of a month onto "January 2026" —
   * and the `.find` below would then have answered the click with whichever came first. Keying on
   * `raw` keeps the identity exactly what it was before the format existed and leaves the format
   * doing the one thing it is for.
   *
   * The label reaches the chart by two routes rather than one, because the chart draws it in two
   * places. `seriesLabels` is what the legend and the labels read. The AXIS tick is not a series
   * name at all, so it comes from the x column's own stored format through `buildCartesianOption`'s
   * `drawLabel` — the cartesian builder has honoured that since the axis-format fix, so this hands
   * it the dimension's `column_settings` and gets the same string the trapezoid mode draws, out of
   * the same rule rather than a second copy of it.
   */
  const keys = steps.map((s) => s.raw);
  const dimFormat = dimension ? columnSettings?.[dimension] : undefined;

  const wideColumns: ResultColumn[] = [
    { name: dimName, type: columns.find((c) => c.name === dimName)?.type ?? "varchar" },
    ...keys.map((name) => ({ name, type: metricType })),
  ];
  const wideRows = steps.map((s, i) => [s.raw, ...steps.map((_, j) => (i === j ? s.value : null))]);

  return (
    <EChartsRenderer
      // A function of the measured size, like every other cartesian option: without it the x-axis
      // label fit has nothing to measure against, the axis falls back to ECharts' `interval:
      // "auto"`, and two of five stage names vanished at 1440px (KI-164).
      option={(size) => buildCartesianOption(wideColumns, wideRows, {
        kind: "bar",
        xAxis: dimName,
        yMetrics: keys,
        seriesLabels: Object.fromEntries(steps.map((s) => [s.raw, s.name])),
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
        metricTypes: Object.fromEntries(keys.map((n) => [n, metricType])),
        // The metric's format on every pseudo-metric column, AND the dimension's on the x column —
        // the second is what makes the axis ticks read the same as the trapezoid mode's step names.
        columnSettings: {
          ...(format ? Object.fromEntries(keys.map((n) => [n, format])) : {}),
          ...(dimFormat ? { [dimName]: dimFormat } : {}),
        },
      }, size)}
      // THE CARTESIAN ADAPTER IS NOT REUSABLE HERE, and it is worth saying why rather than leaving
      // a reader to wonder: this option's x column is `dimName` but its SERIES are the step names,
      // one single-valued series per step. `ChartRenderer`'s cartesian handler drills `xAxis` plus
      // `cartesian.series` — and `cartesian.series` is a COLUMN name there, whereas here every step
      // is its own series and the second dimension does not exist. Pointing that handler at this
      // option would emit a second, imaginary dim.
      //
      // What it needs is one line, and the reason it is one line is that `applyXScale` puts the RAW
      // category on the item's `name` (see the long note in `ChartRenderer`'s cartesian branch), so
      // `p.name` is the step's own dimension value with no label anywhere in the path. The lookup
      // back into `steps` is a guard, not a translation: it refuses a click on anything the funnel
      // did not draw as a step.
      onPointClick={onStepClick && ((p) => {
        // `s.raw`, not `s.name`: the category on the option is the raw cell — see `keys` above.
        const step = steps.find((s) => s.raw === p.name);
        if (step) onStepClick(step, p.x, p.y);
      })}
      onFinished={onFinished}
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
