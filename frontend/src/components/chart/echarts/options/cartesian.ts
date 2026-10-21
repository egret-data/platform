import type { EChartsCoreOption } from "echarts/core";
import type { CartesianViz, LineSeriesStyle } from "@/api/charts";
import type { EChartsSize } from "../EChartsRenderer";
import type { ColumnFormat } from "../../format-types";
import { isNumericType, isTemporalType } from "../../format-types";
import { formatValue, specifiesDateRendering } from "../../format";
import { ariaDescription, baseTextStyle, tooltipSurface } from "../base";
import { CHART_THEME, COLOR_FALLBACK, lineType, mixColor, resolveColor, symbolShape } from "../palette";
import { colorsForKeys } from "../keyColors";
import { isExtractUnit, temporalLabel } from "@/components/chart/temporal-units";
import { boundarySteps, namesAPeriod, periodName } from "@/components/chart/temporal-period";
import { foldTail, OTHER_LABEL } from "../fold";
import {
  AXIS_LABEL_MARGIN, bottomNameGapClearOf, categoryLabelBand, estimatePlotWidth, fitXLabels,
  intervalThatFits, nameGapClearOf, PANEL_TITLE_FONT, plotHeightLessLabels, plotWidthLessGutter,
  splitPanelLayout, verticalValueAxisFit, widestLabelWidth,
  type XLabelFit,
} from "../axisLabelFit";

export interface ResultColumn { name: string; type: string }
export interface CartesianBuildConfig {
  kind: CartesianViz;
  xAxis?: string;
  xMetric?: string;        // scatter only: a metric on the x-axis (metric-vs-metric)
  yMetrics: string[];
  series?: string;
  stacked?: boolean;
  horizontal?: boolean;
  showLabels?: boolean;
  showLegend?: boolean;
  xLabel?: string;
  yLabel?: string;
  /** Series name -> how it is drawn. Offered on line, area, bar and combo — the four kinds
   *  Metabase's `series.display` names; see `perSeriesDisplayApplies`. */
  seriesTypes?: Record<string, SeriesDisplay>;
  /** The SERIES drawn against a second (right) value axis: the chart's metrics on a plain chart,
   *  its breakout BANDS on one with a `series` column. The name is older than the meaning — see
   *  `rightAxisSeries`, which is the only thing that reads it. */
  rightAxisMetrics?: string[];
  /** Metabase's "Split y-axis when necessary". Unset means on, as it is there. Only consulted when
   *  `rightAxisMetrics` is empty — an explicit assignment always wins over a computed one. */
  /** Scatter only: the metric whose value sizes each bubble. */
  bubbleMetric?: string;
  autoSplitY?: boolean;
  /**
   * Metabase's `graph.split_panels` ("Stack series"): draw each series in a panel of its own,
   * stacked down the card, sharing one category axis and each with its own value scale.
   *
   * `splitPanelsApplies` decides where it is OFFERED and `splitPanelLayout` whether the card can
   * carry it; both are asked by this builder and by the settings panel, so neither can offer what
   * the other will not draw.
   */
  splitPanels?: boolean;
  /** Metabase's "Show label", one per axis, separate from the label TEXT. Unset means shown. Kept
   *  apart from the text so hiding a title does not make the user retype it to bring it back. */
  showXLabel?: boolean;
  showYLabel?: boolean;
  logScale?: boolean;            // value axis uses a log scale
  normalized?: boolean;          // 100% stacking — each category's values sum to 100
  goalValue?: number;            // horizontal reference (goal) line
  goalLabel?: string;
  trendLine?: boolean;           // overlay a least-squares trend line per series
  showStackTotal?: boolean;      // label the total atop each stacked column
  seriesColors?: Record<string, string>;  // series name -> colour override (Metabase series settings)
  seriesLabels?: Record<string, string>;  // series name -> display label override
  yMin?: number;                 // manual value-axis minimum (auto when undefined)
  yMax?: number;                 // manual value-axis maximum
  xScale?: "ordinal" | "linear" | "timeseries";  // x-axis scale (default ordinal/category)
  /**
   * The rows already arrive in an order that MEANS something, so leave a nominal axis alone.
   *
   * TWO CALLERS SET IT, for the same reason by different routes:
   *
   *  - `FunnelChart`'s bar mode, whose steps run Visited → Added to cart → Checkout → Purchased
   *    from `funnel.stepOrder`. Alphabetising a pipeline is worse than any engine order, and the
   *    component has already ranked the rows itself before it builds this option.
   *  - `ChartRenderer`, for any chart whose stored spec carries a NON-EMPTY `orderby`. The engine
   *    was told what order to return the rows in, and it did; re-sorting them here would leave the
   *    author's sort visibly inert while every unit test still passed.
   *
   * A spec carrying `orderby: []` must NOT set it — arrival order is then whatever the engine
   * happened to return, which is exactly the defect PR #394 fixed. See `sortCategories`.
   */
  preserveCategoryOrder?: boolean;
  /**
   * The same bargain as `preserveCategoryOrder`, for the BANDS a breakout draws — and a narrower
   * question than the one above, deliberately, because the two are not the same question.
   *
   * `preserveCategoryOrder` is "the stored spec carries ANY sort". That is sound for the x axis: the
   * categories come out of one column, so any `ORDER BY` at all puts the rows in an order the axis
   * can be read off. It is NOT sound for the bands, and the counter-example is ordinary. A spec
   * sorted `ORDER BY category ASC, band ASC` returns its rows grouped by category; the bands' order
   * of FIRST APPEARANCE is then whichever bands the first category happens to contain, then the new
   * ones the second introduces, and so on. Bands `C` at category 1 and `A, B` at category 2 arrive
   * `C, A, B` from a query that sorted ascending by band. Trusting arrival order there would pin a
   * wrong order rather than an arbitrary one, which is worse: it looks intentional.
   *
   * So this flag says something stronger and provable: THE BREAKOUT COLUMN IS THE PRIMARY SORT KEY.
   * When it is, every row of one band precedes every row of the next, so first-seen order IS the
   * engine's order over that column exactly — including a DESC the author asked for, which is the
   * case worth protecting. `ChartRenderer` derives it that way (`orderby[0].field === series`) and
   * nowhere else, so a sort on some other column leaves this false and the renderer sorts.
   *
   * KNOWN AND ACCEPTED ASYMMETRY: the settings panel's own band list (`cartesianSeriesNames`) has no
   * access to the spec's `orderby` — `VizControlProps` carries the last result, not the query — so
   * under this flag the panel lists the bands sorted while the chart draws them in the author's
   * order. Threading the sort through the shell to close a cosmetic gap in the one case where the
   * author sorted BY the breakout column, and where the first click of a reorder arrow stores an
   * explicit `seriesOrder` that wins on both sides anyway, is not worth a shell change.
   */
  preserveSeriesOrder?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
  /**
   * The temporal unit the x-axis dimension was grouped by, from `params.spec.temporal_units`.
   * EITHER KIND, and the two are read at different places — this said "extraction unit (e.g.
   * `day_of_week`)" while that was all it drove, and has been stale since a truncation unit started
   * driving the period naming beneath it:
   *   - an EXTRACTION unit (`day_of_week`, `month_of_year`) is substituted into the categories
   *     themselves through `temporalLabel` (`displayCats`), so the axis never sees the raw integer;
   *   - a TRUNCATION unit (`month`, `quarter`, …) leaves the categories alone and reaches
   *     `drawLabel`, which names the period the bucket covers, and `boundary`, which is the only
   *     door to thinning.
   * `WaterfallBuildConfig.xAxisUnit` is the same field fed from the same line of `ChartRenderer`,
   * with one difference stated there: that builder does not substitute extraction units.
   */
  xAxisUnit?: string;
  xAxisType?: string;            // SQL type of the x-axis dimension, so its labels can be formatted
  /** Metabase's "Show lines and tick marks", per axis. Unset means shown, which is what every
   *  chart drew before the toggles existed. */
  xAxisTicks?: boolean;
  yAxisTicks?: boolean;
  /** An explicit x-label rotation, which SUPPRESSES the measured choice — Metabase's
   *  auto-selection likewise applies only while the setting is untouched. Same field name and
   *  values waterfall and box plot already store (`options/waterfall.ts`, `options/boxplot.ts`).
   *  The panel control that writes it is Task 3; this is the field the builder reads. */
  xLabelRotation?: 45 | 90;
  /** Metabase's fifth `graph.x_axis.axis_enabled` value, "Compact" — a FORMATTING flag on the
   *  category label text, read by `drawLabel` and nowhere else. Same field name the waterfall
   *  stores; the box plot has no twin, because its category axis has no formatter. */
  xLabelCompact?: boolean;
  /** Metabase's `graph.y_axis.unpin_from_zero`. Unset takes the DISPLAY's default, which is
   *  unpinned on a scatter and pinned everywhere else — `unpinFromZeroOn` resolves it, and
   *  `unpinFromZeroApplies` decides where it is honoured at all. */
  unpinFromZero?: boolean;
  /** Metabase's "Number of tick marks" on the value axis. Unset lets ECharts choose. */
  yTickCount?: number;
  seriesUnit?: string;           // temporal unit of the BREAKOUT dimension, same role as xAxisUnit
  seriesType?: string;           // SQL type of the breakout dimension, so its labels can be formatted
  /** Short value-axis labels (25.5K). Defaults to on once the numbers are wide. */
  compactValues?: boolean;
  /** Fold all but the largest `maxCategories - 1` into "Other". Unset keeps every category. */
  maxCategories?: number;
  /** Series names in display order; any not listed keep their natural order after them. */
  seriesOrder?: string[];
  /** Series names to leave out of the chart entirely. */
  hiddenSeries?: string[];
  /** Data-label density: "fit" drops labels that would collide, "all" draws every one. */
  labelDensity?: "fit" | "all";
  /** Line shape. One choice, because a line cannot be both smoothed and stepped. */
  lineShape?: "linear" | "smooth" | "step";
  /** What a null becomes: bridged, dropped to zero, or left as a break in the line. */
  missing?: "interpolate" | "zero" | "none";
  /** Point markers. "auto" shows them only when there are few enough points to read. */
  markers?: "auto" | "on" | "off";
  /** Per-series overrides of the chart-wide line appearance, keyed by series name. */
  seriesLine?: Record<string, LineSeriesStyle>;
  /** Dataset metrics fetched for the tooltip and plotted nowhere — see `CartesianConfig` for why
   *  this is metrics rather than Metabase's columns, and `extraTooltipMetrics` for what this
   *  builder does and does not accept from it. */
  tooltipMetrics?: string[];
}

/** One extra tooltip row: the metric it names and the value at the datum being described. */
interface TooltipExtra { metric: string; value: unknown }

/**
 * The tooltip metrics this chart will actually SHOW, from the ones it was asked to show.
 *
 * Four things are dropped here, and each one is a row that would otherwise be a lie or a duplicate:
 *
 *  - ANY of them, once a breakout names the series. With a `series` column the metric is aggregated
 *    per (x, band), so there is no single value for the category — and egret's cartesian tooltip is
 *    `trigger: "axis"`, showing every band at once, so an honest rendering needs one row per band
 *    with a second name to disambiguate it. Metabase reaches the per-band value because its
 *    cartesian tooltip is `trigger: "item"` throughout (`echarts/cartesian/option/tooltip.tsx:58`)
 *    and its extra rows are built from ONE hovered `seriesModel` and its datum
 *    (`CartesianChart/events.ts:369`). Ours is a different shape, so this is a scope line rather
 *    than a gap, and `cartesianDescriptor.buildSpec` declines to FETCH them in the same state —
 *    the two halves have to agree or the chart pays for a column it discards, which is the very
 *    defect this PR's other half fixes on the scatter.
 *  - A metric the chart already plots. Metabase's `alreadyIncludedColumns` rule
 *    (`shared/settings/cartesian-chart.ts:469`): an ADDITIONAL row is one that says something new,
 *    and a second row repeating the series' own number is worse than the feature's absence. On a
 *    scatter that set includes the x metric and the bubble metric, both of which the tooltip
 *    already prints.
 *  - A metric the RESULT does not carry. A stored key outlives what named it — a dataset drifts, or
 *    a hand-written dashboard-card override arrives carrying a setting whose spec was never
 *    re-run (`applyVizOverride`: "the query `spec` is never overridden"). Neither may put the word
 *    `undefined` in a tooltip.
 *  - A duplicate, so a name listed twice does not print twice.
 */
function extraTooltipMetrics(cfg: CartesianBuildConfig, idx: Record<string, number>): string[] {
  const asked = cfg.tooltipMetrics;
  if (asked == null || asked.length === 0) return [];
  if (cfg.series != null && cfg.series !== "") return [];
  const plotted = new Set<string>(cfg.yMetrics);
  if (cfg.xMetric != null) plotted.add(cfg.xMetric);
  if (cfg.bubbleMetric != null) plotted.add(cfg.bubbleMetric);
  const out: string[] = [];
  for (const m of asked) {
    if (plotted.has(m) || idx[m] == null || out.includes(m)) continue;
    out.push(m);
  }
  return out;
}

// The series names a cartesian chart will render — its y-metrics, or the distinct
// breakout values (null/empty folded to "(empty)", matching the renderer). Used by the
// settings panel to list series for per-series colour / rename without rebuilding the chart.
export function cartesianSeriesNames(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: { yMetrics: string[]; xAxis?: string; series?: string },
): string[] {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  if (cfg.series != null && idx[cfg.series] != null) {
    const gi = idx[cfg.series];
    const xi = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
    const valid = xi == null ? rows : rows.filter((r) => r[xi] != null);
    // THE SAME ORDER THE CHART DRAWS, through the same function — this list is what the Series
    // section renders, and its order is what the reorder arrows write back as `seriesOrder`. A panel
    // listing the bands in arrival order beside a chart drawing them sorted makes every arrow look
    // broken, and the first click would store an order the user never composed.
    //
    // SORTED BEFORE `(empty)` IS SUBSTITUTED, on the raw value, for the reason `sortBands` states at
    // length: the label is not the key. (`(empty)` would sort among the E's; the raw `null` it
    // stands for does not.)
    return sortBands(distinct(valid.map((r) => r[gi])), columns[gi].type)
      .map((sv) => (sv == null || sv === "" ? "(empty)" : String(sv)));
  }
  return [...cfg.yMetrics];
}

/**
 * The ECharts component id of the axis the METRICS are plotted against — the left `yAxis` on a
 * column chart, the `xAxis` on a row one. It exists so `axisNamesFor`'s merge patch can address
 * that axis by id rather than by array position; see `mkValueAxis`, which records what a
 * position-matched patch actually did.
 */
const VALUE_AXIS_ID = "yValue";

const BAR_MIN_HEIGHT = 1; // keep sub-pixel bars visible, like Metabase's barMinHeight
/** Above this many points on one line, markers merge into the line rather than marking it. */
const MARKER_AUTO_MAX = 30;

/** Metabase's S/M/L line sizes. M is the theme's own width, so an unset size renders unchanged.
 * S is 1.5, not ECharts' own 1px default — gridlines/axis lines already render at 1px, and a
 * data line that weighs the same as the chrome around it undercuts the point of drawing it. */
const LINE_WIDTHS = { S: 1.5, M: CHART_THEME.lineWidth, L: 4 } as const;

interface SeriesLike { name?: string; data: (number | null)[]; yAxisIndex?: number; itemStyle?: { color?: string }; __trend?: boolean; __total?: boolean;
  /** The series' RAW column value, behind the display label `name` carries. Set by `decorateSeries`
   *  on every series it builds; absent on the overlays built after it (trend, stack total), which
   *  are never matched against a stored setting. See `settingsKey`. */
  __raw?: string; __metric?: string; __markers?: "auto" | "on" | "off";
  /** The NAME of the series this overlay was fitted to, on a trend line. Set by `trendSeries`, and
   *  read only by the split-panel layout, which has to put a trend line in its source's panel.
   *
   *  RECORDED RATHER THAN RE-DERIVED. The name is `${source} trend`, so the source could be
   *  recovered by stripping that suffix — and the tooltip formatter already does exactly that
   *  (`p.seriesName.endsWith(" trend")`). It is a bad basis for an INDEX lookup: a series genuinely
   *  named "orders trend" would claim the panel of one named "orders", and a rename through
   *  `seriesLabels` moves `name` while the suffix rule keeps pointing at the old string. Metabase
   *  carries the same link explicitly, as `sourceDataKey` (`remapTrendLinesToPanels`). */
  __of?: string }

/**
 * Markers stop reading as points and start reading as a lumpy line once they touch. Applied after
 * folding and x-scaling, because "auto" is a question about the points that end up drawn — and it
 * counts only the ones that exist: a null draws nothing, so a sparse series is not crowded.
 */
function applyMarkers(series: SeriesLike[]): void {
  for (const s of series) {
    if (s.__markers == null) continue;
    const drawn = (s.data ?? []).filter((v) => v != null).length;
    (s as unknown as Record<string, unknown>).showSymbol =
      s.__markers === "on" || (s.__markers === "auto" && drawn <= MARKER_AUTO_MAX);
  }
}

// A transparent overlay bar whose top label shows each column's stack total —
// clean-room of Metabase's getStackTotalsSeries.
function stackTotalSeries(series: SeriesLike[], cfg: CartesianBuildConfig): SeriesLike {
  const values = series.filter((s) => !s.__trend && !s.__total);
  const n = values[0]?.data.length ?? 0;
  const totals: (number | null)[] = [];
  for (let i = 0; i < n; i++) {
    let t = 0; let any = false;
    for (const s of values) { const v = s.data[i]; if (typeof v === "number") { t += v; any = true; } }
    totals.push(any ? t : null);
  }
  const m = cfg.yMetrics[0];
  const fmt = (v: number) => (m ? formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]) : String(v));
  return {
    data: totals,
    __total: true,
    ...{ type: "bar", barGap: "-100%", barWidth: CHART_THEME.barWidth, silent: true, legendHoverLink: false, tooltip: { show: false },
      itemStyle: { color: "transparent" }, emphasis: { itemStyle: { color: "transparent" } },
      label: { show: true, position: "top", fontSize: CHART_THEME.labelFontSize, fontWeight: CHART_THEME.labelWeight,
        formatter: (p: { value: unknown }) => (p.value == null ? "" : fmt(Number(p.value))) } },
  } as SeriesLike;
}

// Least-squares fit y = m·i + b over the non-null points, evaluated at every index.
function trendData(data: (number | null)[]): (number | null)[] | null {
  const pts: [number, number][] = [];
  data.forEach((v, i) => { if (typeof v === "number") pts.push([i, v]); });
  if (pts.length < 2) return null;
  const n = pts.length;
  const sx = pts.reduce((s, p) => s + p[0], 0);
  const sy = pts.reduce((s, p) => s + p[1], 0);
  const sxy = pts.reduce((s, p) => s + p[0] * p[1], 0);
  const sxx = pts.reduce((s, p) => s + p[0] * p[0], 0);
  const denom = n * sxx - sx * sx;
  if (denom === 0) return null;
  const m = (n * sxy - sx * sy) / denom;
  const b = (sy - m * sx) / n;
  return data.map((_, i) => m * i + b);
}

// One dashed trend series per source series, sharing its axis + color, kept out of the legend.
function trendSeries(series: SeriesLike[]): SeriesLike[] {
  const out: SeriesLike[] = [];
  for (const s of series) {
    const fit = trendData(s.data);
    if (!fit) continue;
    out.push({
      name: `${s.name ?? ""} trend`,
      data: fit,
      yAxisIndex: s.yAxisIndex ?? 0,
      __trend: true,
      // See `__of`. The split-panel layout re-binds both axis indices from this; every other mode
      // ignores it.
      __of: s.name,
      ...{ type: "line", symbol: "none", smooth: false, silent: true, legendHoverLink: false, z: 3,
        lineStyle: { type: "dashed", width: 1.5, color: s.itemStyle?.color },
        tooltip: { show: false } },
    } as SeriesLike);
  }
  return out;
}

/**
 * Metabase's `graph.series_order`: filters out hidden series and reorders the rest.
 *
 * Called before the overlays are built, so anything it drops is never counted in a stack
 * total, never normalised against, and leaves no orphaned trend line — but that guarantee
 * only holds as far downstream as every consumer keys off the series that reach it (see the
 * axis-shape logic in baseOption, which had to be re-derived from the surviving series
 * rather than from cfg for the same reason). Order matters beyond the legend: it is also the
 * stacking order and the draw order.
 */
function applySeriesOrder<T extends { __raw?: string; name?: string }>(
  // GENERIC over the series shape rather than fixed to `SeriesLike`, so `scatterXY` — whose data is
  // an array of `{value: [x, y]}` points and not the `(number | null)[]` a bar's is — can call THIS
  // function instead of growing a second copy of the hide-and-reorder rule. Nothing in here touches
  // `data`; it reads `settingsKey` and nothing else, which is exactly the shape both kinds share.
  series: T[],
  cfg: CartesianBuildConfig,
): T[] {
  const hidden = cfg.hiddenSeries;
  const order = cfg.seriesOrder;
  if (!hidden?.length && !order?.length) return series;
  const out = hidden?.length ? series.filter((s) => !hidden.includes(settingsKey(s))) : [...series];
  if (order?.length) {
    const rank = new Map(order.map((n, i) => [n, i]));
    // A name that is no longer plotted simply ranks nowhere; anything the stored order does
    // not mention keeps its natural position after the ones it does.
    out.sort(
      (a, b) =>
        (rank.get(settingsKey(a)) ?? Number.MAX_SAFE_INTEGER) -
        (rank.get(settingsKey(b)) ?? Number.MAX_SAFE_INTEGER),
    );
  }
  return out;
}

/**
 * The name `hiddenSeries` / `seriesOrder` are WRITTEN AGAINST — the raw column value, not the label
 * on screen.
 *
 * WHY THIS EXISTS. `applySeriesOrder` runs after `decorateSeries`, which has already replaced
 * `name` with the DISPLAY label: the user's rename (`seriesLabels`), the period a bucket covers
 * ("January 2026" for a raw `2026-01-01`), or the series column's own number/date format. The
 * settings panel keys on the raw value at every step — `cartesianSeriesNames` reads it off the
 * result, `SeriesListSection` stores back the name it was handed — so matching on `name` meant the
 * Show switch and the reorder arrows went silently inert the moment a chart was renamed, bucketed
 * or formatted. The switch moved, the config stored the value, and the band stayed on screen.
 *
 * `decorateSeries` had already met this bug once, from the other side, and its comment says so:
 * "Filtering by the display label found nothing the moment a series was renamed — the rename
 * silently broke the click." `__raw` was added for the DRILL then. This function was never updated
 * to read it, so the same rename went on breaking the hide for as long as `__raw` had existed.
 *
 * `??` AND NOT `||`, deliberately, because the two differ on a `__raw` of `""` and the panel's
 * vocabulary decides which is right. `__raw` is THE PANEL'S KEY on every path that reaches here — a
 * metric is listed under its own name and a null or empty breakout value is listed as `(empty)` by
 * both sides (`groupName`, `cartesianSeriesNames`) — so an empty `__raw` could only be a metric
 * literally named `""`, which the panel would also list as `""`. `??` matches that and `||` would
 * fall through to the label and miss.
 *
 * `scatterXY` REACHES HERE NOW, and this paragraph used to say it could not: "`scatterXY` returns
 * from `buildCartesianOption` before this function is ever called (it also never calls it itself)".
 * It calls `applySeriesOrder` since the Show switch and the reorder arrows on its own series rows
 * turned out to be stored and thrown away. Its series therefore carry `__raw = groupKey(g)` — the
 * y-metric for the one group of an un-broken-out scatter, whose raw value is `""` and which the
 * panel has never named that. That is the same invariant the previous paragraph states for every
 * other path, not an exception to it, and the drill is unaffected: `ChartRenderer` reads
 * `rawSeriesAt` only inside `if (cartesian.series)`, and with a breakout `groupKey` is the identity.
 *
 * The `?? ""` tail is for `SeriesLike.name` being optional in the type; nothing built here omits it.
 */
const settingsKey = (s: { __raw?: string; name?: string }): string => s.__raw ?? s.name ?? "";

/**
 * The DISPLAY name of a column, by the precedence every titling surface in this family uses: the
 * user's per-series rename first, then the column's own `column_title`, then the raw name.
 *
 * Extracted from `baseOption`'s local `titleOf` so `scatterXY` can ask the same question rather than
 * spell the chain out again — that chart used to name its two axes `cfg.xMetric` and `yMetric` flat,
 * which made it the one cartesian flavour where renaming a column left its axis on the raw name.
 * Two copies of a precedence rule is how the two come to disagree about which of a rename and a
 * `column_title` wins.
 */
const displayTitle = (cfg: CartesianBuildConfig, col: string | undefined): string | undefined =>
  col == null ? undefined : (cfg.seriesLabels?.[col] ?? cfg.columnSettings?.[col]?.column_title ?? col);

/**
 * Whether this chart stacks, resolving the default its KIND carries.
 *
 * An area chart stacks unless told otherwise — Metabase's own default, and for a reason its
 * measurement shows: overlapping translucent areas at 0.3 opacity read as mud, so the shape only
 * works stacked. Bar and line default to unstacked, as they do there.
 *
 * `undefined` is genuinely distinct from `false` here: `cartesianToSettings` drops undefined keys,
 * so a chart whose Stacking control was never touched stores nothing and takes the kind's default,
 * while one explicitly set to "Don't stack" stores `false` and keeps it.
 */
export function resolveStacked(cfg: Pick<CartesianBuildConfig, "kind" | "stacked">): boolean {
  return cfg.stacked ?? cfg.kind === "area";
}

/** The three mutually exclusive stacking states Metabase offers, as one value. */
export type StackMode = "none" | "stacked" | "normalized";

export function stackModeOf(cfg: Pick<CartesianBuildConfig, "kind" | "stacked" | "normalized">): StackMode {
  if (!resolveStacked(cfg)) return "none";
  return cfg.normalized ? "normalized" : "stacked";
}

/** The config patch a chosen mode writes. Two booleans encode three states, so the pair has to be
 *  set together — leaving `normalized` behind when switching to "Don't stack" would store a
 *  combination no control can show. */
export function stackModePatch(mode: StackMode): { stacked: boolean; normalized: boolean | undefined } {
  return {
    stacked: mode !== "none",
    normalized: mode === "normalized" ? true : undefined,
  };
}

const isNormalized = (cfg: CartesianBuildConfig) => !!cfg.normalized && resolveStacked(cfg);

// 100% stacking — rewrite each category column so its series values sum to 100.
function normalizeStacks(series: { data: (number | null)[] }[]) {
  const n = series[0]?.data.length ?? 0;
  for (let i = 0; i < n; i++) {
    let total = 0;
    for (const s of series) { const v = s.data[i]; if (typeof v === "number") total += v; }
    for (const s of series) {
      const v = s.data[i];
      s.data[i] = typeof v === "number" ? (total > 0 ? (v / total) * 100 : 0) : null;
    }
  }
}

/** A metric's [min, max] over the drawn rows. */
type Extent = [number, number];

