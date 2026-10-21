import { pruneUnits } from "@/components/chart/temporal-units";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { FieldZone } from "@/components/chart/config/FieldZone";
import { columnLabels, displayName, fieldTitle, metricLabels } from "@/components/chart/config/field-names";
import {
  SeriesOptions, CartesianAxesTab, OrientationOption, FormatOptions, GoalOptions, TrendOption,
} from "@/components/chart/settings/CartesianDisplaySection";
import { colorsForKeys } from "@/components/chart/echarts/keyColors";
import { SeriesListSection } from "@/components/chart/settings/SeriesListSection";
import {
  cartesianSeriesNames, stackModeOf, seriesDisplayFor, echartsTypeOf, perSeriesDisplayApplies,
  type SeriesDisplay,
} from "@/components/chart/echarts/options/cartesian";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from "@/components/ui/select";
import type { CartesianConfig, CartesianViz, ChartOrderBy, QuerySpec } from "@/api/charts";
import { cartesianFromParams, cartesianToSettings, CARTESIAN_PREFIX } from "./cartesian-codec";
import type { SharedQueryState, VizControl, VizControlProps, VizDescriptor } from "./types";

import { FIELD_LABEL as LABEL_CLASS } from "@/components/chart/settings/FieldLabel";

/**
 * WHAT A FIELD IS CALLED, ON EVERY `<Select>` IN THIS FILE.
 *
 * `displayName`/`fieldTitle` come from `config/field-names.ts` — the same two functions the
 * `FieldZone` beside these controls has used since #444, which is the whole point. Before this,
 * the Y-axis zone offered "Khu vực" and the X-axis select offered `region`, and they sit in the
 * same 255px column: the state #444 identified as the defect, in a file it did not touch.
 *
 * THE VALUE IS ALWAYS THE MACHINE NAME. `value={c.column_name}` is untouched and always will be
 * — a chart's stored config keys on the column the query returns, so renaming a column in the
 * dataset must never rewrite a saved chart. `displayName` changes only the child text, and
 * Radix's `SelectValue` renders that same child into the trigger, so the closed control reads
 * the friendly name for free.
 *
 * `title` carries `Khu vực (region)` on hover, matching the chip in `FieldZone` and the row in
 * the table's `CheckList`. A SECOND LINE was rejected here even though those two use one: a
 * `SelectItem` is a fixed-height row inside a scrolled popover, and doubling the height of the
 * renamed rows only would make the list go ragged where the zone's popover can afford it.
 */
