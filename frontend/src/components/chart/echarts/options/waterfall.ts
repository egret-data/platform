import type { EChartsCoreOption } from "echarts/core";
import type { EChartsSize } from "../EChartsRenderer";
import type { ColumnFormat } from "../../format-types";
import { sortCategories } from "./cartesian";
import { formatValue, specifiesDateRendering } from "../../format";
import { boundarySteps, namesAPeriod, periodName } from "../../temporal-period";
import {
  bottomNameGapClearOf, categoryLabelBand, estimatePlotWidth, fitXLabels, plotHeightLessLabels,
  verticalValueAxisFit, widestLabelWidth, type XLabelFit,
} from "../axisLabelFit";
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
  /** Metabase's "Compact" x-axis mode. This chart HAS a label formatter for it to act through —
   *  `drawLabel` below — which is what makes it different from the box plot. */
  xLabelCompact?: boolean;
  yTickCount?: number;
  yMin?: number;
  yMax?: number;
  logScale?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
  /**
   * The rows already arrive in an order the QUERY asked for, so leave a nominal step axis alone —
   * the same field `CartesianBuildConfig` and `BoxplotBuildConfig` carry, read by the same
   * `sortCategories`, and set by `ChartRenderer` for any chart whose stored spec has a non-empty
   * `orderby`. A spec carrying `orderby: []` must NOT set it: arrival order is then the engine's
   * whim, and on a waterfall that whim is arithmetic, not just arrangement.
   *
   * Unlike the funnel's bar mode there is no second setter here, because a waterfall has no
   * `funnel.stepOrder` to be ranked by.
   */
  preserveCategoryOrder?: boolean;
  /**
   * The truncation unit the x column was BUCKETED by, from `params.spec.temporal_units` —
   * `CartesianBuildConfig.xAxisUnit` is the same field fed from the same place, and `ChartRenderer`
   * fills both on the same line of its own branch.
   *
   * It arrived late. This builder shipped without it, so a waterfall grouped by month drew
   * "2026-01-01" — the bucket's first DAY — where the step covers January, and after PR 02 made the
   * axis all-or-nothing those ten characters × twelve steps meant a one-third-width card drew NO
   * x labels at all. It is what `drawLabel` names the period from and the only door to thinning.
   *
   * An EXTRACTION unit ("day_of_week") may arrive here too, and must do nothing: `namesAPeriod`
   * rejects it and `periodName` hands its value back untouched. Unlike cartesian this builder does
   * not substitute extraction values through `temporalLabel` at all — a pre-existing gap, out of
   * this change's scope, and named so the silence is not read as a claim.
   */
  xAxisUnit?: string;
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

/** The colour the Total bar is drawn in when none is chosen. One function, so the settings swatch
 *  shows the bar's actual colour rather than a stand-in for it (KI-057). */