/**
 * WHICH OF THIS CHART'S SERIES ARE DRAWN AGAINST THE RIGHT-HAND VALUE AXIS — Metabase's per-series
 * `axis` (`lib/settings/series.ts`), decided for the WHOLE SET because "everything on one side" is
 * not a split and a lone axis with nothing on it is a rendering fault rather than a preference.
 *
 * `names` IS WHAT THE CHART ACTUALLY DRAWS: its metrics on a plain chart, its breakout BANDS on one
 * with a `series` column. That parameter is the entire fix. `decorateSeries` used to be handed
 * `cfg.yMetrics[0]` as the metric for every band of a breakout and ask `rightMetrics(cfg)` about
 * that one name — one metric, one answer, all bands — so the per-metric control was inert under a
 * breakout and #423 withdrew it. Metabase has no such limit because its `axis` hangs off a series
 * key that is a metric on one chart and a formatted breakout value on the other; asking about the
 * drawn names is the same idea with the same single stored list.
 *
 * NO SECOND STORED KEY, DELIBERATELY. `rightAxisMetrics` was always a list of names filtered against
 * what the chart draws; only the list it is filtered against widened. A `seriesAxis` map beside it
 * would be two places holding one answer plus a precedence rule between them, which is the shape
 * this programme keeps removing rather than adding.
 *
 * THE HAZARD IT TRADES FOR, stated because it is real: a stored value can only hold METRIC names —
 * the panel wrote `config.yMetrics.map(...)` and since #423 wrote nothing at all under a breakout —
 * so every existing breakout chart starts inert here, measured. The exception is a band whose value
 * is spelled exactly like one of the chart's metrics, which would now move to the right axis on
 * load. That is the price of one key instead of two, and it is smaller than a migration.
 *
 * A HORIZONTAL CHART IS OUT at the top rather than at each call site: its value axis is the
 * horizontal one and there is no second edge to put another on, which is where `isDualAxis` used to
 * say it.
 */
export function rightAxisSeries(
  cfg: Pick<CartesianBuildConfig, "kind" | "horizontal" | "stacked" | "rightAxisMetrics">, names: string[],
): string[] {
  if (isHorizontal(cfg as CartesianBuildConfig)) return [];
  // A STACK CANNOT STAND ON TWO AXES, and this is the same refusal `shouldAutoSplitY` already makes
  // one screen up, in its own words: "A stack answers a different question — the parts of one total
  // — so a second scale would make the stack meaningless." Refusing to CREATE such a split while
  // honouring one somebody typed would be the same rule applied to half the ways of reaching it.
  //
  // METABASE COLLAPSES RATHER THAN REFUSING (`echarts/cartesian/model/stack.ts`): a stack goes to
  // the right only when EVERY series in it was sent right, and otherwise the whole stack goes left,
  // leaving a non-stacked series beside it free to take the other axis. egret is narrower on
  // purpose. `resolveStacked` here is chart-wide, so egret has no per-display stack groups to take
  // an "every member" vote within; reproducing the vote would mean modelling those groups for the
  // one case a combo reaches — a stacked bar pair beside a free line. That is a bigger change than
  // the control it would serve, and refusing outright is honest where a partial collapse would draw
  // a split for some assignments and silently ignore others.
  //
  // THE PANEL WITHDRAWS THE ROWS IN THE SAME STATE, so nothing is offered that this refuses.
  if (resolveStacked(cfg)) return [];
  const right = (cfg.rightAxisMetrics ?? []).filter((n) => names.includes(n));
  // ≥1 on each side, or it is not a split. Metabase reaches the same place from the other
  // direction: `getYAxisSplit` collects a `left`, a `right` and an `auto` bucket and only ever
  // builds the second axis when both named sides are non-empty.
  return right.length > 0 && right.length < names.length ? right : [];
}

/** Per-metric [min, max] over the drawn rows. The split decision needs one range per metric — the
 *  granularity egret's own right-axis control uses, so a computed split is a value that control
 *  could equally have produced by hand. */
function metricExtents(columns: ResultColumn[], rows: unknown[][], metrics: string[]): Record<string, Extent> {
  const out: Record<string, Extent> = {};
  for (const m of metrics) {
    const i = columns.findIndex((c) => c.name === m);
    if (i < 0) continue;
    let lo = Infinity;
    let hi = -Infinity;
    for (const r of rows) {
      const v = Number(r[i]);
      if (!Number.isFinite(v)) continue;
      if (v < lo) lo = v;
      if (v > hi) hi = v;
    }
    if (Number.isFinite(lo) && Number.isFinite(hi)) out[m] = [lo, hi];
  }
  return out;
}

/** Metabase's `KEYS_TO_COMPARE` verbatim — the format fields that make two metrics incomparable on
 *  one scale. Every one of them exists on egret's ColumnFormat under the same name. */
const KEYS_TO_COMPARE = [
  "number_style", "currency", "currency_style", "number_separators", "decimals", "scale",
  "prefix", "suffix",
] as const;

function comparisonSettings(fmt: ColumnFormat | undefined): string {
  if (!fmt) return "";
  const picked: Record<string, unknown> = {};
  for (const k of KEYS_TO_COMPARE) {
    const v = fmt[k];
    if (v === undefined) continue;
    // Metabase drops an EMPTY prefix/suffix before comparing, so "" and unset are the same thing.
    if ((k === "prefix" || k === "suffix") && v === "") continue;
    picked[k] = v;
  }
  return JSON.stringify(picked);
}

/**
 * Whether the y-axis should split itself — Metabase's `shouldAutoSplitYAxis`, ported.
 *
 * Its own note on the last rule, kept because it is the reason for the number: "When the series
 * with the smallest range is less than 5% of the chart's total range, we split the y-axis so it
 * doesn't look too small."
 *
 * The `semantic_type` and multi-card clauses have no egret equivalent — egret plots one dataset and
 * carries no semantic types — so they are dropped rather than approximated. The format clause does
 * translate exactly: two metrics formatted as different currencies, or one scaled and one not, do
 * not belong on one scale however close their ranges are.
 */
export function shouldAutoSplitY(cfg: CartesianBuildConfig, extents: Record<string, Extent>): boolean {
  if (cfg.autoSplitY === false) return false;
  // A stack answers a different question — the parts of one total — so a second scale would make
  // the stack meaningless. Metabase bails on `stackable.stack_type` for the same reason.
  if (resolveStacked(cfg)) return false;
  if (isHorizontal(cfg)) return false;
  const metrics = cfg.yMetrics.filter((m) => extents[m] != null);
  if (metrics.length < 2) return false;

  const fmts = metrics.map((m) => comparisonSettings(cfg.columnSettings?.[m]));
  if (new Set(fmts).size > 1) return true;

  const ranges = metrics.map((m) => extents[m][1] - extents[m][0]);
  const minRange = Math.min(...ranges);
  const chartRange = Math.max(...metrics.map((m) => extents[m][1])) - Math.min(...metrics.map((m) => extents[m][0]));
  if (chartRange === 0) return false;
  return minRange / chartRange <= 0.05;
}

// Metabase's own constants, verbatim.
const SPLIT_AXIS_UNSPLIT_COST = -100;
const SPLIT_AXIS_COST_FACTOR = 2;
const SPLIT_AXIS_MAX_DEPTH = 8;

/** Every partition of `list` into two sides. Past the depth cap the remainder goes to the smaller
 *  side in one piece, so the search stays bounded without dropping a metric. */
function generateSplits(list: string[], left: string[] = [], right: string[] = [], depth = 0): [string[], string[]][] {
  if (list.length === 0) return [[left, right]];
  if (depth > SPLIT_AXIS_MAX_DEPTH) {
    return left.length < right.length ? [[left.concat(list), right]] : [[left, right.concat(list)]];
  }
  return [
    ...generateSplits(list.slice(1), left.concat([list[0]]), right, depth + 1),
    ...generateSplits(list.slice(1), left, right.concat([list[0]]), depth + 1),
  ];
}

/** How badly one axis serves the metrics on it: the square of how much of the axis each metric
 *  fails to use. An empty side scores -100 while unsplit is still favoured, which is what keeps a
 *  chart on one axis unless splitting genuinely pays. */
function axisCost(extents: Extent[], favorUnsplit = true): number {
  if (favorUnsplit && extents.length === 0) return SPLIT_AXIS_UNSPLIT_COST;
  const lo = Math.min(...extents.map((e) => e[0]));
  const hi = Math.max(...extents.map((e) => e[1]));
  const axisRange = hi - lo;
  if (axisRange === 0) return 0;
  return extents.reduce((sum, e) => {
    const r = e[1] - e[0];
    return sum + (r === 0 ? 0 : Math.pow(axisRange / r, SPLIT_AXIS_COST_FACTOR));
  }, 0);
}

/** The cheapest partition. Metabase's `computeSplit`, ported. Returns the RIGHT side, which is what
 *  egret's `rightAxisMetrics` names. */
export function computeAxisSplit(metrics: string[], extents: Record<string, Extent>): string[] {
  const usable = metrics.filter((m) => extents[m] != null);
  let best: [string[], string[]] | null = null;
  let bestCost = Infinity;
  for (const split of generateSplits(usable)) {
    const c = axisCost(split[0].map((m) => extents[m])) + axisCost(split[1].map((m) => extents[m]));
    if (!best || c < bestCost) { best = split; bestCost = c; }
  }
  return best?.[1] ?? [];
}
// `isDualAxis(cfg)` used to live here, asking `rightMetrics(cfg).length` against
// `cfg.yMetrics.length`. It is `rightAxisSeries` now, which takes the DRAWN series names instead —
// the same two conditions over a list a breakout can also answer. Nothing reads a config-only
// answer any more: `baseOption` decides the axis shape from the series that survived hiding
// (`plotted.some(s => s.yAxisIndex === 1)`), which it already had to.

/**
 * A row chart is a horizontal bar chart — and `horizontal` is the BAR's flag and no other kind's.
 *
 * THE EXCLUSION USED TO READ `cfg.kind !== "scatter"`, which honoured the flag on line, area and
 * combo as well. Measured: `xAxis.type` went `category` → `value` and `yAxis.type` `value` →
 * `category` on all three. `OrientationOption` has only ever rendered the switch for `kind ===
 * "bar"`, and `cartesian.horizontal` lives in the SHARED cartesian namespace, so it survives a
 * chart-type switch — which made a bar chart turned horizontal and then switched to a line a
 * sideways line chart with no control anywhere in the product to stand it back up.
 *
 * METABASE HAS NO ORIENTATION SETTING. Its horizontal bar is a separate display type (`row`) with
 * its own d3 renderer that never touches the ECharts cartesian pipeline; `buildAxes`
 * (`echarts/cartesian/option/axis.ts`) is hardcoded dimension-x / metrics-y with no orientation
 * parameter, and a line, area or combo chart there cannot be horizontal at all. So the rule copied
 * is "orientation belongs to the bar family", and the half that moves is the BUILDER's — which is
 * also where the same clause already lived for the scatter, whose own two value axes have nothing
 * to swap.
 *
 * NOTHING IS MIGRATED AND NO STORED VALUE IS DESTROYED, deliberately: a chart that carries
 * `horizontal: true` on a line simply draws upright, and switching it back to a bar draws it
 * sideways again. That is how `xMetric` already behaves one namespace over — stored on any
 * cartesian chart, honoured only on a scatter (`buildCartesianOption`'s opening line).
 *
 * EXPORTED because `settings/CartesianDisplaySection.tsx` relabels the Axes tab's rows by the axis
 * the READER sees, and it used to answer that question with its own `kind === "row" ||
 * !!config.horizontal`. A second copy of this rule is exactly how a line chart with a stale flag
 * comes to print "Y label" over the control that titles its horizontal axis.
 */
export const isHorizontal = (cfg: Pick<CartesianBuildConfig, "kind" | "horizontal">) =>
  cfg.kind === "row" || (!!cfg.horizontal && cfg.kind === "bar");

/**
 * Whether SPLIT PANELS is a thing this chart can be — the question of KIND and SHAPE, asked before
 * the data is known. How many panels there would be, and whether the card can carry them, are two
 * further questions the builder asks later (`splitPanelLayout`); the panel asks the first of those
 * itself, from the series names, exactly as it does for Stacking.
 *
 * TWO REFUSALS, and Metabase makes the first of them too:
 *
 *   - A ROW CHART. `SPLIT_PANELS_SETTINGS` is spread into `CartesianChart` and `ScatterPlot` and
 *     not into `RowChart` (`visualizations/visualizations/*`). The reason is geometric rather than
 *     a gap: a row chart already runs its CATEGORIES down the card, so a stack of panels would be
 *     competing for the one axis it has, and its value axis — the horizontal one — is not the axis
 *     panels give each series its own copy of.
 *   - A METRIC-AGAINST-METRIC SCATTER, which is egret's own refusal and not Metabase's. That chart
 *     leaves `buildCartesianOption` through `scatterXY` on its opening line, and `scatterXY`
 *     composes its whole option itself — two value axes it built, one grid it built, none of the
 *     arithmetic below. Offering panels there would store a key that changes nothing, which is the
 *     defect this programme keeps closing. A scatter over a DIMENSION arrives at `baseOption` like
 *     every other kind and is offered them.
 *
 * Deliberately NOT among the refusals: a breakout. One panel per BAND is the same rule as one panel
 * per metric — Metabase counts `seriesModels` and so does this — and it is the case where the
 * feature earns the most, since a breakout is where a chart most often ends up with six series.
 */
export function splitPanelsApplies(
  cfg: Pick<CartesianBuildConfig, "kind" | "horizontal" | "xMetric" | "yMetrics">,
): boolean {
  if (isHorizontal(cfg)) return false;
  // The exact condition `buildCartesianOption`'s first line diverts on, asked here rather than
  // restated — a second spelling of it is how the panel comes to offer what the builder discards.
  if (cfg.kind === "scatter" && cfg.xMetric && (cfg.yMetrics ?? []).length > 0) return false;
  return true;
}

// A non-ordinal x-scale (linear/timeseries) positions points by their real x value
// instead of evenly-spaced categories — only on a vertical chart.
const usesXScale = (cfg: CartesianBuildConfig) =>
  !isHorizontal(cfg) && (cfg.xScale === "linear" || cfg.xScale === "timeseries");

/**
 * The x-scale a chart is DRAWN at, which is the stored one whenever there is one and otherwise
 * `linear` for a scatter whose x column holds numbers.
 *
 * Why: a scatter over `price` plotted 196 distinct prices as 196 CATEGORIES, evenly spaced and each
 * carrying its own label. ECharts kept whichever of those labels did not collide — three arbitrary
 * prices, `15.69 48.93 84.14` — and once PR 02 made the axis all-or-nothing it drew none at all.
 * Metabase draws `0 20 40 60 80 100` there, and not because it thins better: its x is a VALUE axis,
 * so the labels are round ticks ECharts generates from the extent rather than a subset of the data.
 * egret has had that axis all along (`usesXScale` -> `scaleAxis`); nothing ever selected it.
 *
 * SCATTER ONLY, and that is a rule rather than a starting point. A bar chart's categories are
 * discrete even when they are numbers — bar width, grouping and stacking are all computed off the
 * axis type — so putting bars on a value axis is a much larger change than this defect asks for.
 * A scatter has no such geometry: a point sits where its coordinates say.
 *
 * RESOLVED AT RENDER TIME, never written into the stored config. That is this programme's standing
 * rule and it is what makes a scatter saved before this change pick the new axis up. It also means
 * the answer moves with the data: re-point the chart at a varchar column and the category axis
 * comes back on its own.
 *
 * An explicit `xScale` — including `"ordinal"` — always wins, so a user who has said what they want
 * is never overridden. The panel offers that choice on a dimension-x scatter for exactly this
 * reason (`settings/CartesianDisplaySection.tsx`, `AxesOptions`): an automatic answer with no lever
 * beside it is not a default, it is a rule.
 *
 * ONE KNOWN EDGE, PRE-EXISTING BUT NEWLY REACHABLE. `isNumericType` matches `bigint` and `long`,
 * and a value axis coerces with `Number()` (`applyXScale`), which is a float64 — so a scatter over
 * an id column with values above 2^53 plots and drills on a rounded id. That has been true of a
 * hand-set Linear scale since it existed; what is new is that such a column reaches it without
 * anyone choosing. Narrowing the numeric test to exclude 64-bit integer types would also exclude
 * the ordinary bigint counts that make perfectly good scatter axes, so it is left alone and named
 * here rather than silently traded away.
 */
/**
 * Can this column be DRAWN on this scale at all?
 *
 * `applyXScale` re-keys every point onto a real coordinate — `Date.parse` for a timeseries,
 * `Number` for a linear one — and drops the ones that come out non-finite. Over a column whose
 * values coerce to nothing that is not a degraded chart, it is an EMPTY one: measured live on the
 * parity harness's `MB · Bar`, whose x is a text column, picking Timeseries drew no bars, no y-axis
 * values and no x tick labels. Two clicks, and saving keeps it.
 *
 * The answer is taken from the column's TYPE rather than from its values, for two reasons. The
 * panel has to grey out the same items and has only the type (it holds no rows); and it is
 * Metabase's own rule — `getDefaultXAxisScale`/the `graph.x_axis.scale` options there are computed
 * from the column's semantic type, offering Timeseries only on a temporal column and Linear (with
 * Power/Log/Histogram) only on a numeric one, with Ordinal always available because every column
 * can be drawn as a sequence of categories.
 *
 * THE COST, NAMED RATHER THAN HIDDEN: a `varchar` column holding ISO date strings coerces perfectly
 * well through `Date.parse`, and this refuses it, so such a chart saved with Timeseries falls back
 * to the ordinal axis. That is a downgrade — every point still draws, in order, since ISO strings
 * sort chronologically — where the alternative rule (judge the coerced VALUES) would have to live
 * in the builder and could not be shared with the panel, leaving the select greying items out by
 * one rule while the chart honoured another. A second rule for the same question is how this
 * programme's colour picker came to offer a swatch the chart did not draw. Cast the column, or pick
 * Ordinal, and nothing is lost.
 *
 * An unknown type permits everything: before a query has run there is no result column to judge
 * against, and refusing on a guess would silently rewrite a setting the user made.
 */
export function xScaleFitsColumn(
  scale: NonNullable<CartesianBuildConfig["xScale"]>,
  xColumnType: string | undefined,
): boolean {
  if (scale === "ordinal") return true;
  if (xColumnType == null) return true;
  return scale === "timeseries" ? isTemporalType(xColumnType) : isNumericType(xColumnType);
}

export function resolveXScale(
  kind: CartesianViz,
  xScale: CartesianBuildConfig["xScale"],
  xColumnType: string | undefined,
): CartesianBuildConfig["xScale"] {
  // A stored scale still wins — but only where the column can carry it. A refused one falls
  // THROUGH to the automatic answer rather than to `ordinal` outright, so a scatter over a numeric
  // column that had `timeseries` stored still gets the value axis it would have got with nothing
  // stored at all, instead of the 196-category axis that predates it.
  //
  // WHAT THE PANEL THEN SHOWS is "Ordinal", not the stored word greyed out, and that follows from
  // the select already reading the RESOLVED scale (`AxesOptions`, and the argument on that row):
  // printing "Timeseries" over a chart visibly drawing categories is the same
  // stored-says-one-thing-screen-says-another defect that row was written to avoid. The stored
  // value is not rewritten — nothing here writes to the config — so re-pointing the chart at a date
  // column brings the timeseries axis straight back, which is the same render-time-resolution rule
  // that lets a saved scatter pick up a better default.
  //
  // That is the OPPOSITE choice from the rotation items a few lines away in that panel, which stay
  // showing what they stored while greyed. The difference is which value the control is bound to:
  // `xTicksChoiceOf` reads the config, this select reads the resolution.
  if (xScale != null && xScaleFitsColumn(xScale, xColumnType)) return xScale;
  if (kind !== "scatter") return undefined;
  return xColumnType != null && isNumericType(xColumnType) ? "linear" : undefined;
}

/**
 * A point on a value/time x-axis: its coordinate pair, and the category it came from.
 *
 * THE `name` IS THE WHOLE REASON THIS IS AN OBJECT rather than the bare `[x, y]` array it used to
 * be. A value axis has no categories, so ECharts' `getDataParams` fills a clicked point's `name`
 * from `SeriesData.getName` — which returns `''` for a series that has neither a name list nor an
 * `itemName` dimension, which is every series this function used to produce. The drill read that
 * empty string and filtered `<column> = ''`, matching nothing (egretdb/egret#387).
 *
 * An item object gives ECharts the name list it was missing, so `p.name` is the RAW CATEGORY again
 * on every scale and the drill needs no special case for any of them. That matters most on a
 * `timeseries` axis, where the coordinate is `Date.parse`'s epoch milliseconds — a number the
 * column has never held, and one no amount of reading `data[0]` could have turned back into a
 * filter value.
 *
 * `String(c)`, because an ECharts `name` is a string. It is the same spelling the ordinal path
 * already sends — a category axis reports its category as a string too — so the two paths hand the
 * drill the same thing, and the backend validates a filter value against the column's type either
 * way (`chartquery.rs`, `sql_literal`, which parses a numeric out of a string).
 */
interface ScaledPoint { value: [number, number | null]; name: string }

// Re-key category-aligned series onto real x coordinates: each value becomes an ECharts item
// object at [x, y], dropping points whose x can't be parsed. Mutates the series in place.
function applyXScale(categories: unknown[], series: unknown[], cfg: CartesianBuildConfig) {
  const coord = (c: unknown) => (cfg.xScale === "timeseries" ? Date.parse(String(c)) : Number(c));
  const xs = categories.map(coord);
  for (const s of series as { data: (number | null)[] }[]) {
    s.data = s.data
      .map((y, i) => ({ value: [xs[i], y], name: String(categories[i]) } as ScaledPoint) as unknown as number | null)
      .filter((p) => Number.isFinite((p as unknown as ScaledPoint).value[0]));
  }
}

/**
 * The plotted y of a datum — the number the VALUE axis shows — through every shape a datum takes:
 * a bare number on a category axis, and since #387 a `{value: [x, y], name}` item object on a
 * value/time one. The bare `[x, y]` pair that item object replaced is unwrapped too, because
 * ECharts still accepts one and nothing here should depend on which of the two it is handed.
 *
 * `widestOf` is why this exists, and it is the reason `applyXScale` could not emit item objects on
 * its own: it read `d[1]` off the datum, which an object answers `undefined` to, so the
 * compact-label threshold it feeds would have measured `NaN` and silently become "never compact"
 * on every value-axis chart. `cartesian.labels.test.ts` pins both directions of that.
 */
/**
 * The (name, value) pairs a screen reader hears, taken from the SERIES rather than from the
 * category list — see the note at the `aria` block, which has the two defects this shape fixes.
 *
 * `drawnByRaw` maps a raw category to the string the axis draws for it, because an item object
 * carries the raw value (that is what a drill filters by) while the description's whole premise is
 * that it says what a sighted user sees.
 */
function ariaPoints(
  data: unknown[], drawnCategories: string[], drawnByRaw: Map<string, string>,
): { name: string; value: unknown }[] {
  return data.map((d, i) => {
    const raw = d != null && typeof d === "object" && !Array.isArray(d) && "name" in d
      ? String((d as { name: unknown }).name)
      : undefined;
    // `?? raw` rather than `?? ""`: a name that is not in the map means the two lists have drifted,
    // and the raw category is still a truthful thing to say where the drawn label is unavailable.
    const name = raw != null ? (drawnByRaw.get(raw) ?? raw) : (drawnCategories[i] ?? "");
    return { name, value: datumY(d) };
  });
}

function datumY(d: unknown): unknown {
  const v = d != null && typeof d === "object" && !Array.isArray(d) && "value" in d
    ? (d as { value: unknown }).value
    : d;
  return Array.isArray(v) ? v[1] : v;
}

// ECharts series type for a metric under the active kind.
/**
 * A breakout series' value as it should READ — the same question `drawLabel` answers for the x-axis,
 * about the other dimension, and now with the same three arms in the same order:
 *   1. the column's own format, when it says something a date formatter would consult — and on a
 *      BUCKETED series that bar is `specifiesDateRendering`, not mere existence, for the reason
 *      `drawLabel` sets out: the panel mints `{column_title: "Month"}` objects, and a format silent
 *      about dates would otherwise beat the period name with `formatDate`'s DEFAULTS;
 *   2. otherwise the NAME OF THE PERIOD the bucket covers — "January 2026" for a month, not its
 *      first day. This arm was MISSING, so a breakout by month drew `2026-01-01` in the legend,
 *      in the tooltip's series names, and in the aria description that reads the first band's name;
 *   3. otherwise the raw value. `periodName` is total and falls through to the raw string, so it IS
 *      answer 3 as well.
 * The early `isExtractUnit` return stays even though `periodName` refuses one too: an extraction
 * unit's value is a finished word ("Wed") that a stored date format must not be allowed to re-read,
 * and that is arm 1's guard, not arm 2's.
 *
 * The RAW value is untouched — `decorateSeries` keeps it on `__raw` because that is what a drill
 * filters by, and it is keyed by raw name for colours too.
 */
function seriesDisplayName(cfg: CartesianBuildConfig, raw: string): string {
  const col = cfg.series;
  if (col == null || isExtractUnit(cfg.seriesUnit)) return raw;
  const fmt = cfg.columnSettings?.[col];
  if (fmt != null && (namesAPeriod(cfg.seriesUnit) ? specifiesDateRendering(fmt) : true)) {
    const out = formatValue(raw, cfg.seriesType ?? "varchar", fmt);
    if (out !== "") return out;
  }
  return periodName(cfg.seriesUnit, raw);
}

/** What a series is DRAWN AS. "area" is not an ECharts series type — it is a line with a fill — so
 *  it stays separate from the `type` the option carries (see `echartsTypeOf`). */
export type SeriesDisplay = "bar" | "line" | "area";

/**
 * Whether a series on this chart kind may choose its own display — Metabase's `getHidden` on
 * `series.display` (`lib/settings/series.ts`), whose first arm names the four kinds verbatim:
 *
 *     !["line", "area", "bar", "combo"].includes(single.card.display ?? "")
 *
 * A ROW CHART AND A SCATTER ARE ABSENT FROM THAT LIST UPSTREAM, and the reason is the same one that
 * keeps them out of `seriesDisplayFor`'s lookup below: a row chart IS a horizontal bar and a scatter
 * IS points, so "draw this one as a line" would be a request to draw a chart the reader did not
 * open. Everything else in the family is a chart of BANDS OVER A CATEGORY AXIS, where a bar and a
 * line are two ways of drawing the same numbers and mixing them is the whole point of the combo.
 *
 * METABASE'S SECOND ARM — `settings["stackable.stack_type"] != null` — IS NOT HERE, on purpose. It
 * is a statement about a panel (a stack of mixed shapes is not readable), not about what the
 * builder can draw, and the panel already keeps it: `SeriesListControl` withholds the select while
 * `stackModeOf` is anything but "none". Restating it here would ALSO throw away a display a chart
 * had already stored the moment someone turned stacking on, which is a different and worse thing
 * than hiding a control.
 */