function XAxisControl({ config, onChange, columns, unitRow }: VizControlProps<CartesianConfig>) {
  const labels = columnLabels(columns);
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>X-axis</Label>
        <Select value={config.xAxis ?? ""} onValueChange={(v) => onChange({ xAxis: v || undefined })}>
          <SelectTrigger className="w-full text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
          <SelectContent>
            {columns.map((c) => (
              <SelectItem key={c.column_name} value={c.column_name} title={fieldTitle(c.column_name, labels)}>
                {displayName(c.column_name, labels)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {unitRow(config.xAxis)}
    </>
  );
}

function XMetricControl({ config, onChange, metrics }: VizControlProps<CartesianConfig>) {
  const labels = metricLabels(metrics);
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>X metric (optional)</Label>
      <Select value={config.xMetric ?? "__none"} onValueChange={(v) => onChange({ xMetric: v === "__none" ? undefined : v })}>
        <SelectTrigger className="w-full text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="__none">None — use the dimension</SelectItem>
          {metrics.map((m) => (
            <SelectItem key={m.metric_name} value={m.metric_name} title={fieldTitle(m.metric_name, labels)}>
              {displayName(m.metric_name, labels)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

/** Metabase's "Bubble size": a third metric sizing each point. Only meaningful on a true
 *  metric-vs-metric scatter, so it hides until an x metric is chosen. */
function BubbleMetricControl({ config, onChange, metrics }: VizControlProps<CartesianConfig>) {
  const labels = metricLabels(metrics);
  if (!config.xMetric) return null;
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Bubble size</Label>
      <Select value={config.bubbleMetric ?? "__none"} onValueChange={(v) => onChange({ bubbleMetric: v === "__none" ? undefined : v })}>
        <SelectTrigger aria-label="Bubble size" className="w-full text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="__none">None — every point the same size</SelectItem>
          {metrics.map((m) => (
            <SelectItem key={m.metric_name} value={m.metric_name} title={fieldTitle(m.metric_name, labels)}>
              {displayName(m.metric_name, labels)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

function YAxisControl({ config, onChange, metrics }: VizControlProps<CartesianConfig>) {
  return (
    <FieldZone label="Y-axis" selected={config.yMetrics} onChange={(v) => onChange({ yMetrics: v })}
      labels={metricLabels(metrics)}
      options={metrics.map((m) => m.metric_name)} />
  );
}

/**
 * Metabase's "Additional tooltip columns", in the one namespace where egret can mean it: dataset
 * METRICS fetched for the tooltip and plotted nowhere. `CartesianConfig.tooltipMetrics` carries the
 * argument for why it is metrics rather than columns; this control is the picking half.
 *
 * THE OPTION LIST IS THE COMPLEMENT OF WHAT THE CHART PLOTS — Metabase's own
 * `getAvailableAdditionalColumns`, which subtracts the columns the chart already references
 * (`shared/settings/cartesian-chart.ts:458-476`). A metric already on the Y-axis is not an
 * ADDITIONAL one, and a row repeating a series' own number is worse than the feature's absence.
 * On a scatter the x metric and the bubble metric come out too, for the same reason and because the
 * tooltip already prints both. `FieldZone` subtracts the chosen ones itself.
 *
 * HIDDEN WHEN THERE IS NOTHING TO OFFER **AND** NOTHING TO SHOW, which is Metabase's
 * `getHidden: (rawSeries, vizSettings) => getAvailableAdditionalColumns(...).length === 0`
 * (`lib/settings/graph.ts:431`) plus the guarantee that makes it safe to port.
 *
 * PR #431 objected to controls that vanish as a neighbouring select changes, and the objection is
 * right about the case it names — a control disappearing WITH A VALUE IN IT, leaving a setting that
 * paints and cannot be unset. Metabase never meets that case, and not by luck: its widget reads
 * `getValue: getComputedAdditionalColumnsValue`, which filters the stored list down to what is
 * still available before anything sees it (`cartesian-chart.ts:494-498`), so a value it hides is a
 * value it has already dropped. egret does NOT filter the stored value — a chart's settings are
 * stored JSON and quietly rewriting one at render time is how a user's config loses things without
 * saying so — which leaves the second clause as the way to keep the rule honest here. The two
 * arrive at the same behaviour by different routes, and "always present, sometimes empty" would
 * have been the egret divergence rather than the parity.
 *
 * The threshold is reached only on a one-metric dataset, or on a chart already plotting every
 * metric it has; both are states in which the row could say nothing at all.
 *
 * UNDER A BREAKOUT IT SAYS SO RATHER THAN DISAPPEARING, and that is the same distinction one more
 * time. The builder cannot draw these rows with a `series` column (see `extraTooltipMetrics`) and
 * `buildSpec` declines to fetch them, so the honest thing is a sentence — the shape `SeriesControl`
 * eight lines down already uses for its own "only the first metric is plotted". The chips stay
 * removable, which is exactly what hiding the control would have taken away.
 */
function TooltipMetricsControl({ config, onChange, metrics }: VizControlProps<CartesianConfig>) {
  const plotted = new Set<string>(config.yMetrics);
  if (config.xMetric) plotted.add(config.xMetric);
  if (config.bubbleMetric) plotted.add(config.bubbleMetric);
  const selected = config.tooltipMetrics ?? [];
  const options = metrics.map((m) => m.metric_name).filter((m) => !plotted.has(m));
  if (options.length === 0 && selected.length === 0) return null;
  return (
    <div className="space-y-1">
      <FieldZone label="Tooltip metrics" selected={selected} onChange={(v) => onChange({ tooltipMetrics: v })}
        labels={metricLabels(metrics)}
        options={options} />
      {config.series
        ? <p className="text-xs text-muted-foreground">Not shown while the chart has a breakout — each
          value would belong to one series, not to the category.</p>
        : <p className="text-xs text-muted-foreground">Fetched and shown in the tooltip only.</p>}
    </div>
  );
}

function SeriesControl({ config, onChange, columns }: VizControlProps<CartesianConfig>) {
  const labels = columnLabels(columns);
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Series</Label>
      <Select value={config.series ?? "__none"} onValueChange={(v) => onChange({ series: v === "__none" ? undefined : v })}>
        <SelectTrigger className="w-full text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="__none">None</SelectItem>
          {columns.filter((c) => c.column_name !== config.xAxis)
            .map((c) => (
              <SelectItem key={c.column_name} value={c.column_name} title={fieldTitle(c.column_name, labels)}>
                {displayName(c.column_name, labels)}
              </SelectItem>
            ))}
        </SelectContent>
      </Select>
      {config.series && config.yMetrics.length > 1 && (
        <p className="text-xs text-muted-foreground">With a series, only the first metric is plotted.</p>
      )}
    </div>
  );
}

function MaxCategoriesControl({ config, onChange }: VizControlProps<CartesianConfig>) {
  return (
    <div className="space-y-1">
      <Label htmlFor="cz-maxcat" className={LABEL_CLASS}>Max categories</Label>
      <Input id="cz-maxcat" type="number" min={2} className="text-sm"
        placeholder="Keep every category"
        value={config.maxCategories ?? ""}
        onChange={(e) => {
          const n = Number(e.target.value);
          onChange({ maxCategories: e.target.value === "" || Number.isNaN(n) ? undefined : n });
        }} />
      <p className="text-xs text-muted-foreground">The rest are summed into “Other”.</p>
    </div>
  );
}

// Series names come from the last result, not from anything precomputed — the shell must not know
// cartesian-specific shape.
function SeriesListControl({ kind, config, onChange, resultColumns, rows }: VizControlProps<CartesianConfig> & { kind: CartesianViz }) {
  const names = cartesianSeriesNames(resultColumns, rows, config);
  if (names.length === 0) return null;
  /**
   * The swatch each row shows before any override — `buildCartesianOption`'s own answer, asked with
   * the same function and the same keys, so the picker cannot offer a colour the chart does not
   * draw. (It used to re-derive a rule from the row's index and hand a lone series the summarize
   * green, which the renderer stopped drawing once colours keyed on the series' NAME.)
   *
   * ONE CALL FOR THE WHOLE LIST, hoisted out of the rows deliberately — `SeriesListSection` asks for
   * a default once per rendered row, so a per-row call would redo the sort, the hash and the ledger
   * N times per render, on every unrelated edit in the panel. `PieDisplaySection` hoists its own for
   * the same reason. It is also the honest shape: the assignment is a decision about the whole set
   * (a collision moves one key along, a pinned colour takes its slot out of circulation) and cannot
   * be made a series at a time.
   *
   * EVERY name goes in, hidden series included. `applySeriesOrder` drops the hidden ones AFTER
   * `colorsForKeys` has run, so hiding one does not move the rest — the opposite of the pie, whose
   * hidden slices are dropped before its ring is built. `config.seriesColors` goes in as `existing`
   * because the builder passes it there too: a pinned colour consumes its slot, so leaving it out
   * would disagree with the chart about every OTHER series the moment one was pinned.
   */
  const swatchColors = colorsForKeys(names, config.seriesColors);
  return (
    <SeriesListSection
      names={names}
      order={config.seriesOrder}
      hidden={config.hiddenSeries}
      colors={config.seriesColors}
      labels={config.seriesLabels}
      types={config.seriesTypes}
      /**
       * WHICH KINDS OFFER THE SELECT AT ALL — Metabase's `series.display` `getHidden`, both arms.
       *
       * It used to read `kind === "combo"`, and Metabase's own list is `["line", "area", "bar",
       * "combo"]`: on a line chart there, one series can be flipped to bar without turning the whole
       * chart into a combo. THAT WAS NOT ONLY A PANEL GATE, which a probe settled before the gate
       * moved — `seriesDisplayFor` consulted `seriesTypes` on `combo` alone and returned the chart
       * kind for the rest, so a stored per-series display on a line chart was DISCARDED by the
       * builder (measured: `seriesDisplayFor("line", "w", 1, {w: "bar"})` -> `"line"`, both bands
       * `type: "line"`). Opening this alone would have shipped a control that saves and does not
       * paint. The builder learned the other three kinds in the same PR; `perSeriesDisplayApplies`
       * is its answer to this question, asked rather than restated.
       *
       * Row and scatter stay out for Metabase's reason: a row chart IS a horizontal bar and a
       * scatter IS points, so there is no alternative to offer.
       *
       * HIDDEN WHILE STACKED, which is Metabase's second arm — a stack of mixed shapes is not a
       * thing anyone can read. egret keeps it in the PANEL only, deliberately: withholding the
       * control hides a choice, whereas refusing the stored value in the builder would throw a
       * user's display away the moment they turned stacking on.
       */
      defaultDisplay={perSeriesDisplayApplies(kind) && stackModeOf({ kind, stacked: config.stacked, normalized: config.normalized }) === "none"
        // The BUILDER's own default for a series nobody has set — one call, so the combo's
        // first-is-a-line rule cannot come to be spelled differently in the two places.
        ? (i) => seriesDisplayFor(kind, "", i, undefined) as SeriesDisplay
        : undefined}
      defaultColor={(name) => swatchColors[name]}
      seriesLine={config.seriesLine}
      /**
       * Which rows offer the per-series drill-in. Line appearance is all the popover holds, and
       * `decorateSeries` applies `cfg.seriesLine` only to a series whose ECharts type is "line" —
       * so the button belongs exactly where that is true, asked through the builder's OWN two
       * functions rather than restated as "line or area". A combo series flipped to Bar on the row
       * beside it loses its button on the next render, which is Metabase's measured behaviour (its
       * popover hides the line rows on a `bar` series) arrived at from the same rule.
       *
       * This also widens the reach of the setting: the old `seriesLineStyle` control was declared
       * for the line and area KINDS only, so a combo's line bands could carry per-series style and
       * had no control for it — the same unreachable-setting defect one level up.
       */
      hasLineSettings={(name, i) => echartsTypeOf(seriesDisplayFor(kind, name, i, config.seriesTypes)) === "line"}
      onChange={onChange}
    />
  );
}

/**
 * The Display tab's controls, grouped under named headings and scoped to what this kind
 * actually plots — bar gets orientation and stacking, line does not get orientation.
 */
function displayControls(kind: CartesianViz): VizControl<CartesianConfig>[] {
  // Row is already the horizontal cartesian kind — it has no orientation to choose, so unlike
  // OrientationOption's internal isBar gate (which just renders nothing for row), the control
  // itself is not declared for row at all.
  const showOrientation = kind === "bar";
  return [
    {
      id: "seriesOptions", section: "display", group: "Series",
      // The series NAMES travel in, and not because this section prints them — it does not. They
      // decide whether the Stacking control is offered at all: Metabase hides `stackable.stack_type`
      // once one stackable series is left, and with a breakout the count is a property of the LAST
      // RESULT rather than of the config. The same call `SeriesListControl` makes, for the same set.
      render: ({ config, onChange, resultColumns, rows }: VizControlProps<CartesianConfig>) => (
        <SeriesOptions
          kind={kind} config={config} onChange={onChange}
          seriesNames={cartesianSeriesNames(resultColumns, rows, config)}
        />
      ),
    },
    {
      id: "seriesList", section: "display", group: "Series",
      render: (p: VizControlProps<CartesianConfig>) => <SeriesListControl kind={kind} {...p} />,
    },
    // `seriesLineStyle` used to be declared here for line/area, `advanced: true`, rendering an
    // N-row grid of Selects behind the Series group's "More options". It is gone: the same
    // settings now live in the per-series drill-in on the row itself (SeriesSettingsPopover), so
    // there is no second place to reach them from and no disclosure to find first.
    ...(showOrientation ? [{
      id: "orientation", section: "display" as const, group: "Axes",
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <OrientationOption kind={kind} config={config} onChange={onChange} />
      ),
    }] : []),
    {
      // ONE control for the whole tab, where there used to be two ("axes" and "range"). Not a
      // merge of their settings — `CartesianAxesTab` still asks each of them for its own rows — but
      // the two axes' rows interleave across the pair, so nothing below the tab can group them by
      // axis. Splitting them was also never a disclosure: the shell renders the Axes tab flat, so
      // "range"'s `advanced` flag had no effect there.
      id: "axes", section: "display", group: "Axes",
      // `resultColumns`, not `columns`: the x-axis rotation rule and the x-axis-scale trigger both
      // turn on the type the BUILDER will resolve against, and the builder reads the result's
      // columns. The dataset's own list is the same types by a different route until a chart is
      // pointed at an expression or the dataset drifts from the last run, at which point the two
      // disagree and the result is the one on screen.
      render: ({ config, onChange, resultColumns, rows }: VizControlProps<CartesianConfig>) => (
        <CartesianAxesTab
          kind={kind} config={config} onChange={onChange}
          xColumnType={resultColumns.find((c) => c.name === config.xAxis)?.type}
          // The Y-axis-position rows are one per SERIES now, not one per metric — a breakout's
          // bands can each choose a side. Same call the Series group makes, for the same set;
          // under a breakout it is the only thing that can name them, since they come from the data.
          seriesNames={cartesianSeriesNames(resultColumns, rows, config)}
        />
      ),
    },
    {
      id: "format", section: "display", group: "Format",
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <FormatOptions kind={kind} config={config} onChange={onChange} />
      ),
    },
    {
      id: "goal", section: "display", group: "Format", advanced: true,
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <GoalOptions kind={kind} config={config} onChange={onChange} />
      ),
    },
    {
      id: "trend", section: "display", group: "Format", advanced: true,
      render: ({ config, onChange }: VizControlProps<CartesianConfig>) => (
        <TrendOption kind={kind} config={config} onChange={onChange} />
      ),
    },
  ];
}

/**
 * Metabase's `add-implicit-breakout-order-by`, ported rather than invented
 * (`query_processor/middleware/add_implicit_clauses.clj`): "Fields specified in `breakout` should
 * add an implicit ascending `order-by` subclause *unless* that Field is already *explicitly*
 * referenced in `order-by`."
 *
 * Two details taken from the implementation, not from the docstring, because they are what make
 * the two clauses compose instead of fight:
 *
 *  - It APPENDS. `add-implicit-breakout-order-by` reduces `lib/order-by` over the breakouts on top
 *    of whatever the author already wrote, so `ORDER BY revenue DESC` on a chart broken out by
 *    category becomes `revenue DESC, category ASC`. The author's clause decides the drawn order;
 *    the breakout's decides ties, which is what keeps the result deterministic between two calls —
 *    the whole point of PR #394.
 *  - It skips a field already referenced IN ANY DIRECTION. `lib/order-by` compares on
 *    `order-by-distinct-key`, which is the expression alone (order_by.cljc:104: "Ignore duplicate
 *    order bys with different directions… the second sort won't change anything"). So sorting
 *    descending BY the breakout replaces the implicit ascending clause rather than being defeated
 *    by it.
 *
 * With no authored sort this returns exactly what PR #394 emitted — ascending by the x-axis, or
 * nothing at all when there is no x-axis.
 *
 * EVERY BREAKOUT, NOT JUST THE X AXIS, and that is the third detail taken from the implementation
 * rather than the docstring. `add-implicit-breakout-order-by` REDUCES over `(lib/breakouts query)` —
 * all of them — so a question grouped by category AND by product line comes out of the middleware
 * with `ORDER BY category ASC, product_line ASC`, and `add-order-bys-for-breakouts-test` pins it.
 * This took only the x-axis, which is why a chart's BANDS came back in engine order however many
 * times PR #394's fix ran: the rows were deterministic in the category and arbitrary within it.
 *
 * The reduce order is the breakout order (x, then series), which matters: the FIRST clause decides
 * the drawn order and the rest break its ties. That is what makes the x axis still the primary key
 * on a chart the author has not sorted, and what `preserveSeriesOrder` in `ChartRenderer` reads when
 * an author moves the breakout column to the front.
 *
 * This is the data-level half only. It reaches charts saved AFTER it and no others — a dashboard
 * deserialises the STORED spec verbatim — so `sortBands` in the renderer is the half that reaches
 * everything already saved. Both halves, for the same reason PR #394 needed both.
 */
function withImplicitBreakoutOrder(authored: ChartOrderBy[], breakouts: (string | undefined)[]): ChartOrderBy[] {
  return breakouts.reduce<ChartOrderBy[]>((acc, breakout) => {
    if (!breakout) return acc;
    if (acc.some((o) => o.field === breakout)) return acc;
    return [...acc, { field: breakout, desc: false }];
  }, authored);
}

export function cartesianDescriptor(kind: CartesianViz): VizDescriptor<CartesianConfig> {
  const controls: VizControl<CartesianConfig>[] = [
    { id: "x", section: "data", render: XAxisControl },
    ...(kind === "scatter" ? [
      { id: "xMetric", section: "data" as const, render: XMetricControl },
      { id: "bubbleMetric", section: "data" as const, render: BubbleMetricControl },
    ] : []),
    { id: "y", section: "data", render: YAxisControl },
    { id: "series", section: "data", render: SeriesControl },
    /**
     * ON THE DATA TAB, WHERE METABASE FILES ITS TWIN UNDER DISPLAY, and the divergence is the whole
     * shape of the port rather than a slip. `graph.tooltip_columns` is `getSection: () => t`Display``
     * (`lib/settings/graph.ts:421`) and belongs there: it picks among columns the result ALREADY
     * has and does not touch the query — measured by instrumenting `fetch`/`XHR` before opening the
     * sidebar (zero requests on selection) and by the `dataset_query` staying byte-identical while
     * `visualization_settings` gained the key. egret's picks a metric the QUERY MUST BE CHANGED TO
     * FETCH, which makes it a query input; query inputs live here, beside the Y-axis zone whose
     * list this one is the complement of.
     *
     * IT IS ALSO WHAT KEEPS A DASHBOARD CARD OUT OF IT, and that is not a side effect worth losing.
     * `applyVizOverride` merges a card's override into `settings` and states outright that "the
     * query `spec` is never overridden" — so a card able to set this would store a metric its own
     * stored spec never fetched and paint nothing, which is the save-and-do-not-paint defect this
     * programme has closed eight times, reached through a surface nobody would think to check.
     * `CardVizPanel` renders the Display section whole and narrows the Data section to the controls
     * flagged `cardOverridable`; declaring this one on Data and leaving that flag off is what makes
     * the card panel decline it — by the existing rule, not by a special case.
     */
    { id: "tooltipMetrics", section: "data", render: TooltipMetricsControl },
    // Scatter's x-axis is a metric, not a category to fold into "Other" — nothing to bind
    // maxCategories to there, and the fold code is never reached for a metric-vs-metric plot.
    ...(kind !== "scatter" ? [{ id: "maxCategories", section: "data" as const, render: MaxCategoriesControl }] : []),
    ...displayControls(kind),
  ];

  return {
    key: kind,
    meta: CHART_TYPES.find((t) => t.key === kind)!,
    prefix: CARTESIAN_PREFIX,
    defaultConfig: { yMetrics: [] },
    fromParams: cartesianFromParams,
    toSettings: cartesianToSettings,
    requiredFields: (cfg) => {
      const out: string[] = [];
      if (!cfg.xAxis) out.push("X-axis");
      if (!cfg.yMetrics.length) out.push("a metric");
      return out;
    },
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => {
      const dims = [cfg.xAxis, cfg.series].filter((d): d is string => !!d);
      /**
       * `?? []` AND NOT `cfg.yMetrics`, which is not the belt-and-braces it looks like. The type
       * says `yMetrics: string[]` and every path through the studio honours it, but the harness
       * generator hands `buildSpec` a deliberately degenerate `{}` to prove the guard that refuses
       * a chart with no `cartesian.*` settings still fires (`scripts/harness/params.test.ts`,
       * "refuses to emit a chart with no settings in its own namespace"). The old line read
       * `metrics: mets` and passed the `undefined` straight through — a type lie that survived only
       * because nothing ever spread it. Appending the tooltip metrics does spread it, so the
       * degenerate config started throwing `plotted is not iterable` HERE, before the harness could
       * reach its own error, and the test that asserts the message matches /cartesian\./ failed
       * against a TypeError. Defaulting keeps that guard reachable and makes the returned spec
       * match its own type for the first time.
       */
      const ys = cfg.yMetrics ?? [];
      const plotted = kind === "scatter" && cfg.xMetric
        ? [cfg.xMetric, ...ys, ...(cfg.bubbleMetric ? [cfg.bubbleMetric] : [])]
        : ys;
      /**
       * THE TOOLTIP METRICS, APPENDED — the whole cost of the feature, and the reason it is a day
       * rather than a week. `spec.metrics` is a list of DATASET METRIC NAMES: the engine resolves
       * each through `find_metric` and emits one `<expression> AS <name>` per entry
       * (`chartquery.rs:529`), taking the grouping from `spec.dimensions` alone. So a name added
       * here is another aggregate over the SAME groups — one more column in the result, the same
       * number of rows, the same chart. The builder then draws series from `cfg.yMetrics` and never
       * from the result's columns, so a metric that is in this list and not in that one is fetched
       * and drawn nowhere. That is the hook; the tooltip is what reads it.
       *
       * AFTER the plotted ones, so a chart's stored spec keeps the column order it had and any
       * reader indexing the result positionally is unaffected.
       *
       * DEDUPED against everything already asked for. A metric can reach both keys — pick it as a
       * tooltip metric, then add it to the Y-axis — and the same name twice would be SELECTed twice
       * under one alias, which is an engine error from a state the picker can reach.
       *
       * NOT FETCHED AT ALL UNDER A BREAKOUT, which is the half that keeps this honest. With a
       * `series` column the builder cannot draw these rows (`extraTooltipMetrics` says why: the
       * value is per (x, band) and egret's cartesian tooltip is `trigger: "axis"`), and fetching a
       * column in order to discard it is precisely the defect this PR's other half fixes on the
       * scatter. The two decisions are one decision and have to be spelled the same way in both
       * places.
       */
      const extras = cfg.series
        ? []
        : (cfg.tooltipMetrics ?? []).filter((m, i, all) =>
          !plotted.includes(m) && all.indexOf(m) === i);
      return {
        metrics: [...plotted, ...extras], dimensions: dims, columns: [], filters: shared.filters,
        // The query asks for the order the chart draws in, so the ROWS are deterministic — not
        // just the axis. Without it the engine returns the groups in whatever order it likes, and
        // tooltips, drills, exports and whichever rows survive `row_limit` all disagree between
        // one load and the next.
        //
        // Ascending by the dimension, because that is Metabase's own default. A fresh
        // "summarise by" question carries no `order-by` in its MBQL at all; the query processor
        // adds one implicitly, `:asc`, on every breakout column before the query runs
        // (`add-implicit-breakout-order-by`, add_implicit_clauses.clj). Descending by the
        // aggregation is what a question's AUTHOR asks for, never what the builder emits. It is
        // also the order egret already chose in `pivot/engine.ts` and in smartscalar (`simple.tsx`).
        //
        // An author CAN now ask for one, through `shared.orderby`, and `withImplicitBreakoutOrder`
        // is where the two meet — the same precedence Metabase's own middleware applies. What the
        // renderer then does with a non-empty `orderby` is the other half of honouring it; see
        // `preserveCategoryOrder` in `echarts/options/cartesian.ts`.
        orderby: withImplicitBreakoutOrder(
          // A ROW CHART RANKS: with no sort asked for it is ordered by its first metric, largest
          // first, as the gallery promises (D7, KI-045). That is also what makes a Row limit of 10
          // return the top ten rather than the first ten names. An authored sort still wins, and
          // the breakouts still follow as tie-breakers so equal values come back in a fixed order.
          shared.orderby.length === 0 && kind === "row" && ys[0] ? [{ field: ys[0], desc: true }] : shared.orderby,
          // `dims` itself, which is already "the breakouts, in breakout order" and already filtered
          // to the strings — deriving the list twice is how the two come to disagree about which
          // fields the query groups by.
          dims,
        ),
        row_limit: shared.rowLimit, rollup_groups: [],
        temporal_units: pruneUnits(shared.temporalUnits, dims),
      };
    },
    controls,
  };
}