export function defaultTotalColor(): string {
  return resolveColor(TOTAL_TOKEN, COLOR_FALLBACK.axisLabel);
}

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
export function buildWaterfallOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: WaterfallBuildConfig,
  /** The container's measured size, from `EChartsRenderer`. Absent when the caller has no
   *  measurement — a unit test, or anything handing over a plain option object; see the x-label fit
   *  below, which is the only thing that reads it. */
  size?: EChartsSize,
): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const xi = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const increase = cfg.increaseColor ?? INCREASE;
  const decrease = cfg.decreaseColor ?? DECREASE;
  const total = cfg.totalColor ?? defaultTotalColor();

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
    /**
     * Order the steps by the x column, through the SAME `sortCategories` the bar, row and box
     * charts use — a nominal axis included, which is what this line used to leave in arrival order.
     *
     * A stored spec carries `orderby: []`, so arrival order is whatever the engine happened to
     * return: measured across ten calls to one chart's data endpoint as
     * `['Widget','Gadget','Doohickey','Gizmo']` on seven and
     * `['Gizmo','Widget','Gadget','Doohickey']` on the other three (PR #394). Three consecutive
     * page loads landed on the same one, which is how it survived being called deterministic.
     *
     * A WATERFALL HAS MORE AT STAKE THAN A BAR CHART, not less, and that is why it does not get the
     * funnel's `preserveCategoryOrder` exemption: the running total below builds in this order, so
     * an arbitrary one does not merely shuffle the bars, it changes which step is first and
     * therefore every cumulative value drawn. The funnel is exempt because its steps arrive ranked
     * by `funnel.stepOrder` and alphabetising a pipeline is worse than any engine order; nothing in
     * `WaterfallConfig` states a step order that way — Metabase's waterfall has no such setting
     * either, only its cartesian `graph.*` ones.
     *
     * METABASE DRAWS THE SORTED ORDER TOO, by the other layer: `getWaterfallChartModel` calls the
     * shared `sortDataset`, which returns an ORDINAL dataset unchanged — but the dataset reaching it
     * is already ascending by the breakout, because the query processor adds an implicit ascending
     * `order-by` for every breakout column before the query runs
     * (`add_implicit_clauses.clj`, `add-implicit-breakout-order-by`). So sorting here is parity with
     * what Metabase renders, arrived at where egret can: its stored specs carry no such clause.
     *
     * `preserveCategoryOrder` is the case where an author asked for an order anyway. The waterfall
     * descriptor passes `shared.orderby` through untouched, so PR #395's sort control IS offered on
     * one; the engine then returns the rows in that order and `ChartRenderer` sets the flag. Sorting
     * them again here is how a sort control ends up inert on screen while its own tests pass.
     */
    const xType = columns[xi].type;
    // One entry per distinct step, in arrival order, so `sortCategories` is handed the same list a
    // bar chart's `distinct(xrows.map(...))` hands it — and rows sharing a step stay together
    // whichever way it answers.
    const byStep = new Map<unknown, unknown[][]>();
    for (const r of rows) {
      const at = byStep.get(r[xi]);
      if (at) at.push(r); else byStep.set(r[xi], [r]);
    }
    const ordered = sortCategories([...byStep.keys()], xType, cfg.preserveCategoryOrder)
      .flatMap((k) => byStep.get(k) ?? []);
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

  /**
   * What a step is DRAWN AS — the cartesian builder's `drawLabel`, same three arms in the same
   * order, and deliberately not a second idiom. Hoisted above the fit below rather than written
   * inline on `axisLabel.formatter`, because the fitter has to measure THIS STRING and nothing
   * else; see the `labels:` line at the call site for what measuring the other one cost there.
   *
   *   1. the x column's own format, when it says something the formatter for this column would
   *      consult. "Says something" rather than "exists": on a BUCKETED axis, where arm 2 is in play,
   *      a format silent about dates would otherwise beat the period name with `formatDate`'s
   *      DEFAULTS, and the panel mints exactly such objects — renaming the column to "Month" stores
   *      `{column_title: "Month"}`, which is truthy (`settings/FormatSection.tsx`, `patch`). That
   *      reverted the cartesian axis from "January 2026" to "January 1, 2026" with nothing in the
   *      panel to explain it, and this arm would have shipped the same defect a second time. Off a
   *      bucketed axis there is nothing for the format to outrank, so it still wins on existence
   *      there, exactly as this formatter always did.
   *   2. otherwise the NAME OF THE PERIOD the step's bucket covers — "January 2026" for a month,
   *      not its first day.
   *   3. otherwise the raw value. `periodName` is total and falls through to the raw string itself,
   *      so it IS answer 3 as well — the previous `String(v ?? "")` is what it returns for a step
   *      with no unit, and `""` for a null. `namesAPeriod` is asked only whether arm 2 EXISTS to be
   *      outranked, never to guard the call.
   *
   * The "Total" step goes through this too and comes out unchanged: `dayjs("Total")` is invalid, so
   * `periodName` returns it verbatim. That is arm 3 doing its job, not a special case.
   *
   * `columns[xi ?? 0]?.type` is the pre-existing lookup, kept: `xi` is undefined only when `cfg.xAxis`
   * names no column in the result, and then `columnSettings?.[cfg.xAxis]` decides nothing anyway
   * because there are no categories to draw.
   */
  const bucketedX = namesAPeriod(cfg.xAxisUnit);
  /**
   * …AND THE WATERFALL GETS METABASE'S "Compact" TOO, which overturns a decision this repo had
   * recorded twice. `settings/ChartAxesSection.tsx` said Compact "shortens temporal axis labels,
   * and these charts' x is a category run through its column format, so the option would do nothing
   * here", and `CartesianDisplaySection.tsx` repeated the premise. **The premise is false for this
   * chart**: `bucketedX` two lines up is `namesAPeriod(cfg.xAxisUnit)` and arm 2 below is
   * `periodName(cfg.xAxisUnit, v)` — the same two arms, on the same module, as the cartesian
   * builder's. A month-bucketed waterfall draws "January 2026" today, so there is a temporal label
   * here to shorten and the reason for withholding it does not apply.
   *
   * IT REMAINS TRUE OF THE BOX PLOT, which is why that chart still does not get the option — but
   * for a narrower reason than when this was written. Its category axis DOES have an
   * `axisLabel.formatter` now (`options/boxplot.ts`'s `catLabel`, which reads the category column's
   * own `column_settings`, where before it drew the raw group key); what it still has is no PERIOD
   * NAMING, so with nothing stored it falls back to the raw cell and a formatting flag has nothing
   * to act through. The original decision was right about one of the two charts it covered and
   * wrong about the other — see the note in `settings/ChartAxesSection.tsx`, where the split is
   * stated as structure and the narrowed argument is written out.
   *
   * The three-line shape is the cartesian builder's, deliberately, including which of `stored` and
   * `fmt` `specifiesDateRendering` is asked about — that comment carries the measurement.
   */
  const compactX = cfg.xLabelCompact === true;
  const drawLabel = (v: string) => {
    const stored = cfg.xAxis ? cfg.columnSettings?.[cfg.xAxis] : undefined;
    const colFmt = compactX ? { ...stored, compact: true, date_abbreviate: true } : stored;
    const formatWins = colFmt != null && (bucketedX ? specifiesDateRendering(stored) : true);
    if (formatWins && v != null) {
      const out = formatValue(v, columns[xi ?? 0]?.type ?? "varchar", colFmt);
      if (out !== "") return out;
    }
    return periodName(cfg.xAxisUnit, v, compactX);
  };

  // Whether the step names fit is a question about WIDTH, and until `size` existed this builder
  // could not ask it: the rule here was `longest > 10 && categories.length > 6 ? 30 : 0`, and
  // length and count are only proxies for width. The same rule was deleted from the cartesian
  // builder for the same reason — four categories whose longest is "Doohickey" on a
  // one-third-width card came out as "no rotation needed", and then `interval: "auto"` let ECharts
  // drop three of the four, an axis reading as missing data. See `echarts/axisLabelFit.ts`.
  //
  // A waterfall and a bar chart of the same steps now measure the same WAY, but not necessarily to
  // the same ANSWER — the deleted rule made them share a guess, and this does not restore that.
  // Two inputs differ on identical rows, and both push this chart towards rotating first:
  //   - `showTotal` defaults on, so this axis carries n+1 categories where the bar chart has n.
  //     That is 14% less `dimensionWidth` at six steps, and it is a real extra label, not an
  //     accounting artefact.
  //   - `widest` below is the running-total ENVELOPE, where cartesian's `widestOf` is the largest
  //     single datum. Deltas of 10/20/30 give a bar chart a "30" gutter and this one a "60" gutter,
  //     and the gap widens with every step.
  // A waterfall therefore tilts slightly sooner than the bar chart of its own steps, by design.
  // Worth knowing before reading the two side by side and calling the difference a bug.
  //
  // Cartesian guards this on `ordinalX` — is the category axis the BOTTOM one, and are its labels
  // the array we hold? A waterfall has neither a horizontal mode nor an `xScale`, so both halves of
  // that question are settled by construction: this axis is always a bottom category axis over
  // exactly these `categories`. All that is left to ask is whether there is a size to measure
  // against, and copying a condition that is always true here would read as if there were more.
  //
  // A zero size is not a small chart, it is an UNMEASURED one: `EChartsRenderer` reads
  // `getBoundingClientRect()` before `init`, so a hidden or not-yet-laid-out container reports
  // `{0, 0}`. Measuring it would give a negative `plotWidth` — which the fitter answers "draw every
  // label upright", from a container nobody could see — and a zero height would make the rotated
  // label's projected share infinite and hide the axis. Both are decisions taken against a guessed
  // width, so decline instead.
  const measurable = size != null && size.width > 0 && size.height > 0;
  // An explicit rotation SKIPS the measurement rather than merely outranking it. Computing the fit
  // anyway and using it for `show` would honour the user's 45° and hide their axis in the same
  // breath, which is worse than either outcome alone and looks like the setting doing nothing.
  // Metabase's own auto-selection likewise applies only while the setting is untouched.
  const explicitRotate = cfg.xLabelRotation != null;
  const fit: XLabelFit | undefined = measurable && size && !explicitRotate
    ? fitXLabels({
        // THE STRING THE AXIS ACTUALLY DRAWS, by construction — same closure, same argument.
        //
        // This measured `categories`, the RAW step name, and justified the gap as "the formatted
        // label is the wider of the two, so this under-measures". Naming the periods falsifies both
        // halves at once, the same way it did on the cartesian axis. The two now differ whenever
        // `xAxisUnit` is a truncation unit — the common case and this change's whole subject, not
        // the rare `columnSettings` one that comment named — and the period name is often NARROWER,
        // not wider: a year measured "2026-01-01" at ten characters and drew "2026" at four. So it
        // had begun to OVER-measure, which rotates labels that would have sat upright, in the change
        // whose entire motivation is labels not fitting.
        labels: categories.map(drawLabel),
        // The gutter is the widest value the y-axis can print, through the y-axis's own formatter
        // (`fmt(v, true)` below), and `yTitle` is what adds 26 to `grid.left` below. The VALUE and
        // the formatter, not the formatted value: `estimatePlotWidth` rounds to a tick the axis
        // would actually draw before measuring, which a pre-formatted string cannot be.
        plotWidth: estimatePlotWidth(size, widest, (v) => fmt(v, true), Boolean(yTitle)),
        // The CONTAINER's height, not an inset one: the 70% guard is a share of the whole card
        // (Metabase's `checkHeight`), so passing a grid-adjusted height silently tightens it.
        outerHeight: size.height,
        // THE PRECONDITION FOR THINNING, enforced here rather than assumed downstream: only a
        // BUCKETED temporal axis gets one. `bucketedX` is `namesAPeriod(cfg.xAxisUnit)`, the same
        // single membership rule `drawLabel` above uses, so an axis that is thinned is by
        // construction an axis whose labels are period names. On a waterfall of named stages this is
        // `undefined` and the fitter cannot reach its stride arm at all — there is no such thing as
        // every second stage. (Both fields are also empty for a non-period unit, so the guard holds
        // a second time even if this line is ever loosened.)
        //
        // ONLY THE LADDER, no sizing candidate. This used to pass `widestLabel:
        // widestPeriodName(cfg.xAxisUnit, drawLabel)` as well — the widest name the unit could
        // produce. The fitter now slices each rung's drawn labels out of `labels:` above instead;
        // see `strideThatFits` for the browser measurement and for what that concedes.
        //
        // ONE THING A WATERFALL HAS THAT A BAR CHART DOES NOT: the appended "Total" is a category
        // that is not a period. It costs TWO things, neither a blocker, and both are named here
        // because a partial account of a known cost reads as a complete one.
        //
        //   - IT CAN GO UNLABELLED. It is the LAST category, so a stride that does not land on its
        //     index skips it. That is the right trade at the only width where it arises — thinning
        //     is reached only once every adjacent pair collides, and the answer without it is PR
        //     02's rotate-or-hide, under which the Total's label is tilted or gone anyway. The
        //     bars, the tooltip and the aria description all still name it.
        //   - IT MOVES WHICH RUNG IS THE LAST ONE. `strideThatFits` stops at the first rung at or
        //     past the CATEGORY count, and the Total makes that count one higher — so twelve months
        //     here reach the 24 rung where a bar chart of the same twelve stops at 12. That was a
        //     defect until the budget was clamped: rung 24 was offered `24 x dimensionWidth`, near
        //     twice the plot it had, so this axis thinned on the live dashboard at a width where the
        //     line chart beside it hid. Both now measure real drawn labels against the same clamped
        //     plot; what survives is only that the two land on different RUNGS, and every rung at or
        //     past the count draws index 0 and nothing else, so they draw the same axis.
        //
        // (A third cost is gone rather than fixed: the Total was not covered by the old sizing
        // candidate, so at `year` — the one unit whose period name, "2026", is shorter than "Total"
        // — an on-stride Total could sit a couple of pixels past the budget its stride was chosen
        // under. It is on the drawn set now, so it is measured like any other category.)
        boundary: bucketedX ? { steps: boundarySteps(cfg.xAxisUnit) } : undefined,
      })
    : undefined;
  const rotate = cfg.xLabelRotation ?? (fit?.kind === "rotate" ? fit.rotate : 0);
  // Either way the axis has now DECIDED, and a decided axis never leaves ECharts the subset
  // `interval: "auto"` would let it pick. What it draws is every label, none, or — on a bucketed
  // temporal axis only — every Nth, which the `interval` line below argues is not the same thing.
  const decided = fit != null || explicitRotate;

  /**
   * THE OTHER AXIS, ON THE OTHER DIMENSION — the same fit the cartesian and box-plot builders make,
   * from the same module, because all three lay out the same grid and a third copy of the arithmetic
   * would be a third place for it to drift.
   *
   * NOT THE CARD THE DEFECT WAS FOUND ON, and that is worth saying rather than leaving to be
   * inferred: on the live 166x184 dashboard the crowding was the BAR and the BOX PLOT, at a 10.7px
   * pitch, while this chart's twelve monthly steps thin to upright period names and leave a plot
   * tall enough for six ticks at 21px. `verticalValueAxisFit` is a ceiling, so that card's
   * option comes out byte-identical. What brings this builder in is the case where the same steps
   * are NOT period names — a waterfall over product categories turns its labels 90° exactly as a bar
   * chart does, reserves the same 72px, and then draws its ticks into each other. Same axis, same
   * grid, same remedy.
   */
  const measuredCategories = measurable ? widestLabelWidth(categories.map(drawLabel)) : 0;
  const categoryGutter = measuredCategories > 0 ? measuredCategories : undefined;
  const categoryBand = categoryGutter != null && fit?.kind !== "hide"
    ? categoryLabelBand({ rotate, widestLabel: categoryGutter })
    : 0;
  // The insets this builder is about to emit, hoisted so the fit subtracts the grid it really uses.
  const grid = {
    ...CHART_THEME.grid,
    bottom: (CHART_THEME.grid.bottom as number) + (xTitle ? 22 : 0),
    left: (CHART_THEME.grid.left as number) + (yTitle ? 26 : 0),
  };
  /**
   * How far the value axis runs. A waterfall's bars float: each spans `base[i]` to
   * `base[i] + bars[i].value`, so the axis has to hold both ends of every step and the origin the
   * running total starts from. `widest` above is the same set collapsed to a magnitude; this keeps
   * the two ends, because an interval divides a DISTANCE — a series that dips to -40 and peaks at
   * +60 is 100 across while its biggest number is 60.
   */
  const tops = base.map((b, i) => b + bars[i].value);
  const down = categoryGutter != null && size && !cfg.logScale && cfg.yTickCount == null
    ? verticalValueAxisFit({
        plotHeight: plotHeightLessLabels({ size, band: categoryBand, grid }),
        lo: cfg.yMin ?? Math.min(0, ...base),
        hi: cfg.yMax ?? Math.max(0, ...tops),
      })
    : undefined;
  // NO ROOM IS RESERVED BELOW THE PLOT FOR A TILTED LABEL, at any rotation, measured or explicit —
  // the 44/18 ladder this builder used to add on the explicit path is gone with it. See
  // `XLabelFit`'s doc in `../axisLabelFit.ts`: `containLabel` already expands the grid around the
  // rotated labels, and every pixel added on top came off the plot one-for-one.

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
      // `drawLabel`, not the raw `categories` — the same closure the axis formats its steps with, so
      // a screen reader hears the step it can see. This enumerated the raw bucket, which on a
      // month-bucketed waterfall read "2026-01-01 100" against an axis saying "January 2026".
      // The tooltip below stays on the raw bucket and says why.
      points: categories.map((c, i) => ({ name: drawLabel(c), value: deltas[i] })),
    }),
    // An axis title needs room reserved on the edge it renders on, or ECharts draws it into the
    // tick labels and the plot clips it — the same adjustment the cartesian builder makes, and the
    // reason a shared `CHART_THEME.grid` alone is not enough once titles exist.
    grid,
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
        // THE RAW BUCKET, not `drawLabel`'s period name — so a month-bucketed waterfall's axis
        // reads "January 2026" while this tooltip still reads "2026-01-01".
        //
        // Left as it is DELIBERATELY. This used to be one of TWO surfaces behind the axis, the other
        // being the `ariaDescription` above, and the note here treated the pair as one chart-wide
        // decision. The pair has since been SPLIT, because the two are not alike: the aria
        // description now goes through `drawLabel`, on this chart and on the cartesian family, and
        // only the tooltip stays raw.
        //
        // What separates them is the drill. A tooltip and a drill sit next to each other, and what a
        // step FILTERS by is the raw bucket — naming the period in the tooltip while the click
        // filters on the value trades a cosmetic mismatch for a semantic one. A screen-reader
        // description has no click behind it and exists precisely to say what a sighted user sees
        // (`ariaDescription`'s own header), so leaving it raw was a defect with nothing on the other
        // side of the scale.
        //
        // MEASURED, so the cartesian half is not assumed: its tooltip header is
        // `params.axisValueLabel`, which ECharts builds in `getValueLabel`
        // (`component/axisPointer/viewHelper.js`) from `axis.scale.getLabel` — `OrdinalScale`'s
        // returns the `xAxis.data` entry verbatim (`scale/Ordinal.js`) and never consults
        // `axisLabel.formatter`, and the only override is `axisPointer.label.formatter`, which no
        // builder here sets. Rendered against echarts 6 in jsdom, a month-bucketed bar chart hands
        // its formatter `axisValueLabel: "2026-01-01"` while the axis draws "January 2026". So the
        // two families' tooltips still agree with each other on a dashboard, which is the property
        // this note was protecting.
        const head = `<div style="font-weight:${TOOLTIP_WEIGHT.header}">${categories[i]}</div>`;
        const line = `<div>${dot}<span style="font-weight:${TOOLTIP_WEIGHT.value}">${fmt(delta)}</span></div>`;
        if (isTotal) return head + line;
        return `${head}${line}<div style="font-weight:${TOOLTIP_WEIGHT.name}">Running total: ${fmt(runningTotals[i])}</div>`;
      },
    },
    xAxis: {
      type: "category", data: categories,
      name: xTitle, nameLocation: "middle",
      // `nameGap` is measured from the AXIS LINE, and what sits between the line and the name is the
      // column of category labels — which, once the ladder above turns them 90°, run down into
      // exactly the band 28 places the title in. `bottomNameGapClearOf` states the three terms and
      // owns the one card it declines on; `Math.max` keeps 28 as the floor, which is the gap an axis
      // whose labels are hidden should keep.
      nameGap: Math.max(28, (categoryGutter != null && size
        ? bottomNameGapClearOf({ band: categoryBand, size, gridTop: grid.top as number })
        : undefined) ?? 28),
      nameTextStyle: { color: axisLabelColor },
      axisLabel: {
        fontSize: CHART_THEME.axisFontSize, color: axisLabelColor,
        // `fit.every - 1`, NOT `fit.every`: ECharts' category `interval` counts the labels SKIPPED
        // between two drawn ones, not the stride. `ordinalScaleCreateTicks`
        // (`echarts/lib/scale/helper.js:235`) reads it as `step = max((categoryInterval || 0) + 1, 1)`
        // and walks `tickValue += step` from the extent's start — so `0` is every label, which is what
        // the `decided` arm has always meant here, and every 6th is `5`. Off by one is invisible in a
        // screenshot: it draws Jan/Jun/Nov instead of Jan/Jul, which still looks like a date axis.
        //
        // AND THIS IS NOT PR 02's RULE BEING UNDONE, which is the reading to head off: "auto" drops
        // whichever labels happen to collide, an arbitrary subset, while a stride over buckets draws
        // period boundaries. The full argument, including why restoring a flat `interval: 0` here
        // would NOT restore PR 02, is on the `thin` arm of `XLabelFit` in `../axisLabelFit.ts`,
        // which is its ONE copy. The two further ECharts guarantees the stride rests on — that it
        // starts at index 0, and that the extra end tick is appended but not drawn beside it — are
        // in `options/cartesian.ts` at this same line, and only there: `axisLabelFit.ts` says
        // outright that the ECharts semantics belong at the builder line rather than in it.
        interval: fit?.kind === "thin" ? fit.every - 1 : decided ? (0 as const) : ("auto" as const),
        // The first bucket is the one label a stride promises, so on a thinned axis it is declared
        // required rather than left to `AxisBuilder.fixMinMaxLabelShow`, which drops it in favour of
        // the off-interval end tick ECharts appends. The mechanism, read out of echarts 6.1, is at
        // this same line in `options/cartesian.ts` and only there. A waterfall reaches it by the
        // same route with "Total" as that end tick.
        showMinLabel: fit?.kind === "thin" ? true : undefined,
        rotate,
        // The last answer the ladder can give: when even a rotated label would eat 70% of the
        // card's height, Metabase draws no axis rather than a wall of text.
        show: fit?.kind !== "hide",
        // `drawLabel`, defined above so the FITTER measures this same string. Do not inline it back:
        // the two drifting apart is what made the cartesian fit rotate labels that would have fitted
        // upright, and this axis had the identical seam.
        formatter: drawLabel,
      },
      axisLine: { show: cfg.xAxisTicks !== false, lineStyle: { color: gridLine } },
      axisTick: { show: cfg.xAxisTicks !== false },
    },
    yAxis: {
      type: cfg.logScale ? "log" : "value",
      name: yTitle, nameLocation: "middle", nameGap: 44,
      nameTextStyle: { color: axisLabelColor },
      // The ends the fit's interval divides, written FIRST so a range the user set outranks them —
      // see `verticalValueAxisFit` for why ECharts will not round them itself once an `interval` is
      // named, and for the short last step it leaves behind.
      ...(down != null ? { min: down.min, max: down.max } : {}),
      ...(cfg.yMin != null ? { min: cfg.yMin } : {}),
      ...(cfg.yMax != null ? { max: cfg.yMax } : {}),
      // The user's own tick count wins outright; the measured fit is the fallback behind it, and it
      // arrives ALONE — an `interval` beside a `splitNumber` would win over it in ECharts, which is
      // why the fit above declines outright once one is set rather than being masked here.
      ...(cfg.yTickCount != null ? { splitNumber: cfg.yTickCount }
        : down != null ? { interval: down.interval } : {}),
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