export function perSeriesDisplayApplies(kind: CartesianViz): boolean {
  return kind === "line" || kind === "area" || kind === "bar" || kind === "combo";
}

/**
 * A series' display, keyed by the SERIES name — the same key space as its colour and its rename, so
 * one row of the panel can carry all of them. It used to be keyed by metric, which a breakout has
 * only one of: every band of a broken-out combo was forced to share one display.
 *
 * IT USED TO CONSULT `seriesTypes` ON `combo` ALONE and return the chart kind for everything else,
 * which made a stored per-series display on a line, area or bar chart something the builder
 * DISCARDED rather than merely something the panel would not offer. Measured on a line chart
 * carrying `seriesTypes: {w: "bar"}`: `seriesDisplayFor("line", "w", 1, {w: "bar"})` answered
 * `"line"` and both bands came out `type: "line"`. Metabase offers the setting on all four kinds
 * (see `perSeriesDisplayApplies`), so the narrow lookup was the gap, and opening the panel gate
 * without it would have shipped a control that saves and does not paint.
 *
 * The DEFAULT stays where it was, and only the combo has a positional one: Metabase's
 * `getSeriesDefaultDisplay`, `index === 0 ? "line" : "bar"`. egret defaulted every series to "bar",
 * so a fresh combo was indistinguishable from a bar chart until the user changed something — the
 * one thing the chart type exists to show was off by default. The other three kinds default to
 * THEMSELVES, so a chart with nothing stored is byte-identical to what it was.
 *
 * A CHART SWITCHED FROM COMBO TO LINE NOW KEEPS ITS PER-SERIES DISPLAYS, because the cartesian
 * settings namespace is deliberately shared across kinds (`cartesian-codec.ts`) so bar→line keeps
 * its config. That is a behaviour change on already-saved charts and it is the intended one:
 * Metabase's `series.display` persists across a display change in exactly the same way, and the
 * alternative — clearing the map on every kind change — would throw away work the user did.
 */
export function seriesDisplayFor(
  kind: CartesianViz, seriesName: string, index: number, seriesTypes?: Record<string, SeriesDisplay>,
): SeriesDisplay | "scatter" {
  if (kind === "scatter") return "scatter";
  if (kind === "row") return "bar";
  return seriesTypes?.[seriesName] ?? (kind === "combo" ? (index === 0 ? "line" : "bar") : kind);
}

/**
 * Whether "Unpin from zero" is OFFERED on this chart — Metabase's `isYAxisUnpinFromZeroValid`
 * (`frontend/src/metabase/visualizations/shared/settings/cartesian-chart.ts:245-260`), arm for arm:
 *
 *     if (!settings["graph.y_axis.auto_range"] || settings["stackable.stack_type"] != null) {
 *       return false;
 *     }
 *     return seriesDisplays.every(
 *       (display) => display !== "area" && display !== "bar" && display !== "waterfall",
 *     );
 *
 * THE THREE REFUSALS ARE NOT ARBITRARY and porting them without the reason would make this look
 * like a list to trim. A bar and an area are drawn from a BASELINE — the bar's rectangle and the
 * area's fill both start at zero and their length IS the value — so an axis that does not include
 * zero draws bars whose relative heights lie about their relative values. A line and a scatter draw
 * a POSITION, which is legible at any extent. A stack is the same objection reached from the other
 * side: the pile is measured from the baseline, so its bottom cannot move. Metabase's third
 * exclusion, `waterfall`, is not a cartesian kind in egret and is expressed here as structure
 * instead — `viz/simple.tsx`'s waterfall panel simply never renders the control.
 *
 * THE FIRST GATE IS A TRANSLATION, not a copy, and it is the one thing here that is not verbatim.
 * Metabase stores `graph.y_axis.auto_range` as its own boolean key; egret has no such key, because
 * its "Auto y-axis range" switch is DERIVED — `ValueRangeRows` computes it as `yMin == null &&
 * yMax == null` and its own doc explains why (a fourth key is a fourth thing to disagree with the
 * two boxes). So the same rule is read off the same state by a different route. It matters for the
 * same reason it does upstream: an explicit bound already decides the extent outright, so a flag
 * asking for a different one would be an inert control.
 *
 * `kind` DECIDES A ROW WITHOUT ASKING ITS SERIES: `seriesDisplayFor` answers "bar" for a row chart,
 * so it is refused by the same line that refuses a bar rather than by a special case. A combo is
 * the case that needs the per-series walk, and it is the case where the default matters — a combo
 * left alone is line-then-bar, so it is refused until its second series is changed, which is
 * exactly what the live "MB · Combo" does.
 */
export function unpinFromZeroApplies(
  cfg: Pick<CartesianBuildConfig, "kind" | "yMetrics" | "seriesTypes" | "stacked" | "normalized" | "yMin" | "yMax">,
): boolean {
  // `!= null` and not truthiness: `yMin: 0` is a bound a user set, and it is the ONE bound a
  // truthiness test would drop — the pinned axis spelled out by hand.
  if (cfg.yMin != null || cfg.yMax != null) return false;
  if (resolveStacked(cfg)) return false;
  return (cfg.yMetrics ?? []).every((m, i) => {
    const d = seriesDisplayFor(cfg.kind, m, i, cfg.seriesTypes);
    return d !== "area" && d !== "bar";
  });
}

/**
 * …and whether it is ON, which is a second question because the DEFAULT is not the same on every
 * chart. Metabase's `getYAxisUnpinFromZeroDefault` (`cartesian-chart.ts:242`) is
 * `display === "scatter"` — a scatter is unpinned out of the box and everything else is pinned —
 * so a scatter that has never been touched must come out unpinned rather than merely offerable.
 */
export function unpinFromZeroOn(
  cfg: Pick<CartesianBuildConfig, "kind" | "yMetrics" | "seriesTypes" | "stacked" | "normalized" | "yMin" | "yMax" | "unpinFromZero">,
): boolean {
  if (!unpinFromZeroApplies(cfg)) return false;
  return cfg.unpinFromZero ?? cfg.kind === "scatter";
}

/** The ECharts series type behind a display. An area is a line that happens to be filled.
 *
 *  Exported because the settings panel has to ask the same question the builder does: `decorateSeries`
 *  applies `cfg.seriesLine` only where this returns "line", so the panel's drill-in button must appear
 *  exactly there. Restating "line or area" in the panel would be a second rule that drifts — which is
 *  how the colour rule drifted from the renderer once already. */
export function echartsTypeOf(display: SeriesDisplay | "scatter"): "bar" | "line" | "scatter" {
  return display === "area" ? "line" : display;
}

const distinct = (vals: unknown[]) => {
  const seen = new Set<string>();
  const out: unknown[] = [];
  for (const v of vals) { const k = String(v); if (!seen.has(k)) { seen.add(k); out.push(v); } }
  return out;
};

/**
 * Order the x categories by axis type — temporal chronologically, numeric ascending, and a
 * nominal (string) axis ascending by the category itself.
 *
 * THE NOMINAL BRANCH IS A DELIBERATE DIVERGENCE FROM METABASE, and this is why. Metabase's
 * `sortDataset` (model/dataset.ts) returns an ordinal dataset UNCHANGED — it can, because its
 * queries carry the order: an MBQL question gets an implicit ascending `order-by` on every
 * breakout column before it runs (`add-implicit-breakout-order-by`), and a native question has
 * whatever `ORDER BY` its author wrote. egret's STORED specs carry `orderby: []`, so first-seen
 * order here is whatever the engine happened to return — measured at 7/10 one ordering and 3/10
 * another across ten calls to the same chart, with three consecutive page loads all landing on the
 * 70% case. `cartesianDescriptor`'s `buildSpec` now asks the query for the order, but only charts
 * saved AFTER that carry it; the dashboard path deserialises the stored spec verbatim, so without
 * this branch every already-saved chart keeps drawing in engine order until someone re-saves it.
 *
 * Non-deterministic output that changes between runs is worse than any particular order — the same
 * reasoning `pivot/engine.ts`'s `sortTree` states, for the same `orderby: []`. Ascending by the
 * category matches what `buildSpec` now asks the engine for, so the two layers agree rather than
 * fighting, and it is the order Metabase's own query processor defaults to.
 *
 * `preserveCategoryOrder` is the exemption, and it is narrow: a caller whose rows are ALREADY in a
 * meaningful order keeps it. Arbitrary order is the thing being fixed, and two orders are not
 * arbitrary — a pipeline (the funnel's bar mode, steps from `funnel.stepOrder`) and one the query
 * itself asked for. The second is why the sentence above about egret's stored specs is now only
 * half true: a chart whose author added a sort stores a NON-EMPTY `orderby`, the engine returns its
 * rows in that order, and `ChartRenderer` sets the flag so this leaves them in it. Sorting them
 * anyway is how a sort control ends up doing nothing on screen while its own tests pass.
 *
 * Exported because `options/boxplot.ts` has the same nominal axis and had the same defect by a
 * different route, and one shared function is what stops the two drifting apart — the box plot's
 * categories must sort exactly as a bar chart's do, including the temporal and numeric branches.
 */
export function sortCategories(cats: unknown[], type: string, preserve?: boolean): unknown[] {
  if (isTemporalType(type)) return [...cats].sort(byDerivedKey((v) => Date.parse(String(v))));
  if (isNumericType(type)) return [...cats].sort(byDerivedKey(Number));
  if (preserve) return cats;
  // The nominal branch derives no number, so it tie-breaks directly rather than through
  // `byDerivedKey`. `localeCompare` returning 0 is the tie — see `byRawString` for what makes the
  // fallback total, and why it is code units here and not a second `localeCompare`.
  return [...cats].sort((a, b) => String(a).localeCompare(String(b)) || byRawString(a, b));
}

/**
 * THE LAST RESORT OF EVERY ORDERING IN THIS FILE: the raw value's own string, compared by code unit.
 *
 * WHY IT IS NEEDED AT ALL. Each branch of `sortCategories` compares on a key DERIVED from the value
 * — `Number(v)`, `Date.parse(v)`, the collation `localeCompare` uses. Deriving is lossy, so two
 * values that are genuinely different can land on one key: `Number(null)`, `Number("")` and
 * `Number(0)` are all 0; `"2026-01-01T00:00:00Z"` and `"2026-01-01T00:00:00.000Z"` are one instant;
 * `"Café"` in NFC and `"Café"` in NFD are one piece of text that `localeCompare` — correctly —
 * calls equal. `distinct` deduplicates on `String(v)`, not on the derived key, so it keeps both
 * members of every such pair and hands them to the comparator. A comparator returning 0 tells a
 * stable sort to leave them as it found them, and as it found them IS ARRIVAL ORDER, which is the
 * engine's row order, which is the precise nondeterminism #394 and #436 were written to delete.
 * Both PRs replaced arrival order in the general case and left it in the tied case. This closes it.
 *
 * WHY IT ALWAYS SETTLES, rather than merely making a tie less likely — which is the only property
 * worth having, since "usually deterministic" is what we already had. Everything reaching these
 * comparators has been through `distinct`, and `distinct`'s dedup key is exactly `String(v)`. So
 * within any one list the raw strings are pairwise DISTINCT by construction and this can never
 * itself return 0. The result is a TOTAL order, and total is what makes the output a function of
 * the input set rather than of the input sequence.
 *
 * WHY CODE UNITS AND NOT `localeCompare`. On the nominal branch `localeCompare` is the comparison
 * that just tied; asking it again gets the same 0 and settles nothing. `<` on strings is a raw
 * code-unit comparison with no collation, no locale and no equivalence classes, so it separates
 * anything `distinct` kept apart. It is deliberately NOT the human-readable order — it is the
 * arbitrary-but-fixed order that runs underneath the human-readable one, and it is only ever
 * consulted for values the readable order could not separate.
 *
 * REJECTED: sorting the ORIGINAL index in as the secondary key, i.e. a Schwartzian transform that
 * makes the existing stable-sort behaviour explicit. It is the smaller diff and it is exactly wrong
 * — it would PIN arrival order rather than replace it, encoding today's engine output as the
 * intended answer and making the nondeterminism permanent instead of fixing it. That is the same
 * trap #436 recorded when reusing `preserveSeriesOrder` on the band path would have pinned a wrong
 * order.
 */
const byRawString = (a: unknown, b: unknown): number => {
  const x = String(a);
  const y = String(b);
  return x < y ? -1 : x > y ? 1 : 0;
};

/**
 * A comparator over a NUMERIC derived key, with two guarantees the bare subtraction it replaces did
 * not have: ties fall to `byRawString`, and a key that is not a number cannot corrupt the result.
 *
 * THE NaN CASE IS THE BIGGER HALF, and it is not a tie — it is an INCONSISTENT COMPARATOR. Both old
 * comparators were subtractions, and `Number("n/a")` and `Date.parse(String(null))` are NaN, so the
 * subtraction returned NaN: not negative, not zero, not positive. The spec makes the entire sort
 * implementation-defined at that point, and V8 does not merely misplace the offending element — it
 * stops sorting the elements AROUND it. Measured on Node 22, twelve months with one NULL among them:
 * `Jan Feb Mar Apr May Jul Sep null Jun Aug Oct Nov`. June and August drawn to the right of
 * September, chronology broken across the whole axis, from one null in a date column. That is not a
 * hypothetical column either — the row filter upstream (`rows.filter((r) => r[xi] != null)`) drops
 * nulls from the CATEGORY column only, and the BAND column is never null-filtered at all;
 * `cartesianSeriesNames` folds a null band to `(empty)` AFTER this sort, by design.
 *
 * UNPARSEABLE VALUES SORT LAST, ordered among themselves by `byRawString`. Last rather than first
 * because a value with no position on the axis is not a value at the axis's beginning, and a reader
 * scanning for a missing or malformed entry looks at the end. It also agrees with where the engine
 * would have put them had the query carried the sort this client-side pass exists to substitute
 * for: Trino's `ORDER BY … ASC` defaults to NULLS LAST, as does Postgres. So a chart whose author
 * later adds an explicit ascending sort — which sets `preserveCategoryOrder` and takes the engine's
 * ordering verbatim — does not see its missing values jump from one end to the other. Whichever end
 * was chosen the point is that it is a FIXED one; the previous behaviour placed them nowhere in
 * particular and moved the real values around them.
 */
const byDerivedKey =
  (key: (v: unknown) => number) =>
  (a: unknown, b: unknown): number => {
    const ka = key(a);
    const kb = key(b);
    const aBad = Number.isNaN(ka);
    const bBad = Number.isNaN(kb);
    if (aBad || bBad) return aBad && bBad ? byRawString(a, b) : aBad ? 1 : -1;
    // `===` before subtraction so that equal infinities (Infinity - Infinity is NaN) reach the
    // tie-break rather than re-introducing the NaN this function exists to keep out.
    return ka === kb ? byRawString(a, b) : ka - kb;
  };

/**
 * The same order, applied to the OTHER axis of the chart: the bands a breakout draws.
 *
 * THE DEFECT. The bands came from `distinct(xrows.map(...))`, which is ARRIVAL ORDER, from a spec
 * that mostly carries no sort on the breakout column at all. Measured on the demo dataset: one card
 * drew `Doohickey / Gadget / Gizmo / Widget` on one page load and `Gizmo / Widget / Gadget /
 * Doohickey` on the next, from identical data and identical settings. Pre-existing and independent
 * of split panels — the un-panelled card on the same load was equally arbitrary; panels only put
 * two orderings side by side where one could see them.
 *
 * NOT COSMETIC, which is why it is worth a sort rather than a note. Band order is the LEGEND order,
 * the STACKING order (a stack is drawn bottom-up in series order) and the DRAW order. It also feeds
 * three things that read the band's INDEX rather than its name: `seriesDisplayFor`'s positional
 * default on a combo (`index === 0 ? "line" : "bar"`), `colorsForKeys`' collision walk (a pinned
 * colour consumes a slot and the band that would have hashed onto it moves along — which band moves
 * depends on the order they are offered in), and the split-panel layout. So arrival order did not
 * merely shuffle the legend: it could hand the same band a different colour and a different mark
 * between two loads.
 *
 * A THIN WRAPPER OVER `sortCategories` AND NOT A SECOND SORT, for the reason that function's own
 * comment gives about the box plot: two copies of an ordering rule is how two axes come to disagree
 * about what "ascending" means. The temporal and numeric branches matter here exactly as much as
 * they do there — a breakout by month must band chronologically and not `2026-01, 2026-02, 2026-10`
 * by luck of the string, and a breakout by an integer id must band `1, 2, 10` and not `1, 10, 2`.
 * It exists as a named function rather than a bare call so that the WHY above has a home and so both
 * the builder and the settings panel are visibly asking the same question.
 *
 * WHERE IT SITS IN THE PIPELINE — at the point the bands are DERIVED, before `decorateSeries`, and
 * therefore before `applySeriesOrder`. Both halves of that are load-bearing:
 *
 *  - BEFORE `decorateSeries`, so it sorts the RAW column value. `decorateSeries` replaces `name`
 *    with the display label (a `seriesLabels` rename, the period a temporal bucket covers, the
 *    column's own number format), and sorting on that would put a band renamed "AAA" first and
 *    re-order the whole chart on a cosmetic edit. This codebase has had eight defects from keying on
 *    a display string; `settingsKey` and `__raw` exist because of the last of them.
 *  - BEFORE `applySeriesOrder`, so an explicit `seriesOrder` LOSES NOTHING. That function sorts the
 *    built series by the user's stored rank afterwards, so this only decides the order of the bands
 *    it does not mention — which is exactly the "natural order" its own comment promises. Sorting
 *    after it would have overwritten the user's arrangement; sorting before it also means the
 *    positional readers listed above (the combo default, the colour walk) see the deterministic
 *    order rather than the arbitrary one, which sorting afterwards could never have fixed.
 *    It does not interact with `hiddenSeries` either: dropping members of a sorted list leaves it
 *    sorted.
 *
 * ONE DIVERGENCE FROM `sortCategories`, AND IT IS THE `preserve` ARM. There, `preserve` is honoured
 * on the nominal branch only: a temporal or numeric CATEGORY axis is sorted whatever the query
 * asked for, because an axis that runs left to right through time backwards is not a preference,
 * it is a misread waiting to happen. A BAND has no such geometry. It is a legend entry and a stack
 * layer, so "months, newest first" is an ordinary thing for an author to ask for and there is
 * nothing about the drawing that contradicts it. `preserve` therefore short-circuits every type
 * here, which is why this is a function and not a bare `sortCategories` call at each site.
 *
 * THE TIE-BREAK IS INHERITED, NOT RESTATED, and that is this wrapper's design paying for itself. A
 * band list could tie for exactly the reasons a category list can — and more often, because the
 * band column is the one that is never null-filtered (`buildCartesianOption` drops null CATEGORIES
 * only) so `Number(null) === Number(0)` and `Date.parse(String(null))` being NaN are live here in a
 * way they are not on the x axis. Because this delegates rather than re-sorting, `byRawString` and
 * `byDerivedKey` reached both axes the moment they were added to `sortCategories`, with no second
 * edit and no chance of the two axes settling the same tie two different ways. Had this been a
 * copied comparator the fix would have landed on one axis and the drift would have been invisible.
 *
 * THE `preserve` SHORT-CIRCUIT IS ABOVE ALL OF IT AND STAYS THERE. Tie-breaking is part of deciding
 * an order; `preserve` is the arm that declines to decide one. A tie cannot arise where nothing is
 * compared, so the guard needs no tie logic and must not acquire any — reaching past it to
 * "stabilise" a preserved list would pin the arrival order this file spent two PRs removing.
 */
function sortBands(bands: unknown[], type: string, preserve?: boolean): unknown[] {
  if (preserve) return bands;
  return sortCategories(bands, type);
}

// Metric-vs-metric scatter (Metabase's true scatter): each dimension row is a point
// at (xMetric, yMetric) on two value axes.
// Metabase's bubble scale (scatter/option/series.ts), verbatim including its rationale: the size
// interpolates by AREA, not by radius, because a radius-linear scale exaggerates large values —
// doubling the radius quadruples the ink. `d = 2·√(area/π)`.
const MIN_BUBBLE_DIAMETER = 15;
const MAX_BUBBLE_DIAMETER = 75;

/** Diameter for a bubble value, given the metric's extent. Returns the floor when there is nothing
 *  to scale against — a single point, or an extent of width zero. */
export function bubbleDiameter(value: number, lo: number, hi: number): number {
  if (!Number.isFinite(value) || !(hi > lo)) return MIN_BUBBLE_DIAMETER;
  const t = Math.min(1, Math.max(0, (value - lo) / (hi - lo)));
  const areaLo = Math.PI * (MIN_BUBBLE_DIAMETER / 2) ** 2;
  const areaHi = Math.PI * (MAX_BUBBLE_DIAMETER / 2) ** 2;
  return 2 * Math.sqrt((areaLo + t * (areaHi - areaLo)) / Math.PI);
}

/**
 * Metric-vs-metric scatter (Metabase's true scatter): each dimension row is a point at
 * (xMetric, yMetric) on two value axes.
 *
 * THE CHART-WIDE PANEL KEYS THIS FUNCTION USED TO IGNORE, and the reason the list is now empty.
 *
 * This builder makes its own option from nothing rather than falling through to `baseOption`, which
 * is why it kept arriving at the same defect from a new direction: every setting `baseOption` learns
 * has to be taught to it separately or it is silently dropped on one chart type. #418 and #424 taught
 * it the six axis keys (`yMin`, `yMax`, `yTickCount`, `yAxisTicks`, `unpinFromZero`, `xAxisTicks`)
 * and left eleven more, all of which the Display and Axes tabs go on offering with an X metric
 * picked: `xLabel`, `showXLabel`, `yLabel`, `showYLabel`, `showLegend`, `showLabels`, `labelDensity`,
 * `compactValues`, `goalValue`/`goalLabel`, `hiddenSeries` and `seriesOrder`. Measured by building
 * the same chart twice, once with each key set and once without: byte-identical, every time, while
 * the same key on a scatter WITHOUT an X metric changes the option. Same chart type to the reader,
 * one Data-tab select apart.
 *
 * WHY READ THEM RATHER THAN WITHDRAW THE ROWS, per the rule stated on the `yAxis` literal below —
 * *a divergence is only honest while the panel keeps its half of it*. Withdrawing a row here only
 * moves the inconsistency: the panel cannot hide `Data labels` on one scatter and show it on the
 * other without controls that appear and vanish as a neighbouring select changes, and a chart that
 * already stored `showLabels: true` would keep painting on the category-x flavour with nothing left
 * to turn it off. Metabase's own `ScatterViz` spreads `GRAPH_AXIS_SETTINGS` and `GRAPH_GOAL_SETTINGS`
 * outright, so the titles and the goal line are parity rather than invention; `showLabels` and
 * `labelDensity` are not (its scatter omits `GRAPH_DISPLAY_VALUES_SETTINGS` entirely), and they are
 * honoured anyway because egret already ships them live on the category-x scatter.
 *
 * WHAT IS STILL DELIBERATELY ABSENT, so the next sweep does not report it a third time:
 * `maxCategories` (the control is not declared for any scatter — there is no category axis to fold),
 * `trendLine` (see below), and `xLabelRotation` / `xLabelCompact` (offered on every cartesian kind
 * but visibly greyed here, with a caption, because both axes are value axes whose ticks ECharts
 * generates). All three are the panel keeping its half; none of them is a key saved and dropped.
 *
 * `trendLine`'S ENTRY IN THAT LIST USED TO READ "`TrendOption` renders nothing on a scatter, so no
 * row is offered", AND IT WAS TRUE OF THE WRONG SET. `TrendOption` tested `kind !== "scatter"` and
 * therefore withheld the switch from BOTH scatter flavours, including the one whose trend line this
 * builder has always drawn: a scatter with a dimension on x misses the early return above and falls
 * through to the ordinary path, where `if (cfg.trendLine) series.push(...trendSeries(series))`
 * stands at both call sites. So the sentence recorded a fact about `TrendOption` as though it were
 * a fact about scatters, and hid a live panel/builder disagreement pointing the OTHER way from the
 * eleven this comment is otherwise about — builder honours, panel does not offer. `TrendOption` now
 * gates on `config.xMetric != null`, which is this function's own entry condition, and the switch
 * is on offer wherever the fit is drawn.
 *
 * THIS FUNCTION STILL DRAWS NO TREND, and that is the meaningfulness half rather than an unhonoured
 * key. `trendData` solves `y = m·i + b` over the datum INDEX — a fit against series POSITION, run
 * before `applyXScale` maps points onto real coordinates — which has no referent on a cloud of
 * points in whatever order the engine returned. Fitting y on x instead would be a different
 * statistic under the same switch on one chart type; Metabase declines it too, its scatter's
 * `graph.show_trendline` being gated on server-side `insights` that exist only for a result with
 * one datetime column. A chart that carries `trendLine: true` in here keeps the key and draws
 * nothing, exactly as `xScale` does.
 *
 * NO X-AXIS SCALE OR X BOUNDS EITHER, unchanged: Metabase's keys are `graph.y_axis.scale`/`.min`/
 * `.max` with no x twin, so those would be settings with no parity, no control and no way to undo.
 */
function scatterXY(columns: ResultColumn[], rows: unknown[][], cfg: CartesianBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const di = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
  const xMetric = cfg.xMetric!;
  const xi = idx[xMetric];
  const gi = cfg.series != null ? idx[cfg.series] : undefined;
  const bi = cfg.bubbleMetric != null ? idx[cfg.bubbleMetric] : undefined;
  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const fmtM = (m: string, v: unknown) => formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]);

  /**
   * EVERY Y METRIC, WHERE THIS FUNCTION USED TO READ `cfg.yMetrics[0]` AND STOP.
   *
   * MEASURED: `yMetrics: ["revenue", "orders"]` on a metric-vs-metric scatter came back with
   * `series.length === 1`. `YAxisControl` is the shared, uncapped `FieldZone` every cartesian kind
   * uses and `cartesianDescriptor.buildSpec` puts all of `cfg.yMetrics` into `spec.metrics`, so the
   * second metric was projected, aggregated, transferred and dropped on the floor. The panel had
   * gone further still: `cartesianSeriesNames` returns `[...yMetrics]` with no breakout, so the
   * Series list was offering a colour, a rename and a Show switch for a series nothing drew.
   *
   * Drawing them is Metabase's own answer — one `type: "scatter"` series per metric, each encoding
   * the same x key and its own y (`echarts/cartesian/scatter/option/index.ts:40-56`,
   * `option/series.ts:60-69`), with no cap on the count (`ScatterViz` sets no
   * `maxMetricsSupported`, so `getMaxMetricsSupported` returns `Infinity`). Capping the CONTROL at
   * one instead was the obvious alternative and is rejected in `cartesian.scatterMultiMetric.test.ts`
   * along with the third option, which was to divert the surplus into the tooltip metrics beside it.
   *
   * WITH A BREAKOUT, THE FIRST METRIC ONLY, because the bands are the series then and a cross
   * product would put two clouds on every band. That is Metabase's rule too
   * (`lib/graph/columns.ts:117-124`) and, more to the point, it is already egret's everywhere else:
   * `buildCartesianOption`'s breakout branch takes `cfg.yMetrics[0]`, and `SeriesControl` has
   * always printed "With a series, only the first metric is plotted." under the select. This chart
   * used to apply that caption unconditionally; now it applies it where the caption applies.
   */
  const drawn = gi != null ? cfg.yMetrics.slice(0, 1) : cfg.yMetrics;
  /** The metric whose format and title the two shared, chart-wide readers take: the value axis's
   *  number format and — when there is only one — its title. `baseOption` narrows the same way
   *  ("An axis plotting several metrics has no one name"). */
  const yMetric = drawn[0];
  /** The tooltip metrics, resolved once for the whole chart rather than per point: the answer is a
   *  property of the config and the result's columns, neither of which varies by row. */
  const extraMetrics = extraTooltipMetrics(cfg, idx);

  interface Pt { value: number[]; name: string; bubble?: number; extra?: TooltipExtra[] }
  const points: (Pt & { group: string; metric: string })[] = [];
  for (const r of rows) {
    const x = Number(r[xi]);
    if (!Number.isFinite(x)) continue;
    const b = bi != null ? Number(r[bi]) : undefined;
    // THE EXTRA ROWS RIDE ON THE DATUM, where the ordinary path has to key them by category. A
    // point here IS a row — no grouping, no folding, no `applyXScale` rewriting the array under it
    // — so the honest place for a row's other values is the row's own item object, exactly as
    // `bubble` already does. ECharts hands `params.data` back to the formatter untouched.
    const extra = extraMetrics.length > 0
      ? extraMetrics.map((m) => ({ metric: m, value: r[idx[m]] }))
      : undefined;
    for (const metric of drawn) {
      const y = Number(r[idx[metric]]);
      if (!Number.isFinite(y)) continue;
      points.push({
        value: [x, y],
        name: di != null ? String(r[di] ?? "") : "",
        ...(b != null && Number.isFinite(b) ? { bubble: b } : {}),
        ...(extra ? { extra } : {}),
        group: gi != null ? String(r[gi] ?? "") : "",
        metric,
      });
    }
  }

  // The bubble extent spans EVERY point, not each series' own — otherwise the same value would
  // draw at two sizes depending on which breakout band it landed in.
  const bubbles = points.map((p) => p.bubble).filter((v): v is number => v != null);
  const bLo = bubbles.length ? Math.min(...bubbles) : 0;
  const bHi = bubbles.length ? Math.max(...bubbles) : 0;
  // Total on purpose: ECharts calls this per symbol during layout, and anything thrown in there
  // takes the whole series down silently — the chart keeps its axes and simply draws no points.
  const sizeOf = (p: Pt | undefined) => {
    const v = p?.bubble;
    return bi == null || v == null ? CHART_THEME.scatterSymbolSize : bubbleDiameter(v, bLo, bHi);
  };

  // The scatter's bands, ordered by the same rule the bar/line breakout uses (`sortBands`) — this
  // chart derived them from arrival order too, and its clouds carry the same legend, the same
  // colours and the same per-band panel as any other breakout. `gi` is non-null exactly when
  // `cfg.series` is, since both are read off the same lookup.
  const groups = cfg.series != null && gi != null
    ? sortBands(distinct(points.map((p) => p.group)), columns[gi].type, cfg.preserveSeriesOrder).map(String)
    : [""];
  /**
   * ONE SERIES PER (band, metric) PAIR — but never a cross product, because the two cases are
   * mutually exclusive by construction: `drawn` is narrowed to one metric the moment a breakout
   * exists, so this list is the bands OR the metrics and never both.
   */
  const pairs = groups.flatMap((g) => drawn.map((metric) => ({ g, metric })));
  // Keyed by GROUP NAME so a breakout value keeps its colour across every card that breaks out by
  // it — and so an explicit `seriesColors` entry consumes its colour rather than shadowing it.
  //
  // An un-broken-out scatter has one group whose name is the empty string, and hashing THAT would
  // give every such card the same colour whatever it plots. It is keyed by the metric instead —
  // which is also the name the series is labelled with two lines below, and the key
  // `cartesianSeriesNames` hands the settings panel, so a pinned colour now actually finds its
  // series. Keying the lookup on "" meant `cfg.seriesColors` could never match on this path.
  //
  // THE METRIC, NOT `yMetric`, and that is what carried this key through the multi-metric change
  // unaltered: an un-broken-out scatter's series ARE its metrics now, so "the empty group's name is
  // its y-metric" becomes "each series' name is its own metric" — the same sentence, once there is
  // more than one of them. `cartesianSeriesNames` already returned `[...yMetrics]` here, so this is
  // the builder catching up with the key space the panel was always using.
  const groupKey = (g: string, metric: string) => (g === "" ? metric : g);
  const groupColors = colorsForKeys(pairs.map((p) => groupKey(p.g, p.metric)), cfg.seriesColors);
  /**
   * THE DATA LABEL, and it is the Y value on purpose. A point here has two numbers and only one of
   * them can be written beside it without the label becoming a second chart; `baseOption`'s own
   * label formatter picks the y for exactly the same reason on a value/time x-axis
   * ("the datum is an [x, y] pair — label the y"), and this is the same rule on the same shape.
   *
   * THROUGH THE EXACT FORMATTER, not the compact one two lines down. A label sits on the data and
   * an axis tick sits on the scale; the normal path splits them the same way (`fmt` for labels,
   * `fmtFor` for axes), so "Shorten large numbers" shortens an axis and never a value.
   *
   * PER SERIES, because each cloud now carries its own metric and therefore its own format — a
   * shared closure over `yMetric` would have printed the first metric's currency prefix beside the
   * second metric's count.
   */
  const pointLabelFor = (metric: string) => (cfg.showLabels
    ? {
      label: {
        show: true, fontSize: CHART_THEME.labelFontSize, fontWeight: CHART_THEME.labelWeight,
        position: "top" as const,
        formatter: (p: { value: unknown }) =>
          fmtM(metric, Array.isArray(p.value) ? p.value[1] : p.value),
      },
      // "fit" is the default here too, and a scatter needs it more than a bar chart does: its
      // points overlap by nature, so "all" on a dense plot is a wall of numbers.
      ...(cfg.labelDensity !== "all" ? { labelLayout: { hideOverlap: true } } : {}),
    }
    : {});
  const built = pairs.map(({ g, metric }) => {
    const own = points.filter((p) => p.group === g && p.metric === metric);
    const key = groupKey(g, metric);
    const color = groupColors[key];
    return {
      type: "scatter" as const,
      // `groupKey(g)`, NOT the raw `g` — the same key the colour lookup two lines up uses, and for
      // the same reason it was introduced. `cartesianSeriesNames` hands the settings panel the
      // Y-METRIC for a scatter with no breakout, `SeriesListSection` stores the rename back under
      // whatever name it was handed, and this line then looked it up under the raw group `""`. So a
      // rename was written to `seriesLabels.revenue` and read from `seriesLabels[""]`: saved,
      // round-tripped through the codec, and drawn nowhere. `seriesColors` worked and `seriesLabels`
      // did not, off two lookups seven lines apart on the same value.
      //
      // `settingsKey`'s comment in this file had already written down the fact that settles it —
      // "an un-broken-out scatter's one group is named `""` and is labelled with its y-metric, which
      // is the key both the builder and this panel use" — while the line it describes used the
      // other one.
      //
      // The `?? groupKey(g)` tail is what the old expression's `(g === "" ? yMetric : g)` spelled
      // out by hand; it is the same value, said once. A breakout band is unaffected either way,
      // `groupKey` being the identity on any non-empty group — which is exactly why a fix that only
      // reached the empty case would look complete and pin nothing.
      name: cfg.seriesLabels?.[key] ?? key,
      // `key` HERE TOO, and this is the settings key rather than the drill's — see
      // `settingsKey`, which `applySeriesOrder` below reads it through. The two differ only when
      // `g` is `""`, which happens only without a breakout, and `ChartRenderer` consults
      // `rawSeriesAt` only inside `if (cartesian.series)`. So no drill can observe the difference
      // and the Show switch, which is keyed on what `cartesianSeriesNames` returns, can.
      __raw: key,
      itemStyle: { color, opacity: CHART_THEME.scatterOpacity },
      symbolSize: (_v: unknown, params?: { data?: Pt }) => sizeOf(params?.data),
      // The same lift-and-recede the rest of the family got; a scatter with several bands needs it
      // most, since its points overlap by nature.
      emphasis: { focus: "series" as const },
      blur: { itemStyle: { opacity: CHART_THEME.blurOpacity } },
      ...pointLabelFor(metric),
      data: own,
    };
  });
  // The panel's own Show switch and reorder arrows, through the SHARED function rather than a
  // second copy of it — `applySeriesOrder` was made generic for this call. Order is the draw order
  // and the legend order, and on a scatter it is also which band's points land on top.
  const series = applySeriesOrder(built, cfg);

  /**
   * SHORTENED AXIS NUMBERS, per axis rather than chart-wide, and that asymmetry is `baseOption`'s
   * own: "a dual-axis chart has two scales on purpose, and the small one should not inherit the big
   * one's shorthand". This chart is always two-scaled, so the point applies to every one of them.
   * 10,000 is where a grouped number reaches six characters and "10K" starts winning — the same
   * threshold, read from the same place, so the two paths cannot drift into different ideas of
   * "wide". A column stating its own `compact` still outranks the chart-wide answer.
   */
  const widestOfMetric = (i: number) =>
    points.reduce((w, p) => Math.max(w, Math.abs(p.value[i])), 0);
  const axisFmt = (m: string, widest: number) => {
    const compact = cfg.compactValues ?? widest >= 10_000;
    const col = cfg.columnSettings?.[m];
    return (v: number) =>
      formatValue(v, cfg.metricTypes?.[m] ?? "double", { ...col, compact: col?.compact ?? compact });
  };

  /**
   * THE AXIS TITLES. `displayTitle` is `baseOption`'s precedence asked rather than restated — a
   * per-series rename, then the column's `column_title`, then the raw name — which is what stopped
   * this being the one cartesian flavour where renaming a column left its axis on the machine name.
   *
   * The two `show…Label` switches HIDE rather than blank, and the grid gives the 22px back when the
   * bottom title goes: a hidden title that still reserves its gutter is the setting only half done.
   *
   * NO Y TITLE ONCE TWO METRICS SHARE THE AXIS, which is `baseOption`'s own `metricTitle` rule ("An
   * axis plotting several metrics has no one name — the legend names them instead") reaching this
   * builder with the metrics that reached it. Naming it for `drawn[0]` would label the axis for one
   * of the two clouds standing on it. An explicit `cfg.yLabel` still wins, as everywhere else: a
   * reader who has typed a name for the pair has answered the question this rule declines to guess.
   */
  const xTitle = cfg.showXLabel === false ? undefined : (cfg.xLabel || displayTitle(cfg, xMetric));
  const yTitle = cfg.showYLabel === false
    ? undefined
    : (cfg.yLabel || (drawn.length === 1 ? displayTitle(cfg, yMetric) : undefined));

  /**
   * THE LEGEND, now an override of the automatic answer rather than the automatic answer alone.
   * `?? (pairs.length > 1)` is the same shape `baseOption` uses (`cfg.showLegend ?? (hasBreakout ||
   * yMetrics.length > 1)`) — both of its arms, now that both can be true here — and it is read off
   * the PAIRS rather than off the surviving series so that hiding bands down to one does not
   * silently retract a legend the reader asked for.
   */
  const showLegend = cfg.showLegend ?? pairs.length > 1;

  // Goal line — a horizontal reference at goalValue, on the first series' markLine, exactly as
  // `baseOption` draws it. `!= null` and not truthiness: a goal of 0 is a break-even line. Guarded
  // on there being a series at all, since hiding every band leaves nothing to hang it off.
  if (cfg.goalValue != null && series.length > 0) {
    (series[0] as Record<string, unknown>).markLine = {
      silent: true,
      symbol: "none",
      data: [{ yAxis: cfg.goalValue }],
      label: { formatter: cfg.goalLabel ?? "Goal", position: "insideEndTop", color: axisLabelColor },
      lineStyle: { color: axisLabelColor, type: "dashed" },
    };
  }

  return {
    grid: { ...CHART_THEME.grid, bottom: (CHART_THEME.grid.bottom as number) + (xTitle != null ? 22 : 0) },
    tooltip: {
      trigger: "item", ...tooltipSurface(),
      /**
       * THE HOVERED POINT'S OWN METRIC, read off the datum rather than closed over. This line used
       * to name `yMetric` — correct while there was only ever one, and the second cloud on a
       * two-metric scatter would have reported the first one's name and the first one's format
       * beside its own number.
       *
       * `p.data.metric` and not `p.seriesName`: a renamed series would otherwise be looked up in
       * `metricTypes` and `columnSettings` under its display label and silently fall back to
       * `"double"` with no format — the same rename-breaks-the-lookup defect the `__raw`/`name`
       * split above exists to prevent, one component over.
       */
      formatter: (p: { data: Pt & { metric?: string }; value: number[] }) => {
        const m = p.data.metric ?? yMetric;
        const head = p.data.name ? `${p.data.name}<br/>` : "";
        const bubble = cfg.bubbleMetric != null && p.data.bubble != null
          ? `<br/>${cfg.bubbleMetric}: ${fmtM(cfg.bubbleMetric, p.data.bubble)}`
          : "";
        // The additional tooltip metrics, last and unmarked — see the same block in `baseOption`
        // for why they sit after everything drawn. They came in on the datum, so the point's own
        // row is what they describe and no lookup can slide.
        const extra = (p.data.extra ?? [])
          .map((e) => `<br/>${displayTitle(cfg, e.metric)}: ${fmtM(e.metric, e.value)}`)
          .join("");
        return `${head}${cfg.xMetric}: ${fmtM(cfg.xMetric!, p.value[0])}<br/>${m}: ${fmtM(m, p.value[1])}${bubble}${extra}`;
      },
    },
    ...(showLegend ? { legend: { type: "scroll", bottom: 0, textStyle: { color: axisLabelColor } } } : {}),
    /**
     * `xAxisTicks` REACHES THIS AXIS NOW, and it is the one place in the family it did not.
     * `xCategory` and `scaleAxis` have both keyed their `axisLine`/`axisTick` on it since they were
     * written, so a metric-vs-metric scatter was the single chart whose "X show lines and tick
     * marks" select was offered and thrown away. Found while measuring the Y rows below rather than
     * reported separately, and fixed in the same breath rather than left for whoever noticed second.
     *
     * NO SCALE AND NO BOUNDS HERE, deliberately. Metabase's keys are `graph.y_axis.scale`/`.min`/
     * `.max` with no x twin, so an x log axis or an x range would be a setting with no parity, no
     * control, and no way to undo it. `xAxisTicks` is not in that class — it is an existing stored
     * key whose control this chart has always shown.
     */
    xAxis: {
      type: "value", name: xTitle, nameLocation: "middle", nameGap: 28,
      // The family's title ink (`baseOption`'s axes do the same); ECharts' default reads far paler.
      nameTextStyle: { color: axisLabelColor },
      axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: axisFmt(xMetric, widestOfMetric(0)) },
      axisLine: { show: cfg.xAxisTicks !== false, lineStyle: { color: gridLine } },
      axisTick: { show: cfg.xAxisTicks !== false },
      splitLine: { lineStyle: { color: gridLine } },
    },
    /**
     * THE VALUE-AXIS KEYS THIS PATH USED TO IGNORE, and the note that used to stand here was the
     * record of it: "never reading `yMin`, `yMax`, `yTickCount` or `yAxisTicks` — a stated
     * divergence rather than a gap". That reading was already half-retired when it was written —
     * #418 had just carved `unpinFromZero` into this function as an exception — and MEASURING IT
     * ENDED IT. The Axes tab was offering "Y show lines and tick marks" and "Number of tick marks"
     * on this very chart: driven live on `MB · Scatter` with an X metric picked, the tick-count box
     * read `3` while the rendered `yAxis.splitNumber` stayed at ECharts' default `5`.
     *
     * So it was never a divergence with the controls withheld to match. It was three live controls
     * (the x one above included) saving and not painting, which is the family of defect this
     * programme keeps closing. A DIVERGENCE IS ONLY HONEST WHILE THE PANEL KEEPS ITS HALF OF IT, and
     * this one had stopped. Fixing it in the builder rather than withdrawing the rows also settles
     * the question the other way round: a scatter with an X metric and one without are the same
     * chart type to the reader, and controls that vanish when a neighbouring select changes read as
     * a glitch rather than as a rule — the argument this file already makes about the x-scale
     * rotations.
     *
     * WHAT IS BORROWED FROM `mkValueAxis` AND WHAT IS NOT. The five keys are read exactly as it
     * reads them, spelling included, so the two paths cannot drift into meaning different things by
     * the same name. What is NOT borrowed is its measured `interval` fallback: that comes from the
     * category/vertical fits, which need a category axis to measure against and this chart has
     * none. With no tick count stored, nothing is written and ECharts picks — which is what this
     * axis has always done.
     *
     * `scale` STAYS WRITTEN UNCONDITIONALLY, unlike `mkValueAxis`, and the asymmetry is still the
     * point: there the default is pinned and matches ECharts', so an absence says it; here the
     * default is UNPINNED (`getYAxisUnpinFromZeroDefault` is `display === "scatter"`,
     * `cartesian-chart.ts:242`), so an absence would say the opposite of what was decided and a
     * deliberate pin could not be expressed at all. `unpinFromZeroOn` already returns false once a
     * bound is set, so an explicit range outranks the unpin here exactly as it does on every other
     * kind, and the panel hides the switch in the same state.
     *
     * X IS LEFT PINNED, unchanged: `graph.y_axis.unpin_from_zero` has no x-axis twin upstream.
     */
    yAxis: {
      type: cfg.logScale ? ("log" as const) : ("value" as const),
      scale: unpinFromZeroOn(cfg),
      name: yTitle,
      nameTextStyle: { color: axisLabelColor },
      axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: axisFmt(yMetric, widestOfMetric(1)) },
      axisLine: { show: cfg.yAxisTicks !== false, lineStyle: { color: gridLine } },
      axisTick: { show: cfg.yAxisTicks !== false },
      ...(cfg.yMin != null ? { min: cfg.yMin } : {}),
      ...(cfg.yMax != null ? { max: cfg.yMax } : {}),
      ...(cfg.yTickCount != null ? { splitNumber: cfg.yTickCount } : {}),
      splitLine: { lineStyle: { color: gridLine } },
    },
    series,
  };
}

export function buildCartesianOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg0: CartesianBuildConfig,
  /** The chart's measured pixel size, when the caller has one — this builder is pure data with no
   * DOM of its own, so EChartsRenderer measures the container and threads it through (see
   * `buildPieOption`, the first to take one).
   *
   * Read by the x-axis label fit (`baseOption` -> `fitXLabels`): whether the category labels fit
   * is a question about WIDTH, which this builder could not see until this parameter existed —
   * the rule it replaced guessed from label count and length (`longest > 10 &&
   * categories.length > 6`), which are proxies for width, not width.
   *
   * Undefined means the caller has no measurement (every unit test, `cartesianMeta`, and any
   * caller passing a plain option object rather than a size function); that keeps the size-blind
   * behaviour, since a decision made against a guessed width is worse than one that declines to
   * guess. `__tests__/cartesianXLabels.test.ts` pins both halves. */
  size?: EChartsSize,
): EChartsCoreOption {
  if (cfg0.kind === "scatter" && cfg0.xMetric && cfg0.yMetrics.length > 0) {
    return scatterXY(columns, rows, cfg0);
  }
  // Resolve the x-scale ONCE, here, on the same principle as the axis split below it: every
  // downstream reader — `usesXScale`, `applyXScale`, `foldable`, `ordinalX`, `scaleAxis` — goes on
  // asking `cfg.xScale` exactly as it always did, and a computed scale is indistinguishable from
  // one the user chose. Read off the RESULT column rather than `cfg.xAxisType`, which is the same
  // type by a longer route (`ChartRenderer` looks it up in this very list) but is optional and
  // absent from every caller that builds a config by hand.
  //
  // Below the `scatterXY` early return, because above it would be wasted: a scatter with an
  // `xMetric` AND at least one y-metric leaves through that return and builds its own pair of value
  // axes, so the common metric-vs-metric case never reads this. Not "unreachable" — an `xMetric`
  // scatter with NO y-metric fails that return's second condition and arrives here, where it has no
  // series to draw anyway and leaves through the empty-option branch a few lines down. What this
  // line is really FOR is the scatter whose x is a DIMENSION: the one that was drawing 196
  // categories.
  const xColumn = cfg0.xAxis != null ? columns.find((c) => c.name === cfg0.xAxis) : undefined;
  const xScale = resolveXScale(cfg0.kind, cfg0.xScale, xColumn?.type);
  const cfgScale: CartesianBuildConfig = xScale === cfg0.xScale ? cfg0 : { ...cfg0, xScale };
  /**
   * SPLIT PANELS' MUTUAL EXCLUSIONS, resolved here so the whole builder downstream sees a config
   * that already agrees with itself.
   *
   * Metabase enforces all three, and it enforces each of them TWICE — the control is hidden
   * (`getHidden` returns true under `graph.split_panels`) AND the value is overridden
   * (`stackable.stack_type`'s `getDefault` returns null, `series.axis`'s returns "left"). A recon
   * that wrote the settings straight into `visualization_settings` saw only the second half and
   * concluded these were silent overrides rather than hidden controls; the source
   * (`lib/settings/graph.ts:340-370`, `lib/settings/series.ts:205-212`, `graph.ts:888`) says both.
   * egret does both too — `CartesianDisplaySection` hides the same three rows — because either one
   * alone leaves a way for the reader to reach a state the chart will not draw.
   *
   *   - STACKING. A stack is several series measured from ONE baseline; a panel holds one series,
   *     so there is nothing to stack it against.
   *   - AN EXPLICIT RIGHT-HAND AXIS. Two axes on one plot is the OTHER answer to "these two series
   *     have different scales", and a panel is this one; running both leaves a chart with panels
   *     whose axes are on different edges.
   *   - THE AUTOMATIC SPLIT, which is the same answer arrived at without being asked. It is cleared
   *     BEFORE the `shouldAutoSplitY` block below rather than after, so the split is never computed
   *     — clearing `rightAxisMetrics` afterwards would work too, but it would leave a live call
   *     whose result is thrown away, which is how a later reader restores the bug by "simplifying".
   *
   * `normalized` AND `showStackTotal` ARE DELIBERATELY NOT RESTATED HERE, and an earlier draft of
   * this line did restate them. The mutation run is what caught it: removing either one changed
   * nothing any test could see, because BOTH already hang off stacking —
   * `isNormalized` is `!!cfg.normalized && resolveStacked(cfg)` and the stack total is written under
   * `if (resolveStacked(cfg) && cfg.showStackTotal …)`. `stacked: false` therefore switches off all
   * three, and writing the other two would have been two inert assignments dressed as safety. What
   * keeps that honest is a test rather than this paragraph: `cartesian.splitPanels.test.ts` asserts
   * a config carrying `normalized: true` gets no 0-100 axis, so if `isNormalized` is ever loosened
   * to read the flag alone, it fails here rather than showing up as flat panels.
   *
   * Read off `splitPanels` and `splitPanelsApplies` only — NOT off whether the layout will succeed.
   * Whether the card is tall enough is decided in `baseOption`, after the series exist and a count
   * is known, and a chart that falls back there falls back to a chart with its stacking still off
   * and its axes still merged. That is the same answer in both directions and the alternative is
   * worse: a stack, or a second y-axis, that appears and disappears as a dashboard card is resized.
   */
  const cfgX: CartesianBuildConfig = cfgScale.splitPanels && splitPanelsApplies(cfgScale)
    ? { ...cfgScale, stacked: false, rightAxisMetrics: [], autoSplitY: false }
    : cfgScale;
  // Resolve the automatic axis split ONCE, here, and hand the rest of the builder a config that
  // already names the right-axis metrics. Every downstream reader — `isDualAxis`, the per-series
  // `yAxisIndex`, the axis titles — then goes on asking the same question it always did, and a
  // computed split is indistinguishable from one the user assigned by hand.
  const cfg: CartesianBuildConfig = (cfgX.rightAxisMetrics ?? []).length > 0
    ? cfgX
    : (() => {
        const extents = metricExtents(columns, rows, cfgX.yMetrics);
        if (!shouldAutoSplitY(cfgX, extents)) return cfgX;
        const right = computeAxisSplit(cfgX.yMetrics, extents);
        // A split that puts everything on one side is not a split.
        return right.length > 0 && right.length < cfgX.yMetrics.length
          ? { ...cfgX, rightAxisMetrics: right }
          : cfgX;
      })();
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const xi = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
  const fmt = (metric: string, v: unknown) =>
    formatValue(v, cfg.metricTypes?.[metric] ?? "double", cfg.columnSettings?.[metric]);

  if (xi == null || cfg.yMetrics.length === 0) {
    return baseOption(cfg, [], [], size);
  }

  // Drop rows with a null x value, then order categories by the x column's type.
  const xType = columns[xi].type;
  const xrows = rows.filter((r) => r[xi] != null);
  const catKey = (v: unknown) => String(v);
  let categories = sortCategories(distinct(xrows.map((r) => r[xi])), xType, cfg.preserveCategoryOrder);


  // Breakout: one ECharts series per distinct series value (first y-metric only).
  if (cfg.series != null && idx[cfg.series] != null) {
    const gi = idx[cfg.series];
    const metric = cfg.yMetrics[0];
    const mi = idx[metric];
    // ORDERED, not arrival order — see `sortBands` for what arrival order was measured doing and
    // for why this sits here rather than beside `applySeriesOrder` below. The column's own type
    // comes from the RESULT rather than from `cfg.seriesType`: they are the same string (
    // `ChartRenderer` fills that field by looking this very column up) and reading the result means
    // the builder cannot be handed rows and a type that disagree.
    const seriesValues = sortBands(distinct(xrows.map((r) => r[gi])), columns[gi].type, cfg.preserveSeriesOrder);
    const cell = new Map(xrows.map((r) => [`${catKey(r[xi])} ${catKey(r[gi])}`, r[mi] as number | null]));
    // Metabase parity: null / empty-string breakout values render as "(empty)".
    //
    // HOISTED ABOVE THE BAR COUNT, and that is the fix rather than a tidy-up. The count below used
    // to pass the series name as `""` — a hard-coded empty string, because `groupName` was not yet
    // in scope here — so `cfg.seriesTypes[""]` matched nothing and every band on a combo fell back
    // to `seriesDisplayFor`'s POSITIONAL default (`index === 0 ? "line" : "bar"`), while the
    // display each band is actually drawn with was resolved from `groupName(sv)` fifteen lines
    // further down. Measured on a two-band combo with both bands set to `bar`: the count came out
    // 1, `grouped` came out false, and both series were then pinned at `barWidth: "80%"` at the
    // same x — ECharts drew them on top of one another with only the last one visible.
    //
    // It has to be `groupName(sv)` and not `String(sv)`: `seriesTypes` is keyed by the name the
    // PANEL shows, which for a null or empty band is `(empty)`. `String(null)` is `"null"`, a key
    // nothing can ever write, and the lookup would silently miss again — for the same reason
    // `decorateSeries` and `colorsForKeys` are both keyed on this function's output.
    const groupName = (sv: unknown) => (sv == null || sv === "" ? "(empty)" : String(sv));
    // How many of these will be drawn as bars decides whether bars must sit side by side. With a
    // per-series display the answer is no longer one lookup — a combo can mix them. Grouped bars
    // (>1 series, not stacked) must auto-size to sit side by side; a stack shares an x on purpose,
    // so it is never grouped.
    const barCount = seriesValues.filter(
      (sv, si) => seriesDisplayFor(cfg.kind, groupName(sv), si, cfg.seriesTypes) === "bar",
    ).length;
    const grouped = !resolveStacked(cfg) && barCount > 1;
    // Keyed by GROUP NAME so a breakout value keeps its colour across every card that breaks out by
    // it — a `Gadget` band is the same yellow whether the card also plots `Widget` or not, and
    // whether it is drawn second or fourth. An explicit `seriesColors` entry is folded in rather
    // than layered on top, so it CONSUMES its colour and the series that would have hashed onto it
    // moves along instead of being drawn identically.
    const groupColors = colorsForKeys(seriesValues.map(groupName), cfg.seriesColors);
    // Which BANDS go on the right value axis, decided once for the whole set for the same reason
    // the colours are: "one side holds everything" is a property of the set, not of any one band.
    // Keyed by group name, so it lands in the same key space as the colour, the rename and the
    // display — the panel row that offers all four addresses them with one string.
    const onRight = new Set(rightAxisSeries(cfg, seriesValues.map(groupName)));
    const builtSeries = seriesValues.map((sv, si) => {
      const data = categories.map((c) => {
        const v = cell.get(`${catKey(c)} ${catKey(sv)}`);
        return v === undefined ? null : v;
      });
      return decorateSeries({ name: groupName(sv), data }, metric, si, cfg, fmt, groupColors, grouped, onRight);
    });
    const series = applySeriesOrder(builtSeries as unknown as SeriesLike[], cfg) as unknown as typeof builtSeries;
    if (isNormalized(cfg)) normalizeStacks(series as unknown as { data: (number | null)[] }[]);
    if (cfg.trendLine) series.push(...(trendSeries(series as unknown as SeriesLike[]) as unknown as typeof series));
    if (resolveStacked(cfg) && cfg.showStackTotal && !isNormalized(cfg)) series.push(stackTotalSeries(series as unknown as SeriesLike[], cfg) as unknown as (typeof series)[number]);
    if (usesXScale(cfg)) applyXScale(categories, series, cfg);
    applyMarkers(series as unknown as SeriesLike[]);
    const displayCats = isExtractUnit(cfg.xAxisUnit) ? categories.map((v) => temporalLabel(cfg.xAxisUnit, v)) : categories;
    return baseOption(cfg, displayCats as (string | number)[], series, size);
  }

  const rowByCat = new Map(xrows.map((r) => [catKey(r[xi]), r]));
  const barMetrics = cfg.yMetrics.filter((m, si) => seriesDisplayFor(cfg.kind, m, si, cfg.seriesTypes) === "bar").length;
  const grouped = !resolveStacked(cfg) && barMetrics > 1;
  // The comment this replaces said Metabase tints a lone series with its summarize green. Measured
  // on identical data it does not: a single-metric bar card draws #f9d45c, the colour "revenue"
  // hashes to. The green is Metabase's `success`, which `colorsForKeys` still returns — for a series
  // actually NAMED "profit" or "completed", which is the rule that was mistaken for a lone-series
  // one.
  //
  // A DUPLICATE metric would now draw both series in one colour, where the positional scheme gave
  // them separate slots — `colorsForKeys` returns a Record, so one name is one entry. Not reachable
  // from the picker (`FieldZone` offers only metrics not already selected, and its chips are keyed
  // by name), but chart settings are stored JSON and are not whitelisted server-side, so a
  // hand-written config can hold one. Left as is rather than guarded: two series that ARE the same
  // metric being drawn the same is a fair reading of that config, and the old scheme already
  // collapsed them the moment either had a `seriesColors` entry, which is keyed by name too.
  const metricColors = colorsForKeys(cfg.yMetrics, cfg.seriesColors);
  // Without a breakout a series IS a metric, so this is the per-metric split egret has always had,
  // reached through the function the breakout branch also uses rather than through a second one.
  const onRight = new Set(rightAxisSeries(cfg, cfg.yMetrics));
  let series = cfg.yMetrics.map((metric, si) => {
    const mi = idx[metric];
    const data = categories.map((c) => {
      const r = rowByCat.get(catKey(c));
      return r == null ? null : (r[mi] as number | null);
    });
    return decorateSeries({ name: metric, data }, metric, si, cfg, fmt, metricColors, grouped, onRight);
  });

  // Folding is offered only where "Other" means one clear thing: a single metric, no
  // breakout, and a plain category axis. With a breakout the bucket would have to merge
  // series as well, which changes what the chart says; on a value or time axis "Other" has
  // no coordinate to sit at; and a temporal extract axis is already a fixed small domain
  // (twelve months, seven weekdays) that nobody needs folded. And a trend line fitted
  // through an "Other" bucket would describe where the user set the cutoff rather than the
  // data, since the bucket's size moves with it. Those are separate decisions, taken when
  // the charts that need them are redesigned.
  let foldedCategory: string | undefined;
  const foldable =
    cfg.maxCategories != null &&
    !cfg.series &&
    cfg.yMetrics.length === 1 &&
    !usesXScale(cfg) &&
    !isExtractUnit(cfg.xAxisUnit) &&
    !cfg.trendLine;
  if (foldable) {
    const s = series[0] as unknown as SeriesLike;
    const folded = foldTail(
      categories.map((c, i) => ({ name: String(c), value: s.data?.[i] })),
      cfg.maxCategories,
    );
    // A fold actually happened only if the list got shorter. Recorded on the option so the drill
    // can refuse that one bar: "Other" here is a bucket the chart invented, and `= 'Other'` either
    // matches nothing or — because foldTail merges into a REAL category of that name when one
    // exists — matches a fraction of the bar and understates it. Both are wrong silently.
    if (folded.length < categories.length) foldedCategory = OTHER_LABEL;
    categories = folded.map((p) => p.name);
    s.data = folded.map((p) => (typeof p.value === "number" ? p.value : null));
  }

  series = applySeriesOrder(series as unknown as SeriesLike[], cfg) as unknown as typeof series;

  if (isNormalized(cfg)) normalizeStacks(series as unknown as { data: (number | null)[] }[]);
  if (cfg.trendLine) series.push(...(trendSeries(series as unknown as SeriesLike[]) as unknown as typeof series));
  if (resolveStacked(cfg) && cfg.showStackTotal && !isNormalized(cfg)) series.push(stackTotalSeries(series as unknown as SeriesLike[], cfg) as unknown as (typeof series)[number]);
  if (usesXScale(cfg)) applyXScale(categories, series, cfg);
  applyMarkers(series as unknown as SeriesLike[]);
  const displayCats = isExtractUnit(cfg.xAxisUnit) ? categories.map((v) => temporalLabel(cfg.xAxisUnit, v)) : categories;
  // `__foldedCategory` is not an ECharts key — ECharts ignores what it doesn't know. It rides on
  // the option because the drill needs to recognise the one bar it must refuse, and the option is
  // the only thing both the builder and the click handler hold.
  return {
    ...baseOption(
      cfg, displayCats as (string | number)[], series, size,
      tooltipExtrasByCategory(cfg, idx, rowByCat, categories, displayCats),
    ),
    __foldedCategory: foldedCategory,
  };
}

/**
 * The extra tooltip rows for every category on the chart, keyed by the string the FORMATTER will be
 * handed for that datum.
 *
 * KEYED BY CATEGORY AND NOT BY `dataIndex`, which is the one decision in here worth defending.
 * `applyXScale` FILTERS — a datum whose x will not parse is dropped — so on a value or time x-axis
 * the series' data array stops being index-aligned with the category list. `ariaPoints` carries the
 * scar of exactly that mistake, and the note beside it says why it survived so long: a positional
 * zip "does not read as NaN — it reads as a correct-looking sentence". A category lookup cannot
 * slide, because both shapes a datum takes carry the category and both surface as `params.name`:
 * ECharts reports the `xAxis.data` entry on an ordinal axis, and `applyXScale` writes the raw value
 * into `{value: [x, y], name}` on the other two.
 *
 * BOTH SPELLINGS GO IN. A temporal EXTRACTION unit substitutes the categories themselves
 * (`displayCats`), so the axis says "Wed" where the row holds `3`; registering the display string
 * beside the raw one costs one Map entry per category and means neither reader has to know which
 * kind of axis it is looking at. They are the same array — the same object, even — on every chart
 * that has no extraction unit, and `Map.set` twice with one key is a no-op.
 *
 * A FOLDED "Other" GETS NOTHING, deliberately. `foldTail` SUMS the tail into one bucket, which is a
 * legitimate thing to do to a count and a wrong thing to do to an average or a ratio — and this
 * function has only the fetched per-category values to work with, not the metric's own aggregate.
 * "Other" is a bucket the chart invented (`__foldedCategory` exists so the drill can refuse it for
 * the same reason), so it says nothing rather than something arithmetically false.
 */
function tooltipExtrasByCategory(
  cfg: CartesianBuildConfig,
  idx: Record<string, number>,
  rowByCat: Map<string, unknown[]>,
  categories: unknown[],
  displayCats: unknown[],
): Map<string, TooltipExtra[]> | undefined {
  const names = extraTooltipMetrics(cfg, idx);
  if (names.length === 0) return undefined;
  const out = new Map<string, TooltipExtra[]>();
  categories.forEach((c, i) => {
    const r = rowByCat.get(String(c));
    if (r == null) return;
    const extras = names.map((m) => ({ metric: m, value: r[idx[m]] }));
    out.set(String(c), extras);
    out.set(String(displayCats[i]), extras);
  });
  return out;
}

// Shared decoration: type, color, stack, areaStyle, labels, smooth, symbol.
function decorateSeries(
  s: { name: string; data: (number | null)[] },
  metric: string,
  colorIdx: number,
  cfg: CartesianBuildConfig,
  fmt: (metric: string, v: unknown) => string,
  /** The colour of every series this chart will draw, keyed by its RAW name — built once by the
   *  caller, because the assignment is a decision about the whole set (a hash collision moves one
   *  key along, and a pinned colour takes its slot out of circulation) and cannot be made one
   *  series at a time. Both call paths pass a complete mapping, so there is no `??` fallback here:
   *  a positional one would be a second, silent colour rule that no test could see. */
  colors: Record<string, string>,
  grouped = false,
  /** The series on the RIGHT value axis, by raw name — `rightAxisSeries`' answer for this chart's
   *  whole set. Passed in for the same reason `colors` is: whether a right axis exists at all
   *  depends on how the OTHER series were assigned, so it cannot be decided one series at a time.
   *  Keyed by `s.name` and not by `metric`, which is what a breakout has only one of. */
  onRight: ReadonlySet<string> = new Set(),
) {
  const display = seriesDisplayFor(cfg.kind, s.name ?? metric, colorIdx, cfg.seriesTypes);
  const type = echartsTypeOf(display);
  // `cfg.seriesColors` is NOT consulted again here. The `?? cfg.seriesColors?.[s.name]` this
  // replaces was redundant rather than wrong — `colorsForKeys` has already folded the user's pinned
  // colours in, so re-reading them changes no pixel today (measured: restoring the chain leaves the
  // whole chart suite green). It goes because it reads as a SECOND authority on colour, and the next
  // person to reach for it — to add a case the mapping does not cover — gets a series that is pinned
  // here but still in circulation there, and two series drawn in the same ink.
  const color = colors[s.name];
  const out: Record<string, unknown> = {
    ...s,
    // Display label, in falling order of authority: the user's explicit rename, then the series
    // dimension's own column format, then the raw value. The FORMAT step is what made a breakout
    // by month read "2026-01-01" in the legend while the panel offered a date style for that very
    // column — offered, stored, and ignored.
    name: cfg.seriesLabels?.[s.name] ?? seriesDisplayName(cfg, s.name),
    // The RAW series value, kept because `name` above is what the reader sees and the drill needs
    // what the column holds. Filtering by the display label found nothing the moment a series was
    // renamed — the rename silently broke the click.
    __raw: s.name,
    __metric: metric,                            // keep the source metric for tooltip formatting
    type,
    itemStyle: { color },
  };
  // Only pin the bar width for a single bar per category; grouped bars must let
  // ECharts auto-size and place them side by side (otherwise they overlap).
  if (type === "bar") { if (!grouped) out.barWidth = CHART_THEME.barWidth; out.barMinHeight = BAR_MIN_HEIGHT; }
  // Filled either because the whole chart is an area, or because this one series is displayed as
  // one — a combo can hold both a filled band and a bare line.
  const filled = cfg.kind === "area" || display === "area";
  if (type === "bar" || filled) {
    if (resolveStacked(cfg)) out.stack = "total";
  }
  if (filled) out.areaStyle = { opacity: CHART_THEME.areaOpacity };

  // Pointing at a series lifts it and pushes every other one back — the effect that makes a
  // multi-series chart readable, and the one thing egret's cartesian charts had no answer for.
  // Without `focus`, ECharts applies its default emphasis to whatever is under the pointer and
  // leaves the rest alone: measured on egret, hovering one band lightened ALL FOUR, which says
  // nothing about which one you are on.
  //
  // The numbers are Metabase's, measured on a live instance rather than read off its source:
  //   bar  — hovered stays opaque, the other series drop to 0.3
  //   area — hovered fill goes 0.3 -> 0.6, the other fills 0.3 -> 0.2
  // The trend and stack-total helper series are excluded by their callers, which never reach here.
  out.emphasis = {
    focus: "series",
    ...(filled ? { areaStyle: { opacity: CHART_THEME.areaOpacityEmphasis } } : {}),
  };
  out.blur = {
    itemStyle: { opacity: CHART_THEME.blurOpacity },
    lineStyle: { opacity: CHART_THEME.blurOpacity },
    ...(filled ? { areaStyle: { opacity: CHART_THEME.areaOpacityBlur } } : {}),
  };
  if (type === "line") {
    // A series may depart from the chart-wide line appearance. Metabase keeps these settings
    // per-series only; egret keeps a chart-wide default and lets a series override it, so the
    // common case stays one control instead of one per line.
    const ln = cfg.seriesLine?.[s.name] ?? {};
    const shape = ln.shape ?? cfg.lineShape ?? "linear";
    out.smooth = shape === "smooth";
    if (shape === "step") out.step = "middle";   // egret's existing step mode, not Metabase's step-after
    // Resolved here, applied at the end of buildCartesianOption: folding and x-scaling both
    // rewrite the data afterwards, and "auto" is a question about the points that actually get
    // drawn.
    out.__markers = ln.markers ?? cfg.markers ?? "auto";
    // PER SERIES, over a chart-wide default — the same one-link-longer chain the four fields above
    // take, and Metabase's own arrangement: `line.missing` lives in its SERIES settings and its
    // chart-wide twin is marked `// DEPRECATED: moved to series settings` (`lib/settings/graph.ts`,
    // `LINE_SETTINGS`).
    //
    // THIS LINE CARRIED A CAREFUL ARGUMENT FOR THE OPPOSITE, AND MEASURING IT ENDED IT. The
    // argument was that the other four fields are APPEARANCE — a dashed line and a solid line are
    // the same number drawn differently — whereas `missing: "zero"` REWRITES THE DATA (see below),
    // "and that zero then counts in stack totals, in normalisation and in a trend fit", so two
    // bands disagreeing would make the chart say two things at once with nothing on screen to
    // signal it. Two of those three consequences do not exist. Measured on `v = [1, null, 3, 4]`
    // beside `w = [10, 20, 30, 40]`, chart-wide, before anything changed:
    //
    //   stack total    interpolate [11,20,33,44]  zero [11,20,33,44]  none [11,20,33,44]
    //   normalised w   [90.9,100,90.9,90.9]       …identical…         …identical…
    //   trend of v     [1,2,3,4]                  [0.2,1.4,2.6,3.8]   [1,2,3,4]
    //
    // The stack total does not move because `stackTotalSeries` sums with `typeof v === "number"`:
    // a null contributes nothing and a zero contributes zero, which is the same number.
    // `normalizeStacks` totals the same way, so neither the total nor any OTHER band's percentage
    // moves — only the zeroed band's own point, which draws at 0% instead of leaving a gap, which
    // is what the reader asked for. The trend genuinely does move, and it is the one real
    // consequence; but `trendSeries` fits each series independently off its own `data`, so it
    // moves that band's OWN trend line and nothing else. There is no cross-series contamination
    // left to protect anyone from, and `cartesian.perSeriesMissing` pins all three numbers so this
    // conclusion cannot rot the way its predecessor did.
    //
    // The one residue, recorded rather than hidden: in a category where EVERY band is null, a
    // zeroed band flips `stackTotalSeries`' `any` flag, so the total prints "0" where it would
    // otherwise print nothing. That is the chart-wide setting's existing behaviour reached one
    // band at a time, not something per-series introduces.
    //
    // A log axis has no zero, so filling with one cannot mean what it says — ECharts would drop
    // the point and break the line at exactly the place the user asked to have filled. Leaving
    // the gap is the honest version of "this value cannot be shown here". Asked of the RESOLVED
    // value, so a per-series "zero" cannot slip past a guard written for the chart-wide one.
    const chosen = ln.missing ?? cfg.missing ?? "interpolate";
    const missing = cfg.logScale && chosen === "zero" ? "none" : chosen;
    out.connectNulls = missing === "interpolate";
    if (missing === "zero") {
      // ECharts can bridge a null or break at it, but it has no notion of treating one as
      // zero — that has to be the data, not an option. Safe to rewrite here as plain numbers:
      // applyXScale (which re-keys onto [x, y] pairs) runs after decorateSeries, never before.
      // This is not a rendering nicety: the zero becomes a real data point downstream too — it
      // counts in stack totals, in normalisation, and in a trend fit — because `missing: "zero"`
      // means the user is asserting what the absent value WAS, not just how to draw it.
      out.data = (s.data ?? []).map((v) => (v == null ? 0 : v));
    }
    // Cycle the dash style + marker shape per series so lines are distinguishable
    // without relying on colour alone (a11y: colour-blind users). The dash cycle is an
    // accessibility feature — it lets lines be told apart without colour — so a per-series
    // style REPLACES it for that one series only and leaves the rest cycling.
    // Settings JSON is not whitelisted server-side, so `ln.size` may be a value the type
    // never promised (e.g. a stored "XL"). `in` guards the lookup so an unknown size falls
    // back to the theme width instead of `LINE_WIDTHS[unknown]` silently overwriting
    // ECharts' default width with `undefined`.
    out.lineStyle = {
      width: ln.size && ln.size in LINE_WIDTHS ? LINE_WIDTHS[ln.size] : CHART_THEME.lineWidth,
      type: ln.style ?? lineType(colorIdx),
    };
    out.symbol = symbolShape(colorIdx);
    out.symbolSize = CHART_THEME.symbolSize;
  }
  // Scatter series get a distinct marker shape per series (shape, not colour).
  if (type === "scatter") { out.symbol = symbolShape(colorIdx); out.symbolSize = CHART_THEME.symbolSize; out.itemStyle = { color, opacity: CHART_THEME.scatterOpacity }; }
  // BY SERIES NAME, not by metric. The metric is what every band of a breakout shares, so asking
  // about it gave one answer for the whole chart — the defect this line used to be.
  if (onRight.has(s.name)) out.yAxisIndex = 1;
  if (cfg.showLabels) {
    out.label = {
      show: true, fontSize: CHART_THEME.labelFontSize, fontWeight: CHART_THEME.labelWeight,
      position: isHorizontal(cfg) ? "right" : "top",
      // On a value/time x-axis the datum is an [x, y] pair — label the y.
      formatter: (p: { value: unknown }) => fmt(metric, Array.isArray(p.value) ? p.value[1] : p.value),
    };
    // "fit" is the default: at a dashboard card's width there is no room for a label per
    // bar, and a pile of overlapping numbers is worse than showing fewer. "all" is for
    // someone who would rather have every number and accept the crowding.
    if (cfg.labelDensity !== "all") out.labelLayout = { hideOverlap: true };
  }
  return out;
}

/**
 * A LEGEND NEEDS 100px PER ENTRY, AND A CARD UNDER 400px THAT CANNOT GIVE IT THAT GETS NO LEGEND.
 *
 * Metabase's rule, copied from `visualizations/components/legend/LegendLayout.tsx` rather than
 * invented here — the whole of it is four lines:
 *
 *     const maxXItems  = Math.floor(width / MIN_ITEM_WIDTH);   // MIN_ITEM_WIDTH   = 100
 *     const isNarrow   = width < MIN_LEGEND_WIDTH;             // MIN_LEGEND_WIDTH = 400
 *     const isVertical = maxXItems < items.length;
 *     const isVisible  = hasLegend && !(isVertical && isNarrow);
 *
 * Past 400px Metabase turns the legend VERTICAL and puts it beside the plot, which is why the
 * `isNarrow` half exists at all; egret's legend is an ECharts component along the top and has no
 * vertical mode, so what it takes from the rule is the refusal and not the reflow. Below 400px the
 * two products then agree exactly, which is the range every measurement behind this was taken in.
 *
 * WHAT IT REPLACES. Measured at 209px on all three breakout cards in both themes: four entries were
 * laid out across ~307px inside a 193px plot, and ECharts' scroll legend clips MID-ITEM — so the
 * card drew one named entry, one coloured chip with no name beside it, two entries gone entirely,
 * and a "1/4" pager offering four pages for four series. Verified against Metabase at :3033 with
 * the same four-band card at 170, 214, 258 and 345px: its series legend is absent at every one of
 * them, and the only "legend" nodes in its DOM are the card's own title caption.
 *
 * That is the better answer, and not merely the compatible one. The pager was never the defect —
 * two of four series unnamed and two invisible is not a legend at any pager position — and the
 * 24px band it costs at the top of a 262px card is worth more to the plot than to a control the
 * reader cannot read. The series are still named on hover, which is where a narrow card's reader
 * was already getting them.
 *
 * AN UNMEASURED CARD KEEPS ITS LEGEND. `size` absent means "not laid out yet" — a hidden container,
 * a first paint, jsdom — never "tiny", which is the same reading `measurable` and the funnel's
 * `useMeasuredSize` already take. Refusing here would strip the legend from every chart in the
 * test suite and from the first frame of every chart in the product.
 */
export function legendFitsAcross(size: EChartsSize | undefined, items: number): boolean {
  if (size == null || size.width <= 0) return true;
  const card = size.width + CARD_INSET_PX;
  if (card >= LEGEND_MIN_WIDTH) return true;
  return items <= Math.floor(card / LEGEND_MIN_ITEM_WIDTH);
}
/** Metabase's `MIN_ITEM_WIDTH` — the width one legend entry is assumed to need. */
const LEGEND_MIN_ITEM_WIDTH = 100;
/** Metabase's `MIN_LEGEND_WIDTH` — above this it reflows rather than refusing, so egret stops asking. */
const LEGEND_MIN_WIDTH = 400;
/**
 * THE CARD'S PADDING, added back because Metabase asks this question of the CARD and egret only
 * measures the plot inside it.
 *
 * `EChartsRenderer` observes the ECharts HOST, which sits inside the dashboard card's padding:
 * measured across eight cards on the parity dashboard, every one of them is a 209px card with a
 * 193px host — a flat 16px, 8px a side. Metabase's `LegendLayout` is handed its visualization's own
 * width, which is the card's content box (measured: a 214px card gives it 212, a 170px card 168).
 *
 * WITHOUT THIS TERM THE RULE MISCLASSIFIES egret's most common card, and its own two measurements
 * say so. On a 209px card the four-band breakout legend is unusable (four entries laid out across
 * ~307px, clipped mid-item) and the two-metric combo legend is fine (`orders` and `revenue` occupy
 * ~128px of the 193 available, no pager). `floor(193/100) = 1` is below both counts and hides both
 * — it would have taken a working legend off "MB · Combo", which this sweep found clean. `floor(209
 * / 100) = 2` separates them exactly, and agrees with Metabase's own boundary: it draws this same
 * two-series legend at a 214px card and drops it at 170px.
 *
 * So the term is not a fudge toward a desired answer; it is the difference between measuring the
 * plot and measuring the card, and the card is what the rule was written about. A chart outside a
 * dashboard card has less chrome than this and is over-credited by up to 16px, which can only ever
 * admit one extra entry at a boundary — the harmless direction, and the same direction every other
 * "declined to refuse" default in this builder errs in.
 */
const CARD_INSET_PX = 16;

function baseOption(
  cfg: CartesianBuildConfig,
  categories: (string | number)[],
  series: unknown[],
  /** Absent when the caller has no measurement; see the x-label fit below. */
  size?: EChartsSize,
  /** The additional tooltip rows for each category, by the key `params.name` will carry — built by
   *  `tooltipExtrasByCategory`, which is where the keying is argued. Absent on every chart that has
   *  not asked for any, and on the breakout branch, which cannot answer the question. */
  tooltipExtras?: Map<string, TooltipExtra[]>,
): EChartsCoreOption {
  const horizontal = isHorizontal(cfg);
  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const normalized = isNormalized(cfg);
  // Defaults are decided here and never stored, so a chart saved before this change picks
  // them up too. `??` keeps an explicit choice winning — including `false` and `""`.
  const hasBreakout = cfg.series != null && cfg.series !== "";
  const showLegend = cfg.showLegend ?? (hasBreakout || cfg.yMetrics.length > 1);
  // A renamed series is what the legend and tooltip already show, so the axis follows it
  // first; `column_title` is the per-column rename the Formatting panel will write. Lifted to
  // `displayTitle` so `scatterXY`, which builds its own axes, asks the same question rather than
  // spelling the precedence out a second time.
  const titleOf = (col: string | undefined) => displayTitle(cfg, col);
  // An axis plotting several metrics has no one name — the legend names them instead.
  const metricTitle = (metrics: string[]) => (metrics.length === 1 ? titleOf(metrics[0]) : undefined);
  const xTitle = cfg.showXLabel === false ? undefined : (cfg.xLabel ?? titleOf(cfg.xAxis));

  // Compact labels are for wide numbers, not for many bars — three bars of revenue in the
  // millions crowd an axis just as badly as forty do. Measured per axis: a dual-axis chart
  // has two scales on purpose, and the small one should not inherit the big one's shorthand.
  const plotted = (series as SeriesLike[]).filter((s) => !s.__trend && !s.__total);

  // Axis shape follows the series that survived hiding, not the config: hiding the only
  // right-axis metric should take its axis with it. Keyed on yAxisIndex rather than on the
  // metric names because that index was baked into each series before hiding ran — an axis
  // a surviving series still points at has to exist. The Set matters: a breakout splits one
  // metric across many series, so without deduping, leftSide would list it once per series
  // and metricTitle (single-metric only) would stop titling breakout charts' axis.
  const dual = plotted.some((s) => s.yAxisIndex === 1);
  /**
   * The plotted series on one value axis.
   *
   * EVERYTHING BELOW THAT USED TO SELECT BY `__metric` SELECTS THROUGH THIS NOW, and under a
   * breakout that is the difference between a working split and a decorative one: every band of a
   * breakout carries the SAME `__metric`, so a metric-name filter answers "yes" for both sides and
   * each axis would be sized over all the data. Measured: `small` [1,2] and `big` [1000,4000] split
   * across two axes came out with both axes spanning 0–4000. The assignment lives on the series, so
   * the question is asked of the series.
   */
  const onAxis = (side: 0 | 1) => plotted.filter((s) => (s.yAxisIndex === 1 ? 1 : 0) === side);
  /** …and the METRICS behind a set of series, for the readers that format or title by column. A Set
   *  because a breakout splits one metric across many bands. */
  const metricsOf = (list: SeriesLike[]) =>
    [...new Set(list.map((s) => s.__metric ?? s.name ?? ""))].filter(Boolean);
  /**
   * The metrics on the LEFT value axis (on a row chart, on the only one), for a given legend
   * selection — `undefined` meaning "as built", which is what every build-time reader wants.
   *
   * A PARAMETER RATHER THAN A CONSTANT because a legend click hides a series without the option
   * being rebuilt, so anything derived from the series list here goes stale the moment one is
   * clicked. `yTitleFor` is the reader that cares; see `axisNamesFor` below for the measurement.
   */
  const leftSideFor = (selected?: Record<string, boolean>) => {
    const shown = selected == null
      ? plotted
      : plotted.filter((s) => s.name == null || selected[s.name] !== false);
    return metricsOf(shown.filter((s) => s.yAxisIndex !== 1));
  };
  /** The SERIES each value axis carries, as built — what the extent, the number format and the
   *  width estimate all take now. A `leftSide` holding the left axis's METRIC NAMES used to stand
   *  here and feed all three; it is gone because under a breakout every band names the same metric,
   *  so a metric list cannot tell the two axes apart. `leftSideFor` survives it, and only for the
   *  axis TITLE, which genuinely is asking "which columns is this axis showing". */
  const leftSeries = onAxis(0);
  const rightSeries = onAxis(1);
  /** The value axis's title, for a legend selection. The ladder is unchanged and its ORDER is
   *  load-bearing: an author's own `yLabel` and their `showYLabel: false` are decisions, not
   *  derivations, so neither may follow the legend — only the metric-name arm does. */
  const yTitleFor = (selected?: Record<string, boolean>) =>
    cfg.showYLabel === false ? undefined : (cfg.yLabel ?? metricTitle(leftSideFor(selected)));
  const yTitle = yTitleFor();
  // OVER A SERIES LIST, not over metric names — see `onAxis`. It used to skip a series whose
  // `__metric` was absent from the list, which cannot tell one breakout band from another.
  /**
   * THE BIGGEST NUMBER THE VALUE AXIS HAS TO DRAW — which on a stack is the PILE'S, not any one
   * datum's, and getting that wrong is what put uncompacted labels under a rotated axis title.
   *
   * MEASURED, at 209px and again at 309px so it is not a fit decision a wider card would undo. The
   * harness's "MB · Area" and "MB · Area Breakout" plot the same metric over the same twelve months
   * to the same ~24.7K maximum with the same settings shape. "MB · Area" drew `0/5K/10K/15K/20K/25K`;
   * the breakout drew `0/5,000/…/25,000`, and those six-character labels ran under the rotated
   * "revenue" title by 6.2x15px on "10,000" and 6.1x15px on "15,000".
   *
   * THE CAUSE WAS TWO FUNCTIONS DESCRIBING ONE AXIS AND DISAGREEING. `axisExtent` sums a stack per
   * category — it has since the row chart's interval fit needed it, and its own note argues why —
   * while this took the largest single DATUM. On a GROUPED chart the two agree by construction:
   * unstacked bands sit side by side from one baseline, so the axis really does stop at the tallest
   * band, and "MB · Bar Breakout" drawing `0/2,000/…/8,000` at a 6K band maximum is correct and
   * matches Metabase. On a STACK they cannot agree: each band peaks near 6K and the axis is drawn
   * to the 24K pile, so the 10,000 threshold below was being asked about a number the axis never
   * draws. The arithmetic here is now `axisExtent`'s own, asked of the same `resolveStacked`
   * predicate `decorateSeries` consults when it writes `stack: "total"`.
   *
   * NORMALISATION NEEDS NOTHING FROM THIS, deliberately. A normalised stack is 0-100 by
   * construction: `isNormalized` short-circuits both `axisExtent` and `fmtFor` before either
   * reaches here, so teaching this function about it as well would be a second copy of a rule that
   * already holds. THE STACK TOTAL LABELS NEED NOTHING EITHER, and for a different reason: they are
   * DATA labels, and `showValues` states one line up that a label sitting on the data is formatted
   * through the exact formatter rather than the compact one. `stackTotalSeries` already does that,
   * so a pile labelled "24,724.86" over an axis reading "25K" is this builder's existing and
   * deliberate split between what the axis says and what a datum says — not a gap this opens.
   *
   * ONE SERIES IN, ONE SERIES OUT: `panelGutter` asks this per panel via `fmtFor([s])`, and summing
   * a single series per index is that series' own values, so the split-panel path is unmoved
   * whatever `cfg.stacked` says.
   */
  const widestOf = (list: SeriesLike[]) => {
    const stacked = resolveStacked(cfg);
    let widest = 0;
    // Positives and negatives kept apart, as `axisExtent` keeps them: ECharts stacks them into two
    // piles growing in opposite directions from the baseline, and a net figure is shorter than both.
    const up: number[] = [], down: number[] = [];
    for (const s of list) {
      const data = (s.data ?? []) as unknown[];
      for (let i = 0; i < data.length; i++) {
        // On a value/time x-axis the datum is a `{value: [x, y], name}` item — the y is what the
        // axis shows. `datumY` unwraps both that and the bare `[x, y]` pair it replaced.
        const y = Number(datumY(data[i]));
        if (!Number.isFinite(y)) continue;
        if (!stacked) {
          const n = Math.abs(y);
          if (n > widest) widest = n;
        } else if (y >= 0) up[i] = (up[i] ?? 0) + y;
        else down[i] = (down[i] ?? 0) + y;
      }
    }
    // `filter` skips the holes a sparse index leaves, exactly as `axisExtent` does — a category
    // every series is null at must not contribute an `undefined` and turn this into NaN.
    for (const v of [...up, ...down].filter((n) => Number.isFinite(n))) {
      if (Math.abs(v) > widest) widest = Math.abs(v);
    }
    return widest;
  };

  const fmtFor = (list: SeriesLike[]) => {
    const metrics = metricsOf(list);
    // 10,000 is where a grouped number reaches six characters and "10K" starts winning.
    const compact = normalized ? false : (cfg.compactValues ?? widestOf(list) >= 10_000);
    return (v: number) => {
      if (normalized) return `${Math.round(v)}%`;
      const m = metrics[0];
      if (!m) return String(v);
      const col = cfg.columnSettings?.[m];
      // A column that states its own `compact` preference outranks the chart-wide default.
      return formatValue(v, cfg.metricTypes?.[m] ?? "double", { ...col, compact: col?.compact ?? compact });
    };
  };

  // Whether the labels fit is a question about WIDTH, and until `size` existed this builder could
  // not ask it: the rule here was `longest > 10 && categories.length > 6 ? 30 : 0`, and length and
  // count are only proxies for width. Four categories whose longest is "Doohickey" on a
  // one-third-width card came out under that rule as "no rotation needed", and then
  // `interval: "auto"` let ECharts drop three of the four — an axis reading as missing data.
  //
  // THE MEASUREMENT applies to a bottom ORDINAL axis and nothing else. A horizontal chart stacks
  // its categories down the left, where their length costs nothing, so there is no collision to
  // measure; and a linear/timeseries x is a value axis whose labels are ticks ECharts generates,
  // not this array — deciding a fit from `categories` there would reserve room below a rotation
  // that never gets applied, since `scaleAxis` carries no `rotate`. Fitting a value axis means
  // generating its ticks first, and NOTHING OWNS THAT: this said "which is PR 03", and PR 03 turned
  // out to be about which axis a scatter gets (`resolveXScale`), not about laying out the ticks on
  // it. The deferral stands; the owner named for it never existed.
  const ordinalX = !horizontal && !usesXScale(cfg);

  /** Metabase's `graph.y_axis.unpin_from_zero`, resolved ONCE for the whole option — the axis flag
   *  below and `axisExtent` above are two halves of the same decision, and computing it twice is
   *  how they would come to disagree on a chart whose stacking or bounds changed mid-build. */
  const unpin = unpinFromZeroOn(cfg);

  /**
   * What a category is DRAWN AS. Hoisted above the fit below rather than written inline on
   * `axisLabel.formatter`, because the fitter has to measure THIS STRING and nothing else — see
   * the `labels:` line at the call site for what happened when it measured the other one.
   *
   * THREE answers in priority order, and the middle one is what PR 03 added:
   *   1. the column's own format, when it says something the formatter for this column would
   *      consult — a stored-then-ignored setting is the defect the pivot, the pie and the breakout
   *      legend each had in turn. "Says something" rather than "exists": on a BUCKETED axis, where
   *      arm 2 is in play, a format silent about dates would otherwise beat the period name with
   *      `formatDate`'s DEFAULTS, and the panel mints exactly such objects — renaming the column to
   *      "Month" stores `{column_title: "Month"}` and reverted this axis from "January 2026" to
   *      "January 1, 2026" (see `specifiesDateRendering`). Off a bucketed axis there is nothing for
   *      the format to outrank, so it still wins on existence there, as it always did.
   *   2. otherwise the NAME OF THE PERIOD the bucket covers, when the axis is bucketed by a
   *      truncation unit — "January 2026" for a month, not its first day;
   *   3. otherwise the raw value, which is every non-temporal axis and every temporal column that
   *      was not bucketed.
   * `periodName` is total and falls through to the raw string itself, so it IS answer 3 as well —
   * `namesAPeriod` is asked only whether arm 2 EXISTS to be outranked, never to guard the call.
   *
   * The trend card answers the same question about the same column and has to stay in step:
   * `SmartScalarChart.tsx`, where arms 1 and 2 are these and ARM 3 IS NOT, and says so.
   */
  const bucketedX = namesAPeriod(cfg.xAxisUnit);
  /**
   * METABASE'S "Compact", THE FIFTH `graph.x_axis.axis_enabled` VALUE, AND IT IS SMALLER THAN THE
   * NAME SUGGESTS. Its three x-axis model builders say the same thing three times —
   * `echarts/cartesian/model/axis.ts:716` (timeseries), `:778` (numeric), `:856` (category) —
   * and each says only:
   *
   *     compact: settings["graph.x_axis.axis_enabled"] === "compact",
   *
   * spread into `formatValue`'s options. It is A FORMATTING FLAG ON THE LABEL TEXT. Nothing about
   * layout, tick density or rotation is touched upstream, and nothing is touched here either.
   *
   * WHY THAT IS THE RULE AND NOT WHAT YOU SEE. Measured live at :3033 on "MB · Line" (12 monthly
   * buckets): Show drew SIX labels (March 2026 / May / July / September / November / January),
   * Compact drew TWELVE (Feb 2026 / Mar 2026 / … / Jan 2026). A reader would reasonably conclude
   * Compact means "draw every label". It does not — twelve labels are a CONSEQUENCE: shorter
   * strings stop colliding, so the tick chooser stops thinning. Porting "draw every label" would
   * have been copying the value instead of the rule, and it would break the moment a card was
   * narrow enough that even "Jan 2026" did not fit. Nothing below tells the fitter anything: it is
   * handed `drawnCategories`, which is this closure's own output, so the density follows for free.
   *
   * BOTH ARMS TAKE IT, because Metabase's single `compact` reaches both halves of `formatValue` —
   * a date's month token and a number's magnitude suffix. egret spells those as two `ColumnFormat`
   * fields (`date_abbreviate` and `compact`, `format.ts:103` and `:60`), so ONE flag here sets
   * BOTH; setting only `compact` would have made a compact date axis a no-op, silently, on the very
   * axis the feature is named for. Spread OVER the stored format rather than replacing it, which is
   * again Metabase's shape (`{...columnSettings, compact}`): a user who has chosen a date style
   * keeps it and gets it abbreviated, instead of having it swapped out from under them.
   */
  const compactX = cfg.xLabelCompact === true;
  const drawLabel = (v: string) => {
    const stored = cfg.xAxis ? cfg.columnSettings?.[cfg.xAxis] : undefined;
    // Compact is itself something the format now SAYS, so it makes arm 1 reachable on a column that
    // has no stored settings at all. Without this line a numeric category axis with an untouched
    // format fell through to arm 3 and drew `12400` with Compact on — an inert control on one of the
    // two halves of Metabase's single flag, which is the defect this programme keeps closing.
    const fmt = compactX ? { ...stored, compact: true, date_abbreviate: true } : stored;
    // ASKED OF `stored`, NEVER OF `fmt`, and this is the one line where the difference bites. The
    // `date_abbreviate` compact adds would make `specifiesDateRendering` true on every bucketed
    // axis, handing it to `formatDate`'s DEFAULTS — so a compact monthly axis would have drawn
    // "Jan 1, 2026" instead of "Jan 2026". Compact is a modifier on whatever wins; it is not itself
    // a reason for the column format to outrank the period name.
    const formatWins = fmt != null && (bucketedX ? specifiesDateRendering(stored) : true);
    if (formatWins && v != null) {
      const out = formatValue(v, cfg.xAxisType ?? "varchar", fmt);
      if (out !== "") return out;
    }
    return periodName(cfg.xAxisUnit, v, compactX);
  };

  // An explicit rotation SKIPS the measurement rather than merely outranking it. Computing the fit
  // anyway and using it for `show` would honour the user's 45° and hide their axis in the same
  // breath, which is worse than either outcome alone and looks like the setting doing nothing.
  // Metabase's own auto-selection likewise applies only while the setting is untouched.
  const explicitRotate = cfg.xLabelRotation != null;
  // A zero size is not a small chart, it is an UNMEASURED one: `EChartsRenderer` reads
  // `getBoundingClientRect()` before `init`, so a hidden or not-yet-laid-out container reports
  // `{0, 0}`. Measuring it would give a negative `plotWidth` — which the fitter answers "draw every
  // label upright", from a container nobody could see — and a zero height would make the rotated
  // label's projected share infinite and hide the axis. Both are decisions taken against a guessed
  // width, which is the one thing `size`'s own doc says never to do, so decline instead.
  const measurable = size != null && size.width > 0 && size.height > 0;
  /** What the category axis draws, wherever that axis sits — measured by the ladder below on a
   *  vertical chart and by the row chart's two fits further down, so the two cannot disagree about
   *  which strings are on the card. */
  const drawnCategories = categories.map((c) => drawLabel(String(c)));
  /** Raw category -> the string the axis draws for it. `ariaPoints` needs the reverse lookup
   *  because a value/time datum carries its RAW category and not its label. */
  const drawnByRaw = new Map(categories.map((c, i) => [String(c), drawnCategories[i]]));
  const fit: XLabelFit | undefined = measurable && size && ordinalX && !explicitRotate
    ? fitXLabels({
        // THE STRING THE AXIS ACTUALLY DRAWS, by construction — same closure, same argument.
        //
        // This measured `categories.map(String)`, the RAW value, and justified the gap as "the
        // formatted label is the wider of the two, so this under-measures". PR 03 falsified both
        // halves at once. The two now differ whenever `xAxisUnit` is a truncation unit — the
        // common case and this PR's whole subject, not the rare `columnSettings` one that comment
        // named — and the period name is often NARROWER, not wider: a year measured "2026-01-01"
        // at ten characters and drew "2026" at four, a quarter ten against "Q3 2026" at seven. So
        // it had begun to OVER-measure, which rotates labels that would have sat upright, in the
        // PR whose entire motivation is labels not fitting. Formatting here retires the
        // `columnSettings` skew too, which that comment conceded and left standing.
        labels: drawnCategories,
        // The widest VALUE the left axis carries and the axis's own formatter — not the formatted
        // datum, which is what this passed until the gutter was found reserving 10px for "24.7K" on
        // an axis whose top tick is "25K". `estimatePlotWidth` rounds to a tick it would draw.
        plotWidth: estimatePlotWidth(size, widestOf(leftSeries), fmtFor(leftSeries), Boolean(yTitle)),
        // The CONTAINER's height, not an inset one: the 70% guard is a share of the whole card
        // (Metabase's `checkHeight`), so passing a grid-adjusted height silently tightens it.
        outerHeight: size.height,
        // THE PRECONDITION FOR THINNING, enforced here rather than assumed downstream: only a
        // BUCKETED temporal axis gets one. `bucketedX` is `namesAPeriod(cfg.xAxisUnit)`, the same
        // single membership rule `drawLabel` above uses, so an axis that is thinned is by
        // construction an axis whose labels are period names. On an ordinal axis of product names
        // this is `undefined` and the fitter cannot reach its stride arm at all — there is no such
        // thing as every second product. (Both fields are also empty for a non-period unit, so the
        // guard holds a second time even if this line is ever loosened.)
        //
        // ONLY THE LADDER, no sizing candidate. This used to pass `widestLabel:
        // widestPeriodName(cfg.xAxisUnit, drawLabel)` as well — the widest name the unit could
        // produce, threaded through this builder's own `drawLabel` so the stride was sized against a
        // string the axis would really render. It was still the wrong string: a rung draws indices
        // 0, every, 2*every, … and the fitter now measures exactly those out of `labels:` above,
        // which is the same closure's output for the buckets this chart actually has. See
        // `strideThatFits` for the browser measurement and for what dropping the unit's worst case
        // concedes.
        boundary: bucketedX ? { steps: boundarySteps(cfg.xAxisUnit) } : undefined,
      })
    : undefined;
  // …but an explicit rotation is honoured WHEREVER the category axis sits, `ordinalX` or not. On a
  // row chart it is the left-hand axis, and ECharts rotates that one too: `buildAxisLabel` reads
  // `axisLabel.rotate` for every axis kind and `innerTextLayout` composes it with the axis's own
  // rotation (`PI/2` for a y-axis, `cartesianAxisHelper.js`), so 90° there turns the names to run
  // vertically alongside the axis. Dropping the setting because the axis is not at the bottom
  // would be a silent no-op in the panel — and the panel offers this control on row charts, where
  // it labels the vertical axis as the category one.
  const rotate = cfg.xLabelRotation ?? (fit?.kind === "rotate" ? fit.rotate : 0);

  /**
   * A ROW CHART'S TWO AXES ARE THE OTHER WAY UP, and both of the fits above are about the bottom
   * one, so neither has ever run for it. What it needs instead is measured here.
   *
   * THE LADDER ABOVE IS NOT SIMPLY WIDENED TO REACH IT, which is the tempting one-line version.
   * `ordinalX` excludes a horizontal chart for the right reason: the ladder's answers are ROTATE and
   * HIDE, and a row chart's bottom axis is a VALUE axis. Its labels are round ticks ECharts
   * generated, so tilting them is a remedy Metabase does not offer there (it greys the control out
   * on a value axis) and hiding them is not what its row chart does either — at 311px it draws one
   * tick and keeps drawing it. Fewer ticks is the value axis's answer; see `intervalThatFits`.
   *
   * WHAT THE LEFT SIDE COSTS IS THE CATEGORY NAMES, not a column of value ticks — the row chart is
   * the one cartesian where those two swap edges — so the gutter is measured from the labels
   * themselves rather than sampled from `widestTickLabel`. Measured ONCE, because the same width
   * decides both answers: how much plot is left for the ticks, and how far out the category axis's
   * own name has to sit to clear its labels.
   *
   * Live, at the 166x184 card this was found on: `Doohickey` measures 60.8 of the 166, leaving ~55
   * for ticks that measure 30.6 apiece — one label's worth, where four were being drawn on an 8px
   * pitch, each overlapping its neighbour by 23px.
   *
   * A VERTICAL CHART MEASURES THE SAME STRINGS, which is why the `horizontal &&` guard this line
   * carried is gone rather than mirrored. There the names are on the BOTTOM axis, and what their
   * width buys is a height: turned 90° each one runs its own width down the card, and
   * `categoryLabelBand` is the projection. The two uses are the same measurement asked for opposite
   * dimensions, so taking it once is what keeps a bar chart and a row chart of the same data
   * agreeing about how long `Doohickey` is.
   */
  const measuredCategories = measurable && size ? widestLabelWidth(drawnCategories) : 0;
  // A ZERO IS NOT A ZERO-WIDTH LABEL, it is no measurement at all: `measure-text.ts` reports one
  // deliberately where there is no canvas ("Size claims belong in a browser"), which is every jsdom
  // test in the repo. Declining there is the same rule `intervalThatFits` keeps for its own candidate
  // and the same one `measurable` keeps for `size` — a name placed against a width nobody observed
  // is the guess this builder does not make. It also keeps every unmeasured row chart's option
  // exactly as it was before this fit existed.
  const categoryGutter = measuredCategories > 0 ? measuredCategories : undefined;
  /**
   * How far the value axis runs — the distance an interval has to divide, and the magnitude its
   * widest tick is formatted from.
   *
   * A BAR AXIS INCLUDES ZERO whether or not the data does — ECharts' `scale` defaults to false and
   * a bar measured from a floating baseline lies — so the extent starts at the origin and is
   * widened by the data. `yMin`/`yMax` replace their end when the user has set one, and a
   * normalised chart is 0-100 by construction (the `max: 100` below is the same statement).
   *
   * A STACK IS SUMMED, PER CATEGORY, and leaving that out was a REGRESSION rather than a gap: three
   * stacked metrics whose largest datum is 300K reach 870K together, and an interval sized against
   * 300K asks for nine labels where six fit. Measured at 300px and 320px, a stacked row chart that
   * drew clean ticks before this fit existed came out overlapping by 13.9 and 11.9px — worse than
   * the defect the fit is here to remove. `resolveStacked` is the same predicate `decorateSeries`
   * consults when it writes `stack: "total"`, asked here rather than restated, so the two cannot
   * disagree about which series pile up.
   *
   * GROUPED BARS ARE NOT SUMMED, and that is not an oversight: unstacked series sit side by side,
   * each measured from the same baseline, so the axis only has to reach the tallest single one.
   * `resolveStacked` is false for them, which is the whole distinction.
   */
  const axisExtent = (list: SeriesLike[]): { lo: number; hi: number } => {
    if (normalized) return { lo: 0, hi: 100 };
    const stacked = resolveStacked(cfg);
    // THE HALF OF "UNPIN FROM ZERO" THAT `scale: true` CANNOT REACH, and the reason the flag is
    // read here as well as on the axis. Metabase hands ECharts nothing but `scale`; this builder
    // does not stop there — it measures the card and, when the ticks would crowd, writes an
    // EXPLICIT `min`/`max` from `verticalValueAxisFit` (and, on a row chart, an `interval` from
    // `intervalThatFits`). Both are derived from this function, whose seed of `lo = 0, hi = 0` is
    // itself a statement that the axis includes the origin. An explicit `min` outranks `scale`
    // OUTRIGHT in ECharts, so porting the flag alone would have left the axis pinned on exactly
    // the small cards a user looks at — a browser measures, jsdom does not — while every unmeasured
    // unit test passed. That is this programme's recurring "saves but does not paint" defect
    // reached from a new direction, which is why the seed moves with the flag.
    const seed = unpin ? { lo: Infinity, hi: -Infinity } : { lo: 0, hi: 0 };
    // Per category index, so a stack is summed where it is drawn. Positives and negatives are kept
    // apart because they grow in opposite directions from the baseline — ECharts stacks them into
    // two piles, not one net figure, and an axis sized on the net one is too short at both ends.
    const up: number[] = [], down: number[] = [];
    let lo = seed.lo, hi = seed.hi;
    // OVER THE SERIES ON THIS AXIS, not over the ones whose metric it names — see `onAxis`. Under a
    // breakout the `__metric` filter this replaces admitted every band to both axes, so a split of
    // `small` [1,2] from `big` [1000,4000] produced two axes both spanning 0–4000 and the split
    // changed nothing a reader could see.
    for (const s of list) {
      const data = (s.data ?? []) as unknown[];
      for (let i = 0; i < data.length; i++) {
        // On a value/time x-axis the datum is an [x, y] pair — the y is what the axis shows.
        const n = Number(Array.isArray(data[i]) ? (data[i] as unknown[])[1] : data[i]);
        if (!Number.isFinite(n)) continue;
        if (!stacked) {
          if (n < lo) lo = n;
          if (n > hi) hi = n;
        } else if (n >= 0) up[i] = (up[i] ?? 0) + n;
        else down[i] = (down[i] ?? 0) + n;
      }
    }
    // `filter` skips the holes a sparse index leaves, so a category every series is null at cannot
    // contribute an `undefined` and turn the whole extent into NaN.
    if (stacked) {
      hi = Math.max(hi, ...up.filter((v) => Number.isFinite(v)));
      lo = Math.min(lo, ...down.filter((v) => Number.isFinite(v)));
    }
    // An unpinned axis with NO finite datum still holds its seed, and `±Infinity` is not an extent
    // — the fits below would take a span of `-Infinity` and decline, which is the right answer by
    // accident rather than by statement. Fall back to the pinned seed so the two paths agree about
    // what an empty chart's axis is, and so nothing downstream has to hold an infinity.
    // (`stacked` cannot be true here while `unpin` is: `unpinFromZeroApplies` refuses a stack, so
    // the `Math.min(Infinity, ...[])` this line would otherwise have to answer for is unreachable.)
    if (!Number.isFinite(lo) || !Number.isFinite(hi)) { lo = 0; hi = 0; }
    return { lo: cfg.yMin ?? lo, hi: cfg.yMax ?? hi };
  };
  // A LOG axis is excluded: its ticks are powers, so a linear interval means something else
  // entirely there and the crowding it suffers is not the crowding this measures.
  //
  // `horizontal` is back on this line and not on `measuredCategories` above, which is the swap this
  // change makes: the MEASUREMENT is shared with the vertical fit below, the WIDTH fit it feeds is
  // still the row chart's alone. Without it every vertical chart would be handed the horizontal
  // budget for its left-hand ticks — the axis where a label's width costs nothing.
  const rowExtent = horizontal && categoryGutter != null && size && !cfg.logScale
    ? axisExtent(leftSeries) : undefined;
  const valueInterval = rowExtent && size
    ? intervalThatFits({
        plotWidth: plotWidthLessGutter(size, categoryGutter!, Boolean(xTitle)),
        span: rowExtent.hi - rowExtent.lo,
        // BOTH FROM THE SAME EXTENT, and not `widestOf`: a value axis draws round ticks, not data,
        // and on a stack the number it reaches is the pile's, not any one datum's. (`widestOf` is
        // the ladder's, one axis over and one chart kind away — it feeds a vertical chart's gutter,
        // where the labels are the left axis's own and the stack question does not arise. The two
        // agree on every unstacked chart, where the extent's ends ARE the extreme data.)
        // On a horizontal chart `leftSide` is the metric side whatever the name says — the split is
        // by series, and `isDualAxis` is false here, so every plotted series is in it.
        widestValue: Math.max(Math.abs(rowExtent.lo), Math.abs(rowExtent.hi)),
        format: fmtFor(leftSeries),
      })
    : undefined;
  // Metabase puts the legend along the top; reserve room there for it. The category axis
  // sits at the bottom normally and on the left when horizontal — the value axis is the
  // other one — so reserve room on whichever edge each title actually renders on.
  //
  // HOISTED ABOVE THE AXES rather than left beside the option it goes into: the vertical fit below
  // subtracts these two insets from the card to find its plot, and taking them from the object the
  // builder really emits is what stops that arithmetic drifting from the grid it describes.
  const bottomTitle = horizontal ? yTitle : xTitle;
  const leftTitle = horizontal ? xTitle : yTitle;

  /**
   * SPLIT PANELS — Metabase's `graph.split_panels`, which its panel labels "Stack series".
   *
   * ONE GRID PER PLOTTED SERIES, stacked down the card, all sharing one category axis that is drawn
   * once under the last of them, and each with a value axis scaled to ITS OWN data. That last part
   * is the feature: `Count` against `Sum of Tax` on one axis is a line at the top and a line flat
   * along the bottom, and on two axes it is two readable lines.
   *
   * WHAT `plotted` ALREADY GUARANTEES, and the reason this needs no filtering of its own: it is the
   * series that survived `hiddenSeries` and `seriesOrder`, minus the trend and stack-total overlays.
   * So "one panel per VISIBLE series" is the list, in the order the reader sees, and a series the
   * author curated away never gets a panel.
   *
   * …AND WHY THERE IS NO LEGEND HERE. Metabase renders its legend as React DOM above the canvas;
   * egret's IS an ECharts component, and clicking it hides a series WITHOUT the option being
   * rebuilt. Under panels that would have to change the panel COUNT and re-lay-out the rest — which
   * a merge `setOption` cannot express, and merge patches are exactly what `__ariaFor` and
   * `__axisNamesFor` exist to send (#412, #417). A TITLE PER PANEL is what replaced it, and it is
   * not a compromise: Metabase has no per-panel titles at all and leaves the reader colour-matching
   * against a legend. Naming the panel says the same thing in the place it is about, and it removes
   * the toggle that could not have been honoured.
   */
  const panelSeries = cfg.splitPanels && splitPanelsApplies(cfg) ? plotted : [];
  /**
   * The widest value label ANY panel would draw, measured once for all of them.
   *
   * Every panel's plot must start at the same x or the shared category axis is a fiction — the same
   * date would sit at a different pixel in each panel. That is why these grids run with
   * `containLabel: false` (see `splitPanelLayout`), and switching it off means this builder owes
   * ECharts the left inset it would otherwise have computed. Metabase owes it the same debt and
   * pays it the same way: `Math.max` over the panels' tick widths (`computeSplitPanelLayout`).
   *
   * THE TWO ENDS ARE THE CANDIDATES, not every tick: a value axis's labels are round numbers
   * between its extremes, so the widest of them is at one end or the other. `axisExtent` and
   * `fmtFor` are asked PER PANEL, which is the whole point — panel 1 formats `4` and panel 2 formats
   * `4000` as "4K", and charging every panel the widest chart-wide label would indent them all by
   * the biggest number on the card.
   *
   * Zero where there is no canvas (jsdom), exactly as `categoryGutter` is: the inset then reserves
   * only the title band, which is wrong for a chart nobody is drawing and right for one nobody can
   * measure. Nothing renders there.
   */
  const panelGutter = panelSeries.length > 1
    ? widestLabelWidth(panelSeries.flatMap((s) => {
        const e = axisExtent([s]);
        const f = fmtFor([s]);
        return [f(e.lo), f(e.hi)];
      }))
    : 0;
  /**
   * The band the category labels occupy below the plot, hoisted above the grid it is measured
   * against — the split layout needs it to know where the LAST panel has to stop, and the
   * single-grid `verticalPlot` below needs it for the same subtraction it always did.
   *
   * A HIDDEN axis draws nothing below the plot, so it reserves nothing — and a value/time x-axis is
   * excluded outright: its bottom labels are ECharts' own round ticks rather than these strings, so
   * the band would be measured off labels the axis does not draw. Declining is the safe direction,
   * and it leaves every scale-x chart's option untouched.
   */
  const categoryBand = categoryGutter != null && fit?.kind !== "hide"
    ? categoryLabelBand({ rotate, widestLabel: categoryGutter })
    : 0;
  /**
   * Where the panels go, or `undefined` where the card cannot carry them.
   *
   * THE INSETS ARE THE SPLIT CHART'S OWN and not the `grid` below, which is the ordering this block
   * forced: `grid.top` reserves 24px for a legend, and a split chart has none, so reading it here
   * would charge the panels for a component that is not drawn. The bottom is the reverse — a split
   * chart owes the category labels an explicit reservation where a `containLabel` grid gets it free.
   * `Math.max(band, axisFontSize)` because an unmeasured band is 0 and even an unmeasured axis draws
   * one line of text.
   *
   * MEASURED, OR NOT AT ALL. `measurable` is the same guard `fit` and `verticalPlot` keep: a zero
   * size is an UNMEASURED card, not a small one, and a panel boundary derived from it is the guess
   * this builder does not make. An unmeasured build keeps the chart it always drew.
   */
  const panels = panelSeries.length > 1 && measurable && size
    ? splitPanelLayout({
        size,
        count: panelSeries.length,
        inset: {
          top: CHART_THEME.grid.top as number,
          bottom: (CHART_THEME.grid.bottom as number) + (bottomTitle ? 22 : 0)
            + Math.max(ordinalX ? categoryBand : 0, CHART_THEME.axisFontSize) + AXIS_LABEL_MARGIN,
        },
      })
    : undefined;
  /** Whether the panels are actually being drawn — the layout's answer, not the config's. */
  const splitting = panels != null;
  /**
   * …so the LEGEND survives the fallback. A card too short for panels falls back to the ordinary
   * chart, and an ordinary multi-series chart without a legend is a chart whose lines are unnamed.
   * The stacking overrides deliberately do NOT follow this switch — see `buildCartesianOption`,
   * which argues why a stack that appears and disappears with a card's height is the worse answer.
   */
  const drawLegend = showLegend && !splitting && legendFitsAcross(size, plotted.length);

  // Metabase puts the legend along the top; reserve room there for it. The category axis
  // sits at the bottom normally and on the left when horizontal — the value axis is the
  // other one — so reserve room on whichever edge each title actually renders on.
  //
  // HOISTED ABOVE THE AXES rather than left beside the option it goes into: the vertical fit below
  // subtracts these two insets from the card to find its plot, and taking them from the object the
  // builder really emits is what stops that arithmetic drifting from the grid it describes.
  const grid = {
    ...CHART_THEME.grid,
    top: (CHART_THEME.grid.top as number) + (drawLegend ? 24 : 0),
    bottom: (CHART_THEME.grid.bottom as number) + (bottomTitle ? 22 : 0),
    left: (CHART_THEME.grid.left as number) + (leftTitle ? 26 : 0),
  };
  /**
   * The panels' grid rects. One left inset for all of them, computed rather than contained — see
   * `panelGutter`. `right` is `CHART_THEME`'s own: what overhangs on that edge is half of the last
   * category label, and it overhangs on a single-grid chart too, where `containLabel` pulls the plot
   * in. Left as the plain inset here rather than guessed at, and recorded as the one term of this
   * layout that is Metabase's (`firstXTickWidth`/`lastXTickWidth`) and not yet egret's.
   */
  const panelGrids = panels?.map((p) => ({
    left: (CHART_THEME.grid.left as number) + (leftTitle ? 26 : 0) + panelGutter + AXIS_LABEL_MARGIN,
    right: CHART_THEME.grid.right as number,
    top: p.top,
    height: p.height,
    containLabel: false,
  }));

  /**
   * …AND A VERTICAL CHART'S LEFT-HAND VALUE AXIS HAS THE SAME DEFECT ON THE OTHER DIMENSION, which
   * the fit above cannot see because it is budgeted in pixels of WIDTH.
   *
   * Live, at the same 166x184 card: the bar chart drew `100K 200K 300K 400K` on a 10.7px pitch
   * against 15px label boxes — every neighbouring pair overlapping — and the box plot beside it drew
   * `20 40 60 80` on the same pitch. The line, waterfall, combo and scatter cards on that dashboard
   * were clean at 21px, and the difference is not the numbers on the axis: it is that a bar's four
   * product names turn 90° and eat 72px off the bottom of the card, leaving a 56px plot where the
   * line chart's thinned period names leave 122. Six ticks fit in 122px and do not fit in 56.
   *
   * SO THE INPUT IS A PLOT HEIGHT AND NOT A CARD HEIGHT, and it is the one thing this fit needs
   * measuring for — a label's height is its font's, so nothing about the pitch is a `measureText`
   * call. `categoryLabelBand` turns the widest name and the rotation the ladder chose into the band
   * ECharts reserves below the plot; the two together are what the builder knows and ECharts does
   * not tell it.
   *
   * `categoryGutter != null` GATES IT, so an unmeasured card is left exactly as it was — the same
   * rule the row chart's fit keeps, reached from the other side. The band at 90° IS the measurement,
   * and the rotation that selects the arm came from `fitXLabels`, which measured too; deciding a
   * tick count from a jsdom's zero would be the guess this builder does not make.
   *
   * (`categoryBand` itself is now computed further up, beside the split-panel layout that also needs
   * it. Nothing about the arithmetic moved — only the line.)
   */
  // A HIDDEN axis draws nothing below the plot, so it reserves nothing — and a value/time x-axis is
  // excluded outright: its bottom labels are ECharts' own round ticks rather than these strings, so
  // the band would be measured off labels the axis does not draw. Declining is the safe direction,
  // and it leaves every scale-x chart's option untouched.
  // (`ordinalX` is `!horizontal && !usesXScale(cfg)`, so a row chart is already out — writing
  // `!horizontal` again here would read as a second condition and mutate to nothing.)
  //
  // A SPLIT PANEL'S PLOT HEIGHT IS THE PANEL, EXACTLY, and it is read straight off the layout rather
  // than through `plotHeightLessLabels`. That function subtracts the card's insets and the category
  // band from the CARD to find the one plot on it; with panels there is no such subtraction to make
  // — the layout already did it once for all of them, `containLabel` is off so nothing expands, and
  // every panel is the same height by construction, so panel 0's is every panel's. Reaching for the
  // card-wide function here would have charged each panel the whole chart's bottom labels.
  //
  // `ordinalX` is NOT required on that arm, and that is the one condition this branch drops rather
  // than inherits: it guards the `band` term, which this arm does not use. A timeseries chart —
  // where two metrics of different magnitude are most often plotted together, and so the commonest
  // reason to reach for panels at all — therefore gets its per-panel fit instead of being silently
  // excluded from it. The log exclusion stays: a log axis's ticks are powers, and a linear interval
  // means something else entirely on one.
  const verticalPlot = panels && !cfg.logScale
    ? panels[0].height
    : ordinalX && categoryGutter != null && size && !cfg.logScale
      ? plotHeightLessLabels({ size, band: categoryBand, grid })
      : undefined;
  /** The scale a VERTICAL value axis needs, per axis — `mkValueAxis` is called twice on a dual
   *  chart and the two carry different series, so the extent is asked for the series that axis
   *  actually shows rather than for the chart. A NORMALISED axis is 0-100 by construction and
   *  already divides cleanly, so it takes the interval and keeps its own ends. */
  const verticalFit = (list: SeriesLike[]) => {
    if (verticalPlot == null) return undefined;
    const e = axisExtent(list);
    return verticalValueAxisFit({ plotHeight: verticalPlot, lo: e.lo, hi: e.hi });
  };
  // Either way the axis has now DECIDED, and a decided axis never leaves ECharts the subset
  // `interval: "auto"` would let it pick. What it draws is every label, none, or — on a bucketed
  // temporal axis only — every Nth, which the `interval` line below argues is not the same thing.
  const decided = fit != null || explicitRotate;
  // NO ROOM IS RESERVED BELOW THE PLOT FOR A TILTED LABEL, at any rotation, measured or explicit.
  // See `XLabelFit`'s doc in `../axisLabelFit.ts` for the measurement that settled it: `containLabel`
  // already expands the grid around the rotated labels, and everything this builder added on top
  // came off the plot one-for-one. Deleted rather than set to zero — a `+ 0` in the grid below is
  // an invitation to put a number back in it.

  const categoryAxis = {
    type: "category" as const, data: categories,
    name: xTitle, nameLocation: "middle" as const,
    // `nameGap` is measured from the AXIS LINE, so what it has to clear is whatever sits between the
    // line and the name — nothing at the bottom of a vertical chart, where 28 has always been right,
    // but the whole column of category names on a row chart, where the title was drawn straight
    // across them. `nameGapClearOf` states the three terms.
    //
    // 28 STANDS IN TWO CASES, and they are different refusals. Unmeasured, because a gap guessed
    // from no width is the one thing this builder's `size` doc says never to do. And on a card too
    // narrow for the labels it has, because there the clearing gap would put the title off the
    // canvas — 28 draws it across its labels, which is this PR's own defect, but a title one can
    // read over the top of beats a title that is not there. `nameGapClearOf` owns that judgement
    // and says `undefined`; this line only has to have somewhere to fall back to.
    //
    // 28 NO LONGER STANDS AT THE BOTTOM OF A VERTICAL CHART, which is what the comment above used to
    // claim and what the live card falsified: the labels there are ROTATED, so they run down into
    // exactly the band the title is placed in, and `category` was drawn across all four of them.
    // Same three terms, a different clamp — `bottomNameGapClearOf` states why the left axis's does
    // not transfer. `Math.max` because a hidden axis reserves no band and its clearing gap comes out
    // under the flat 28, which is the gap an axis with nothing below it should keep.
    nameGap: horizontal
      ? (categoryGutter != null && size ? nameGapClearOf(categoryGutter, size) : undefined) ?? 28
      : Math.max(28, (categoryGutter != null && size
          ? bottomNameGapClearOf({ band: categoryBand, size, gridTop: grid.top })
          : undefined) ?? 28),
    // The title matches the label colour — an axis name is not a lesser piece of chrome
    // than the values beneath it, and ECharts' default nameTextStyle colour reads paler.
    nameTextStyle: { color: axisLabelColor },
    // Category labels go through the column's own format. Extraction units are already
    // substituted above (`displayCats`); a TRUNCATION unit is named by the formatter below —
    // a chart grouped by month read "2026-01-01" where the bucket is really January 2026. The
    // formatter only changes what is drawn; `data` stays the raw value, so a drill still filters
    // on what the engine returned.
    axisLabel: {
      fontSize: CHART_THEME.axisFontSize, color: axisLabelColor,
      // `fit.every - 1`, NOT `fit.every`: ECharts' category `interval` counts the labels SKIPPED
      // between two drawn ones, not the stride. `ordinalScaleCreateTicks`
      // (`echarts/lib/scale/helper.js:235`) reads it as `step = max((categoryInterval || 0) + 1, 1)`
      // and walks `tickValue += step` from the extent's start — so `0` is every label, which is what
      // the `decided` arm below has always meant, and every 6th is `5`. Off by one here is invisible
      // in a screenshot: it draws Jan/Jun/Nov instead of Jan/Jul, which looks like a date axis.
      //
      // Two more things checked in that source rather than assumed, because both would undo the
      // "every drawn label is a period boundary" property this rests on. The stride starts at INDEX
      // ZERO — `startTick` is only re-aligned when the extent starts past 0, i.e. under a data zoom
      // this axis does not have (`:239-241`) — so the first bucket is always one of the drawn ones.
      // And the last index is appended `offInterval` (`:251-253`) but then IGNORED by `AxisBuilder`
      // while `showMinLabel`/`showMaxLabel` are unset, which they are (`AxisBuilder.js:675`); so a
      // December that is not on the stride is not quietly drawn beside July. The tick MARKS follow
      // the labels for free: `axisTick` carries no `interval`, which defaults to "auto", and "auto"
      // on a tick model means "whatever the labels decided" (`axisTickLabelBuilder.js:182-187`).
      //
      // AND THIS IS NOT PR 02's RULE BEING UNDONE, which is the reading to head off: "auto" drops
      // whichever labels happen to collide, an arbitrary subset, while a stride over buckets draws
      // period boundaries — the full argument, and why restoring a flat `interval: 0` here would
      // NOT restore PR 02, is on the `thin` arm of `XLabelFit` in `../axisLabelFit.ts`, which is
      // its one copy.
      interval: fit?.kind === "thin" ? fit.every - 1 : decided ? (0 as const) : ("auto" as const),
      // THE FIRST BUCKET IS THE ONE LABEL A STRIDE PROMISES, so on a thinned axis it is declared
      // required rather than left to ECharts' min/max heuristic — which discards it. Read in
      // `AxisBuilder.fixMinMaxLabelShow` (`:648-726`) against echarts 6.1, because a `thin` decision
      // that renders nothing is exactly what this looked like on the live dashboard:
      //
      //   - `ordinalScaleCreateTicks` (`scale/helper.js:229-253`) walks the stride from index 0 and
      //     then APPENDS the extent's last index with `offInterval: true`. Twelve monthly buckets at
      //     stride 12 therefore hand `AxisBuilder` two labels, "January 2026" and "December 2026",
      //     not one.
      //   - `fixMinMaxLabelShow` runs `deal(showMinLabel, 0, 1)` FIRST. With the option unset it
      //     tests the two for intersection and, finding one, ignores the OUTMOST — January — before
      //     `deal(showMaxLabel, 1, 0)` ever gets to drop December for being `offInterval`. Two
      //     labels ~84 and ~91px wide on a 94px plot overlap by about a pixel, so the axis lost the
      //     only label it had chosen to draw, to a label it was about to throw away.
      //
      // `true` flips that one branch: the inner label is dropped instead, which is the same December
      // the next `deal` would have dropped anyway. Set ONLY on the thin arm — index 0 is on-stride
      // there by construction (`ordinalScaleCreateTicks` starts at 0), and it is the label the whole
      // `thin` argument in `../axisLabelFit.ts` rests on. On every other arm the axis draws all its
      // labels or none, and this heuristic has nothing to decide.
      showMinLabel: fit?.kind === "thin" ? true : undefined,
      rotate,
      // The last answer the ladder can give: when even a rotated label would eat 70% of the
      // card's height, Metabase draws no axis rather than a wall of text.
      show: fit?.kind !== "hide",
      // `drawLabel`, defined above so the FITTER measures this same string. Do not inline it back:
      // the two drifting apart is what made the fit rotate labels that would have fitted upright.
      formatter: drawLabel,
    },
    // TWO different `show`s sit on this axis and they are not the same switch. `xAxisTicks` is the
    // axis LINE and the tick marks; the LABELS are `axisLabel.show` above, which only the measured
    // fit ever writes. Metabase's single `axis_enabled: false` hides both, so egret's "Hide" is
    // narrower than its namesake — a pre-existing divergence shared with the other two charts that
    // ship this select (the waterfall and the box plot, via `settings/ChartAxesSection.tsx`; a
    // sankey has no axes and stores neither field), but worth saying here now that a second `show`
    // on the same axis could be mistaken for the one the panel writes.
    axisLine: { show: cfg.xAxisTicks !== false, lineStyle: { color: gridLine } },
    axisTick: { show: cfg.xAxisTicks !== false },
  };
  // Non-ordinal x: a value (linear) or time (timeseries) axis positioning points by
  // their real coordinate. Series carry [x, y] pairs (see applyXScale).
  //
  // No `rotate` here, and PR 03 did NOT change that — this note used to say rotating these labels
  // "is PR 03", and PR 03 turned out to be about which axis a scatter gets, not about laying out the
  // ticks on it. An explicit `xLabelRotation` is still a NO-OP on a value x-axis.
  //
  // NOT BECAUSE IT COULD NOT BE APPLIED. ECharts would honour it: `AxisBuilder.buildAxisLabel` reads
  // `axisLabel.rotate` with no branch on the axis kind (`AxisBuilder.js:875`), so adding one key
  // here would rotate these generated ticks today. The reason not to is PARITY — Metabase greys the
  // rotation out on a value axis too, so honouring it would be a divergence dressed up as a fix, in
  // the workstream whose whole point is to stop diverging. A second reason, real but thinner in a
  // product this young: a chart saved as Linear can still CARRY a rotation from before the switch,
  // and applying it would silently turn labels nobody has touched in months. Either way the setting
  // is one the panel must not OFFER where it would land here, which is what `valueX` is for.
  //
  // WHAT PR 03 ADDED TO THIS AXIS'S CATCHMENT, and the reason the panel rule had to move with it: a
  // scatter over a numeric dimension now arrives here WITHOUT anyone having chosen a scale
  // (`resolveXScale`). Before, `usesXScale` was true only where the user had picked Linear or
  // Timeseries and the panel could grey the rotations out by reading `config.xScale` alone. It now
  // has to resolve the scale the same way this builder does, from the x column's type —
  // `settings/CartesianDisplaySection.tsx`, `valueX`, which calls `resolveXScale` rather than
  // restating it. A config can still CARRY a rotation here, and the panel still shows it rather
  // than pretending it is gone; what it will not do is let one be newly chosen.
  const scaleAxis = {
    type: cfg.xScale === "timeseries" ? ("time" as const) : ("value" as const),
    name: xTitle, nameLocation: "middle" as const, nameGap: 28,
    nameTextStyle: { color: axisLabelColor },
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor },
    axisLine: { show: cfg.xAxisTicks !== false, lineStyle: { color: gridLine } },
    axisTick: { show: cfg.xAxisTicks !== false },
    splitLine: { show: false as const },
  };
  const xCategory = usesXScale(cfg) ? scaleAxis : categoryAxis;
  /** `list` IS THE SERIES ON THIS AXIS, not the metrics behind them — see `onAxis`. A breakout
   *  gives both axes the same one metric, so a metric-name argument could not describe either. */
  const mkValueAxis = (list: SeriesLike[], position?: "left" | "right") => {
    // Whichever of the two fits this axis's orientation makes reachable — exactly one of them can be
    // non-`undefined` on any chart, and both are `undefined` on a card with room to spare. The row
    // chart's answers an interval alone; the vertical one answers the two ends it divides as well,
    // and `verticalValueAxisFit` argues why the horizontal one ought to and does not yet.
    const down = verticalFit(list);
    const measuredInterval = valueInterval ?? down?.interval;
    return {
      // A STABLE HANDLE FOR THE LEGEND PATCH, and the trap it avoids is worth stating in full
      // because it is not a hypothetical — both halves below were measured, not reasoned about.
      //
      // `axisNamesFor` re-titles this axis through a merge `setOption`, and ECharts resolves which
      // existing component a patch entry refers to via `mappingToExists`, which matches on `id`
      // FIRST and on `name` second. The axis's `name` is the very field being patched. With no id,
      // a two-axis patch of `[{name: "rev"}, {name: ""}]` against axes currently named `["", ""]`
      // came back `["", "rev"]` — the title on the wrong side of the chart, after nothing more
      // exotic than toggling one legend entry off and on again.
      //
      // THE ID MUST MATCH ON BOTH SIDES OR IT IS WORSE THAN NEITHER. Removing it from here while
      // `axisNamesFor` still names it does not fall back to position: a patch naming an id no
      // component has makes ECharts CREATE that component. Measured on the dual-axis combo — three
      // y-axes where there were two, the phantom carrying the new title and the real left axis
      // keeping the stale one. `cartesianLegendAxisTitle.echarts.test.tsx` pins the count for
      // exactly this reason.
      //
      // Honest about what is NOT proven: with today's single-entry patch, dropping the id from
      // BOTH sides still lands on the right axis, because ECharts falls through to index matching
      // and index 0 is the left axis. That is a coincidence of this patch's shape — it was the
      // array shape that transposed — so the id is deliberate defence rather than the only thing
      // holding the current code up, and the mutation run records it surviving.
      id: position === "right" ? "yValueRight" : VALUE_AXIS_ID,
      type: cfg.logScale ? ("log" as const) : ("value" as const),
      name: position === "right" ? undefined : yTitle,
      nameLocation: "middle" as const,
      nameTextStyle: { color: axisLabelColor },
      // Clear of the tick labels: a vertical axis's numbers are far wider than a horizontal
      // axis's are tall.
      nameGap: horizontal ? 28 : 44,
      position,
      // Metabase's `graph.y_axis.unpin_from_zero`, and its whole implementation there is one line —
      // `scale: !!settings["graph.y_axis.unpin_from_zero"]` (`echarts/cartesian/option/axis.ts:380`).
      // ECharts' own `scale` is the flag: `false` (its default) forces a value axis to include the
      // origin, `true` lets it fit the data. Nothing is computed, so there is no number to copy and
      // nowhere for one to drift.
      //
      // WRITTEN ONLY WHEN TRUE, which `scatterXY` deliberately does NOT do — see the note there.
      // Pinned is ECharts' default as well as egret's, so writing `scale: false` would say the same
      // thing more loudly while changing the emitted option of every chart in the product; an
      // absence here means "pinned" for the same reason it does upstream.
      ...(unpin ? { scale: true } : {}),
      ...(normalized ? { max: 100 } : {}),
      // The ends the vertical fit's interval divides — see `verticalValueAxisFit`, which explains why
      // ECharts will not round them itself once an `interval` is named, and what the short last step
      // it leaves behind measured on a live card. NOT on a normalised axis, which is 0-100 by
      // construction and divides cleanly already; and outranked below by a range the user set, which
      // is why these are written first.
      ...(down != null && !normalized ? { min: down.min, max: down.max } : {}),
      // Manual axis range (Metabase y-axis min/max) overrides the auto extent.
      ...(cfg.yMin != null ? { min: cfg.yMin } : {}),
      ...(cfg.yMax != null ? { max: cfg.yMax } : {}),
      axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: fmtFor(list) },
      axisLine: { show: cfg.yAxisTicks !== false, lineStyle: { color: gridLine } },
      axisTick: { show: cfg.yAxisTicks !== false },
      // `splitNumber` is a HINT — ECharts still rounds to human intervals, so asking for 5 can
      // yield 4 or 6. Metabase's own control behaves the same way, being the same underlying knob.
      //
      // The user's own tick count wins outright, and takes the hint's looseness with it. `interval` is
      // the measured fallback behind it, and it is `undefined` on every axis either fit has nothing to
      // say about — any card roomy enough for ECharts' own default on the edge that matters, and every
      // unmeasured one. `intervalThatFits` argues why it is an interval rather than a `splitNumber` of
      // our own, and has the measurements; `verticalValueAxisFit` states what the vertical answer does
      // NOT inherit from it.
      //
      // WHICH OF THE TWO IS CONSULTED IS DECIDED BY THE CHART'S ORIENTATION and never by both at once:
      // `valueInterval` is `undefined` off a row chart and `verticalFit` is `undefined` on one, so the
      // `??` picks whichever axis this actually is rather than preferring one.
      ...(cfg.yTickCount != null ? { splitNumber: cfg.yTickCount }
        : measuredInterval != null ? { interval: measuredInterval } : {}),
      // A second (right) axis omits gridlines to avoid doubling them up.
      splitLine: position === "right" ? { show: false } : { lineStyle: { color: gridLine } },
    };
  };

  let xAxis: unknown;
  let yAxis: unknown;
  if (panels) {
    /**
     * ONE CATEGORY AXIS, DRAWN ONCE. Every panel gets an x-axis — it is the coordinate system its
     * series is plotted in, and a grid without one has nothing to position against — but only the
     * LAST of them draws labels, ticks or a title. Metabase's `buildPerPanelXAxes` suppresses the
     * same three fields on the same axes, and for the same reason: the panels are stacked over one
     * shared domain, so drawing it four times says the same thing four times and spends three
     * panels' worth of card doing it.
     *
     * THE AXIS LINE STAYS ON ALL OF THEM (Metabase even darkens it there). It is not a label — it is
     * the baseline each panel's series is measured from, and a panel drawn without one floats.
     * `axisLine` is therefore left exactly as `xCategory` built it rather than suppressed alongside
     * the labels.
     *
     * `nameGap: 0` GOES WITH `name: undefined`. A category axis's `nameGap` is computed above to
     * clear a band of rotated labels this axis is not drawing; leaving it behind on an axis with no
     * name is harmless today and is a reserved gap waiting for the next person who gives these axes
     * a name back.
     */
    xAxis = panels.map((_, i) =>
      i === panels.length - 1
        ? { ...xCategory, gridIndex: i }
        : {
            ...xCategory,
            gridIndex: i,
            name: undefined,
            nameGap: 0,
            axisLabel: { ...(xCategory as { axisLabel?: object }).axisLabel, show: false },
            axisTick: { show: false },
          });
    /**
     * …AND A VALUE AXIS PER PANEL, each sized to the one series in it. `mkValueAxis` is handed a
     * single-series list, so its extent, its number format and its vertical fit are all that
     * series' — which is the whole feature and not a detail of it.
     *
     * A DISTINCT `id` PER AXIS, and this is the trap the merge patch sets. `mkValueAxis` stamps
     * `VALUE_AXIS_ID` ("yValue") on the left axis so `axisNamesFor` can re-title it by id; N panels
     * would mean N components wearing one id, which is ECharts' `mappingToExists` being asked to
     * resolve an ambiguity it resolves by position anyway. Overwritten here rather than parameterised
     * into `mkValueAxis`, because the ONLY reader of that id is a patch this mode does not send —
     * see `axisNamesFor`, which returns nothing at all here.
     *
     * `position` IS LEFT UNSET, which is what puts every panel's axis on the left. Metabase forces
     * `series.axis` to "left" under panels and hides the control; egret cleared `rightAxisMetrics`
     * in `buildCartesianOption`, so no series ever reached `yAxisIndex: 1` and `dual` is false here.
     * Two axes on one plot and a panel per series are two answers to the same question, and running
     * both leaves panels whose axes are on different edges.
     *
     * `name: undefined` ON EVERY ONE OF THEM, and the title is drawn once instead — see
     * `valueAxisName` below. It is not dropped: four axes each carrying `mkValueAxis`'s `yTitle` is
     * what the first version did, and the live 760x560 breakout drew the word "orders" down the left
     * of the card FOUR TIMES. One measure, named once.
     */
    yAxis = panelSeries.map((s, i) => ({
      ...mkValueAxis([s]),
      id: `${VALUE_AXIS_ID}Panel${i}`,
      gridIndex: i,
      name: undefined,
    }));
  } else if (horizontal) {
    xAxis = mkValueAxis(leftSeries);
    yAxis = categoryAxis;
  } else if (dual) {
    xAxis = xCategory;
    yAxis = [mkValueAxis(leftSeries, "left"), mkValueAxis(rightSeries, "right")];
  } else {
    xAxis = xCategory;
    yAxis = mkValueAxis(leftSeries);
  }

  /**
   * EVERY SERIES INTO ITS PANEL. Both indices, because a panel is a grid and a grid is addressed by
   * the pair — binding only `yAxisIndex` would scale the series against the right axis and draw it
   * across the first panel's x.
   *
   * A TREND LINE FOLLOWS ITS SOURCE, through `__of` and never through its own name. `trendSeries`
   * hard-codes `yAxisIndex: s.yAxisIndex ?? 0`, which was right while there was one axis per SIDE
   * and is wrong the moment there is one per series: every trend line would have been drawn in the
   * first panel, against a scale that is not its own. Metabase carries the same link explicitly
   * (`remapTrendLinesToPanels`, keyed on `sourceDataKey`).
   *
   * Anything this map cannot place is LEFT ALONE rather than defaulted to panel 0 — a stack total
   * cannot occur here (stacking is cleared upstream), and a series that somehow escaped `plotted`
   * has no panel to be right about.
   */
  if (panels) {
    const panelOf = new Map<string, number>();
    panelSeries.forEach((s, i) => { if (s.name != null) panelOf.set(s.name, i); });
    for (const s of series as SeriesLike[]) {
      const key = s.__trend ? s.__of : s.name;
      const i = key != null ? panelOf.get(key) : undefined;
      if (i == null) continue;
      (s as unknown as Record<string, unknown>).xAxisIndex = i;
      (s as unknown as Record<string, unknown>).yAxisIndex = i;
    }
  }

  // Goal line — a horizontal reference at goalValue, drawn via a series' markLine.
  //
  // ONE PER PANEL when the chart is split, which is Metabase's own shape (`buildSplitPanelGoalSeries`
  // duplicates the goal into every grid). It is not decoration: each panel has its own value scale,
  // so a goal of 2 sits near the top of a 0-4 panel and on the floor of a 0-4000 one, and a reader
  // shown it in the first panel only would take the second panel's silence for "no goal here".
  // Attached to each PANEL'S OWN series rather than emitted as extra series, because a markLine is
  // already drawn in its host series' coordinate system — the binding above has put each of those in
  // the right grid, so the goal lands there for free.
  if (cfg.goalValue != null && series.length > 0) {
    const markLine = () => ({
      silent: true,
      symbol: "none",
      data: [{ yAxis: cfg.goalValue }],
      label: { formatter: cfg.goalLabel ?? "Goal", position: "insideEndTop", color: axisLabelColor },
      lineStyle: { color: axisLabelColor, type: "dashed" },
    });
    for (const host of panels ? panelSeries : [series[0] as SeriesLike]) {
      (host as unknown as Record<string, unknown>).markLine = markLine();
    }
  }

  // Trend series stay out of the legend.
  const legendNames = series.filter((s) => !(s as SeriesLike).__trend && !(s as SeriesLike).__total).map((s) => (s as SeriesLike).name);
  /**
   * THE LEGEND, IN THE THEME'S OWN INK — which it had never been, on any cartesian chart in the
   * product.
   *
   * MEASURED in a browser on the deployed bundle, at both card sizes and in BOTH THEMES: every
   * legend label rendered at the literal `#54555a` and the scroll pager at `#6d6e73`, byte for byte
   * the same in light and dark, while every axis label and title an inch away resolved
   * `var(--muted-foreground)` properly — `oklch(0.49 0.07 270.49)` in light, `oklch(0.71 0.01
   * 286.09)` in dark. Against the dark card surface (`oklch(0.21 0.01 285.93)`) that constant sits
   * near 2.4:1. A legend label is TEXT, not a non-text indicator, so its bar is 4.5:1 and it was
   * failing by a wide margin in one of the two themes the product ships.
   *
   * WHY IT WAS A CONSTANT AT ALL. It is ECharts' own default, reached because this option named no
   * `textStyle` — `base.ts` records that the shared `bottomLegend`/`sideLegend` builders which used
   * to carry a theme were deleted when the pie stopped being the only caller, and no cartesian
   * legend ever had one of its own. Nothing was overridden; the question was never asked.
   *
   * ECHARTS CANNOT READ A CSS CUSTOM PROPERTY — it paints into a canvas/SVG it owns, and `oklch(...)`
   * only ever reaches it as a resolved string. `axisLabelColor` is that resolution, computed once at
   * the top of this builder and already handed to every axis label, every axis name, the panel
   * titles, the split chart's rotated `graphic` and the goal line. This joins that list rather than
   * opening a second route to the same token; `scatterXY`'s own legend has carried it since it was
   * written, which is the precedent being followed.
   *
   * ALL FOUR KEYS, because the pager is three separate colours and only the first is the label:
   * `pageTextStyle` is the "1/4", `pageIconColor` the arrow that can still be clicked and
   * `pageIconInactiveColor` the one at the end of the run. Leaving any of them would have themed
   * the legend and left the control beside it stranded on the constant this removes.
   */
  const legend = drawLegend
    ? {
        top: 0,
        left: "center" as const,
        type: "scroll" as const,
        textStyle: { color: axisLabelColor },
        pageTextStyle: { color: axisLabelColor },
        pageIconColor: axisLabelColor,
        // The disabled arrow, mixed toward the surface so it reads as unavailable in EITHER theme —
        // a second hand-picked token would have to be re-checked per theme, which is the trap the
        // tooltip's ink ramp already documents at length in `base.ts`.
        pageIconInactiveColor: mixColor(axisLabelColor, resolveColor("var(--card)", COLOR_FALLBACK.card), 0.4),
        ...(legendNames.length !== series.length ? { data: legendNames } : {}),
      }
    : undefined;
  /**
   * THE PANEL TITLES, which are what egret has instead of a legend here — see the `panelSeries` note
   * for why the legend had to go rather than merely being unnecessary.
   *
   * The series' DISPLAY name, so a rename through `seriesLabels` reaches the title exactly as it
   * reaches the tooltip and the aria description; `decorateSeries` has already resolved it into
   * `name`, so this asks nothing of its own.
   *
   * ANCHORED AT THE PANEL'S OWN LEFT EDGE rather than centred, and left-aligned with the plot below
   * it: a centred title over a panel whose left inset is a column of numbers reads as belonging to
   * the card rather than to the panel. `titleTop` comes from the layout, which owns the band it sits
   * in — deriving it here from `top` would be a second copy of `PANEL_TITLE_BAND`.
   */
  const panelTitles = panels?.map((p, i) => ({
    text: panelSeries[i]?.name ?? "",
    top: p.titleTop,
    left: panelGrids?.[i].left,
    // ECharts' `title.padding` DEFAULTS TO 5, which is 5px `PANEL_TITLE_BAND` did not budget for and
    // which put the first title's box through the "40K" label beside it on the live 540x561 chart.
    // Zeroed so the band's arithmetic is the arithmetic — see `PANEL_TITLE_BAND` for the measurement.
    padding: 0,
    textStyle: { fontSize: PANEL_TITLE_FONT, fontWeight: CHART_THEME.labelWeight, color: axisLabelColor },
  }));
  /**
   * THE VALUE AXIS'S NAME, DRAWN ONCE for the whole stack of panels — Metabase's
   * `buildSplitPanelYAxisLabel`, which is a rotated `graphic` text at the vertical centre of every
   * panel, and this is that.
   *
   * WHY NOT THE AXES' OWN `name`. Every panel HAS a value axis, so every one of them would carry the
   * title, and the live 760x560 four-band breakout drew "orders" down the left of the card four
   * times. The measure is the same measure in all four panels; what differs between them is the
   * series, and that is what the panel TITLES say. Metabase reaches the same split — its per-panel
   * axis models are built with `showLabel: false` for exactly this reason (`model/axis.ts`).
   *
   * IT SITS IN THE 26px BAND `panelGrids` ALREADY RESERVES for a value-axis title, centred in it, so
   * nothing about the left inset moves. `rotation: PI/2` is ECharts' counter-clockwise radian, which
   * reads bottom-to-top — the direction an ECharts axis `name` is drawn in, so a chart that has
   * panels and one that does not read the same way round.
   *
   * `graphic` IS NOT A MERGE HAZARD HERE even though it accumulates under a merge `setOption`:
   * `EChartsRenderer` applies the built option with `notMerge: true`, and the only merge patches in
   * the product are the pie's centre overlay and `__axisNamesFor`'s — neither of which this chart
   * sends (`axisNamesFor` returns `{}` under panels).
   */
  const valueAxisName = panels && yTitle
    ? [{
        type: "text" as const,
        left: (CHART_THEME.grid.left as number) + 13,
        top: (panels[0].top + panels[panels.length - 1].top + panels[panels.length - 1].height) / 2,
        rotation: Math.PI / 2,
        style: {
          text: yTitle,
          fill: axisLabelColor,
          fontSize: CHART_THEME.axisFontSize,
          fontWeight: CHART_THEME.labelWeight,
          align: "center" as const,
          verticalAlign: "middle" as const,
        },
      }]
    : undefined;

  // Formatted tooltip — one row per (non-trend) series, value via its metric's format.
  // Series may be renamed, so resolve the source metric from each series' __metric.
  const metricByName = new Map<string, string>();
  for (const s of series as SeriesLike[]) {
    if (s.name != null && s.__metric != null) metricByName.set(s.name, s.__metric);
  }
  const tipFmt = (name: string, v: number) => {
    if (normalized) return `${Math.round(v)}%`;
    const m = metricByName.get(name) ?? (cfg.yMetrics.includes(name) ? name : cfg.yMetrics[0]);
    return m ? formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]) : String(v);
  };
  type TipParam = { axisValueLabel?: string; name?: string; seriesName?: string; value?: unknown; marker?: string };
  const tooltipFormatter = (params: TipParam | TipParam[]) => {
    const arr = Array.isArray(params) ? params : [params];
    const visible = arr.filter((p) => p.value != null && p.seriesName && !p.seriesName.endsWith(" trend"));
    if (visible.length === 0) return "";
    const header = visible[0].axisValueLabel ?? visible[0].name ?? "";
    // On a value/time x-axis the datum is an [x, y] pair — take y.
    const yOf = (v: unknown) => (Array.isArray(v) ? Number(v[1]) : Number(v));
    const lines = visible.map((p) => `${p.marker ?? ""} ${p.seriesName ?? ""}: <b>${tipFmt(p.seriesName ?? "", yOf(p.value))}</b>`);
    /**
     * The additional tooltip metrics, AFTER every plotted series and with no marker of their own —
     * Metabase renders them `isSecondary: true` (`CartesianChart/events.ts:385`) for the same
     * reason: a swatch is the reader's link back to something drawn on the chart, and these are the
     * rows with nothing drawn to link to.
     *
     * `p.name` FIRST and `axisValueLabel` only as the fallback, which is the opposite order from
     * `header` one line up and is deliberate. They agree on an ordinal axis; on a value or time one
     * `axisValueLabel` is the formatted x COORDINATE while `name` is the datum's own raw category,
     * which is what this map is keyed on. Reading the header's own answer here would find nothing on
     * exactly the charts `applyXScale` touches — the silent half of the miss, since a tooltip that
     * simply omits the rows looks like a chart that was never configured for them.
     */
    const extras = tooltipExtras?.get(String(visible[0].name ?? visible[0].axisValueLabel ?? ""));
    for (const e of extras ?? []) {
      lines.push(`${displayTitle(cfg, e.metric)}: <b>${
        formatValue(e.value, cfg.metricTypes?.[e.metric] ?? "double", cfg.columnSettings?.[e.metric])
      }</b>`);
    }
    return `${header}<br/>${lines.join("<br/>")}`;
  };

  // The first plotted series carries the values a screen reader hears; the rest are named
  // in the count, since reading every series of a multi-series chart is worse than useless.
  const described = (series as SeriesLike[]).filter((s) => !s.__trend && !s.__total);

  /**
   * The description, for a given legend selection — `undefined` meaning "as built".
   *
   * WHY THIS IS A FUNCTION AND NOT A STRING. ECharts' legend is a live control that hides series
   * without the option being rebuilt, and the description was baked once at build time. Confirmed
   * in a browser on the deployed "MB · Combo": clicking `orders` off left the chart drawing revenue
   * alone (its 0-400 axis gone) while the container's `aria-label` still read "Combo chart of
   * orders … 2 series; January 2026 205, …" — named for the hidden series, counting it, and
   * enumerating its values. Same shape as the pie's `toggledOffSlices`, one family over.
   *
   * `EChartsRenderer` calls this from `legendselectchanged` and merges the result back in. The
   * alternative — dropping the enumeration from any chart that HAS a legend — was rejected: it
   * pays for a state most readers never enter with a permanent loss for every reader, and the
   * enumeration is the only part of this sentence carrying numbers.
   *
   * With everything hidden, `head` is `undefined` and `ariaDescription` says "No data." — which is
   * the same answer the `metric: head?.name` line below already gives for a saved all-hidden
   * curation, now reached by the ephemeral route too.
   */
  const ariaFor = (selected?: Record<string, boolean>) => {
    const shown = selected == null
      ? described
      : described.filter((s) => s.name == null || selected[s.name] !== false);
    const first = shown[0];
    /**
     * WHAT THE CHART IS OF — the metric under a breakout, the first series otherwise.
     *
     * MEASURED on all three breakout cards, at both sizes and in both themes: "MB · Bar Breakout"
     * announced "Bar chart of DOOHICKEY by order_month. 12 values: 4 series; …" and then read
     * Doohickey's twelve numbers, while the chart drew revenue broken out by category — four bands
     * and forty-eight points. "MB · Bar Breakout Desc", whose bands sort the other way, said "of
     * Widget". Whichever band happened to sort first was being announced as the chart's subject.
     *
     * `first?.name` IS STILL RIGHT WITHOUT A BREAKOUT, which is why this is a branch and not a
     * replacement: on the combo card the first series IS a metric (`orders`), the legend says so,
     * and the values enumerated below are its own. The nineteen non-breakout cards were swept clean
     * and none of them moves.
     *
     * UNDER A BREAKOUT THE ANSWER ALREADY EXISTED and was never asked for: every band carries the
     * same `__metric`, so `metricsOf` returns exactly one column and `metricTitle` resolves it
     * through the same rename ladder that titles the value axis — which is why this card's own axis
     * was correctly labelled "revenue" the whole time the description said "Doohickey".
     *
     * #400's principle is that a description must describe what is DRAWN. A recent sweep found the
     * pie naming slices it was not drawing and fixed it by asking the same question of the drawn
     * arcs (`rootSlices`); this is that generalisation reaching the family one over — ask the
     * option what it is showing rather than taking the first thing in a list.
     *
     * No fallback to `cfg.yMetrics[0]` on either arm: with every series hidden there is no metric
     * being described, and naming one would tell a screen reader about a chart nobody can see.
     */
    const subject = hasBreakout ? (metricTitle(metricsOf(shown)) ?? first?.name) : first?.name;
    return ariaDescription({
      kind: `${cfg.kind === "line" ? "Line" : cfg.kind === "area" ? "Area" : cfg.kind === "row" ? "Row" : cfg.kind === "scatter" ? "Scatter" : cfg.kind === "combo" ? "Combo" : "Bar"} chart`,
      metric: subject,
      /**
       * WHICH SERIES THE NUMBERS BELOW BELONG TO, stated only when it is not already the subject.
       *
       * Naming the metric alone would have traded a wrong subject for an unattributed enumeration:
       * a reader would hear "Bar chart of revenue … 4 series" and then twelve numbers with nothing
       * saying they are Doohickey's rather than the chart's. So the band is stated instead of
       * dropped.
       *
       * The condition is what keeps this off the swept cards by construction rather than by a
       * second `hasBreakout` test: on a combo the subject IS the first series' name, the two agree,
       * and `ariaDescription` emits the sentence it emitted before this change, byte for byte.
       */
      seriesName: first?.name !== subject ? first?.name : undefined,
      // The dimension AS THE AXIS TITLE NAMES IT, not the raw column name. `titleOf` is the same
      // rename ladder `xTitle` is built from — but read past `showXLabel`, because hiding the title
      // takes a label off the card and should not take the dimension out of the description with it:
      // the bars are still there and still grouped by that column.
      dimension: cfg.xLabel ?? titleOf(cfg.xAxis),
      // What is SHOWN, not what was built: a count that keeps saying "2 series" over one drawn
      // series is the same lie as naming the hidden one.
      seriesCount: shown.length,
      // OVER THE PLOTTED SERIES, which is what `fmtFor` takes now. It read `cfg.yMetrics`, and the
      // two agree on every chart except one where a metric is hidden: there the description used to
      // be formatted through a column that is not on the card. Same direction as `seriesCount`
      // above — describe what is shown.
      format: (v) => fmtFor(plotted)(Number(v)),
      // `drawnCategories`, NOT `categories.map(String)` — the raw bucket is what this enumerated
      // until now, so a month-bucketed axis drew "January 2026" while a screen reader heard
      // "2026-01-01". `ariaDescription`'s own header says it exists to state "what a sighted user
      // sees", and `drawLabel` is the single closure that decides what that is (the fitter and the
      // axis formatter already read it, for the same reason).
      //
      // THE TOOLTIP DOES NOT MOVE WITH IT, and that is deliberate rather than an oversight: its
      // header is `params.axisValueLabel`, which ECharts builds from `axis.scale.getLabel`
      // (`component/axisPointer/viewHelper.js`) — on an OrdinalScale that returns the `xAxis.data`
      // entry verbatim and never consults `axisLabel.formatter` — so the tooltip does still read the
      // raw bucket. `options/waterfall.ts`'s tooltip formatter carries the argument for leaving it
      // there: a tooltip sits next to a drill and what a click FILTERS by is the raw bucket, so
      // naming the period on one while the other filters on the value trades a cosmetic mismatch for
      // a semantic one. A description has no click behind it, which is why it moves and that does not.
      //
      // READ OFF THE SERIES, NOT OFF THE CATEGORY LIST. This used to be
      // `drawnCategories.map((name, i) => ({ name, value: first.data?.[i] }))`, which was wrong in
      // two independent ways on a value/time x-axis and right on an ordinal one:
      //
      //   1. `Number(first.data[i])` is `NaN` for the `{value: [x, y], name}` item object a datum
      //      became in #387 — measured live during #411, a scatter read "15.69 NaN, 19.87 NaN, …"
      //      for every point. `datumY` is the single unwrap for that, introduced in #397 for
      //      `widestOf` and never picked up here.
      //   2. `applyXScale` FILTERS: a datum whose x will not parse is dropped, so `first.data` is
      //      no longer index-aligned with `drawnCategories` and the zip silently paired each
      //      survivor with the NEXT label's value. That does not read as `NaN` — it reads as a
      //      correct-looking sentence, which is why it outlived the defect that was visible.
      //
      // The item object carries its own raw category as `name` (the whole reason it is an object —
      // see `ScaledPoint`), so the pairing comes from the datum itself and cannot slide. That name
      // is the RAW value, so it goes back through `drawnByRaw` to reach the string the axis draws;
      // an ordinal datum is a bare number with no name and keeps the positional read it always had.
      points: first ? ariaPoints(first.data ?? [], drawnCategories, drawnByRaw) : [],
    });
  };

  /**
   * The value axis's title as a MERGE PATCH, for a given legend selection.
   *
   * Same defect as `ariaFor`'s, one component over, and confirmed in a browser before it was
   * written rather than inferred from the shape: on the deployed "MB · Combo", clicking `orders`
   * off left ECharts drawing revenue alone and dropping the left axis's tick labels entirely (no
   * series, no extent, no numbers) while the rotated title "orders" stayed painted beside the empty
   * axis — its `<text>` box even shifted from x=573 to x=549 as the plot expanded into the space
   * the numbers vacated. A title naming a series that is not on the chart, next to an axis carrying
   * nothing.
   *
   * WHY A PATCH RATHER THAN A STRING. Which component holds the value axis depends on the chart:
   * a row chart swaps the two, so its `yAxis` is the CATEGORY axis and writing the metric name
   * there would overwrite the dimension's title. Returning the shape to merge keeps that decision
   * here, beside the `if (horizontal)` that made it, instead of restating it in the renderer.
   *
   * ADDRESSED BY `id`, never by position — `mkValueAxis` carries the measurement of what happens
   * without one. Only the left axis appears in the patch: an id-matched merge leaves every
   * component it does not name alone, so the right axis (never titled) needs no restating.
   *
   * `""` RATHER THAN `undefined` for "no title". An explicit `undefined` is not a reliable way to
   * clear a value already set through a merge; `""` is, and it is what ECharts' own model reports
   * for an axis nobody named, so "clear it" and "it was never set" end in the same state.
   */
  const axisNamesFor = (selected?: Record<string, boolean>) => {
    /**
     * NOTHING TO SAY UNDER SPLIT PANELS, and returning `{}` rather than a patch is load-bearing in
     * two directions even though no legend event can fire in that mode.
     *
     * There is no component wearing `VALUE_AXIS_ID` there — every panel axis carries
     * `yValuePanel<i>` instead — and `mkValueAxis` records what a patch naming an id nothing wears
     * actually does: ECharts CREATES that component. Measured on the dual-axis combo as a phantom
     * third y-axis carrying the new title while the real axis kept the stale one. So the failure
     * mode of getting this wrong is not "the title does not update", it is a spurious axis on the
     * card.
     *
     * And the answer would be wrong even if the id matched: `yTitleFor` narrows to a single metric's
     * name over the whole chart, which is a question about ONE shared axis. With a panel per series
     * the axes do not share a title to narrow.
     *
     * `EChartsRenderer` merges this into the patch it is building, so an empty object simply adds
     * nothing and the aria update it already holds still goes out.
     */
    if (splitting) return {};
    const axis = { id: VALUE_AXIS_ID, name: yTitleFor(selected) ?? "" };
    return horizontal ? { xAxis: axis } : { yAxis: axis };
  };

  return {
    aria: ariaFor(),
    /** Read by `EChartsRenderer` on `legendselectchanged` — see `ariaFor`. Not an ECharts key: it
     *  is read off the object this function returns, before that object reaches `setOption`. */
    __ariaFor: ariaFor,
    /** Read by the same handler, off the same event — see `axisNamesFor`. Also not an ECharts key. */
    __axisNamesFor: axisNamesFor,
    color: undefined,
    // The product's own typeface, at the option root — this is the one builder that composes its
    // whole option itself rather than spreading base.ts's `baseOption()`, so it has to say so.
    textStyle: baseTextStyle(),
    grid: panelGrids ?? grid,
    tooltip: { trigger: cfg.kind === "scatter" ? "item" : "axis", ...tooltipSurface(), formatter: tooltipFormatter },
    // The crosshair spans every panel, so hovering one reads the same category in all of them —
    // which is the point of stacking panels over one shared domain rather than drawing four charts.
    // Only written when there are panels to link; on every other chart it is absent as it always was.
    ...(panels ? { axisPointer: { link: [{ xAxisIndex: "all" }] } } : {}),
    legend,
    // Absent, not empty, off a split chart — `TitleComponent` is registered for these and an empty
    // array would still ask ECharts to walk it on every chart in the product.
    ...(panelTitles ? { title: panelTitles } : {}),
    ...(valueAxisName ? { graphic: valueAxisName } : {}),
    xAxis,
    yAxis,
    series,
  };
}
