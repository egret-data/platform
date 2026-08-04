import type { CartesianConfig, CartesianViz } from "@/api/charts";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { DecimalInput } from "./DecimalInput";
import { resolveXScale, stackModeOf, stackModePatch, type StackMode } from "@/components/chart/echarts/options/cartesian";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";
import { AutoToggle } from "./AutoToggle";
import { xTicksChoiceOf, xTicksPatch, type XTicksChoice } from "./ChartAxesSection";

function Toggle({ id, label, checked, onChange }: { id: string; label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

interface SectionProps {
  kind: CartesianViz;
  config: CartesianConfig;
  onChange: (patch: Partial<CartesianConfig>) => void;
}

/**
 * Stacking, the chart-wide line-shape/missing-value/marker choices (line and area only),
 * per-series colour/label, and the combo per-metric series type. Per-series style and size
 * live in their own control (SeriesLineStyleSection) so they can hide behind "More options".
 */
/*
 * `defaultSeriesColor` used to live here. It took `(hasBreakout, count, index)` and re-derived a
 * colour rule from the INDEX — which is how a panel comes to offer a swatch the chart does not draw:
 * once the builder keyed colours on the series' NAME (`colorsForKeys`), every single-series card
 * showed `#689735` in the picker and something else on screen, and a user could "fix" a colour that
 * was never wrong.
 *
 * Its replacement is not a fixed version of it. `SeriesListControl` (viz/cartesian.tsx) calls
 * `colorsForKeys` directly, once for the whole list, exactly as `PieDisplaySection` does — because a
 * wrapper here would be a SECOND name for the renderer's colour rule, and the next person to reach
 * for it to cover a case the mapping does not would get a swatch that no longer matches the chart.
 * That is the same argument `decorateSeries` makes for not re-reading `seriesColors` after the
 * mapping has folded it in.
 */

export function SeriesOptions({ kind, config, onChange }: SectionProps & { seriesNames?: string[] }) {
  // Combo stacks too — Metabase's own per-series display setting hides itself on
  // `stackable.stack_type`, which only means anything because a combo can reach that state there.
  // egret excluded combo, so the guard that hides the display select was unreachable and a combo
  // could not stack at all.
  const canStack = kind === "bar" || kind === "row" || kind === "area" || kind === "combo";
  const isLineish = kind === "line" || kind === "area";
  return (
    <div className="space-y-2.5">
      {/* Three mutually exclusive states, offered as one choice — Metabase's own Stacking control.
          Two switches ("Stacked", then "100% stacked" appearing under it) made the reader discover
          the third state by turning the first one on, and let the panel store `normalized` while
          `stacked` was off: a combination no control could show and the renderer ignored. */}
      {canStack && (
        <div className="space-y-1">
          <Label className="text-sm font-normal">Stacking</Label>
          <Select value={stackModeOf({ kind, stacked: config.stacked, normalized: config.normalized })}
            onValueChange={(v) => onChange(stackModePatch(v as StackMode))}>
            <SelectTrigger aria-label="Stacking" className="h-7 w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="none">Don&apos;t stack</SelectItem>
              <SelectItem value="stacked">Stack</SelectItem>
              <SelectItem value="normalized">Stack — 100%</SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}
      {canStack && stackModeOf({ kind, stacked: config.stacked, normalized: config.normalized }) === "stacked" && (
        <Toggle id="cz-stacktotal" label="Stack totals" checked={!!config.showStackTotal} onChange={(v) => onChange({ showStackTotal: v })} />
      )}
      {isLineish && (
        <div className="flex items-center justify-between gap-2">
          <Label className="text-sm font-normal">Line shape</Label>
          <Select value={config.lineShape ?? "linear"} onValueChange={(v) => onChange({ lineShape: v as CartesianConfig["lineShape"] })}>
            <SelectTrigger aria-label="Line shape" className="h-7 w-24"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="linear">Linear</SelectItem>
              <SelectItem value="smooth">Smooth</SelectItem>
              <SelectItem value="step">Step</SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}
      {isLineish && (
        <div className="flex items-center justify-between gap-2">
          <Label className="text-sm font-normal">Missing values</Label>
          <Select value={config.missing ?? "interpolate"} onValueChange={(v) => onChange({ missing: v as CartesianConfig["missing"] })}>
            <SelectTrigger aria-label="Missing values" className="h-7 w-28"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="interpolate">Connect</SelectItem>
              <SelectItem value="zero">Zero</SelectItem>
              <SelectItem value="none">Gap</SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}
      {isLineish && (
        <div className="flex items-center justify-between gap-2">
          <Label htmlFor="cz-markers" className="text-sm font-normal">Markers</Label>
          {/* A plain three-option Select bound directly to the string, like Line shape and
              Missing values above — NOT AutoToggle. AutoToggle's value is boolean|undefined;
              translating "auto" through it maps that literal to `false` ("Off"), and nothing
              ever un-writes a value that was never wrong to store in the first place. */}
          <Select value={config.markers ?? "auto"} onValueChange={(v) => onChange({ markers: v === "auto" ? undefined : (v as CartesianConfig["markers"]) })}>
            <SelectTrigger id="cz-markers" aria-label="Markers" className="h-7 w-24"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="auto" title="Auto shows markers only when there are few enough points to read">Auto</SelectItem>
              <SelectItem value="on">On</SelectItem>
              <SelectItem value="off">Off</SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}

      {/* The per-series display moved onto the series row itself (SeriesListSection), where it sits
          with that series' colour and rename — one row per series, the way Metabase has it. It used
          to be a THIRD parallel list keyed by metric, printing every name again. */}
    </div>
  );
}

/** X-axis scale, the left/right axis split for multi-metric charts, and the axis titles. */
export function AxesOptions({ kind, config, onChange, xColumnType }: SectionProps & {
  /** The SQL type of the result column on `config.xAxis` — see the identical prop on
   *  `AxisRangeOptions`. Here it decides what the X-axis-scale trigger READS on a chart whose scale
   *  was resolved rather than stored. */
  xColumnType?: string;
}) {
  // On a horizontal chart the CATEGORY axis is the vertical one, so naming these controls after
  // the config keys (`xLabel` is the category axis, `yLabel` the value axis) prints them backwards
  // from what the reader sees: "Y min" would govern the horizontal extent. Metabase relabels the
  // same settings by the axis they appear on; so do these.
  const horiz = kind === "row" || !!config.horizontal;
  const catAx = horiz ? "Y" : "X";   // the dimension axis
  const valAx = horiz ? "X" : "Y";   // the measure axis
  return (
    <div className="space-y-2.5">
      {/*
        WHY A SCATTER NOW GETS THIS SELECT. It was excluded while the exclusion was inert: a scatter
        always drew a category axis, which is what the hidden select would have said, so hiding it
        cost nothing. `resolveXScale` ended that — a scatter over a numeric dimension is drawn on a
        VALUE axis that nobody chose, and with the select hidden the automatic answer was also the
        only answer, under a rotation select greyed out with an explanation and no lever to act on.

        Metabase computes the same default and SHOWS it: `ScatterPlot.tsx` spreads
        `GRAPH_AXIS_SETTINGS`, which carries `graph.x_axis.scale` with `persistDefault: true`. egret
        shows it without persisting — the resolution stays at render time, which is this programme's
        standing rule and what lets a saved chart pick up a better default later — so the two differ
        only in what is written to the database, not in what the user sees or can change.

        STILL HIDDEN for the METRIC-VS-METRIC scatter, and that is not the same exclusion: that
        chart leaves through `scatterXY`, which builds its own two value axes and never reads
        `xScale` at all. Offering the select there would store a setting nothing consults — the very
        defect the rotation rows below are greyed out to avoid.
      */}
      {(kind !== "scatter" || config.xMetric == null) && kind !== "row" && !config.horizontal && (
        <div className="flex items-center justify-between gap-2">
          <Label className="text-sm font-normal">X axis scale</Label>
          {/* The RESOLVED scale, not the stored one. Reading `config.xScale` here would print
              "Ordinal" underneath a chart visibly drawing a value axis, which is the same
              stored-says-one-thing-screen-says-another defect the "Auto" Y-axis rows below were
              added to fix. Off a resolved chart the two are identical, since `resolveXScale`
              returns a stored scale unchanged. */}
          <Select value={resolveXScale(kind, config.xScale, xColumnType) ?? "ordinal"} onValueChange={(v) => onChange({ xScale: v as CartesianConfig["xScale"] })}>
            <SelectTrigger aria-label="X axis scale" className="h-7 w-28"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="ordinal">Ordinal</SelectItem>
              <SelectItem value="linear">Linear</SelectItem>
              <SelectItem value="timeseries">Timeseries</SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}

      {config.yMetrics.length > 1 && !config.horizontal && kind !== "row" && (
        <div className="space-y-1.5 border-t pt-2">
          <div className="text-xs text-muted-foreground">Y axis</div>
          {/* "Auto" is the resting state, not "Left". With the automatic split turned on and nothing
              assigned by hand, the chart can perfectly well draw a metric on the right — and these
              selects used to say "Left" underneath a chart that was visibly doing otherwise.
              Choosing either side turns the split off for the whole chart, which is what an
              explicit assignment means. */}
          {config.yMetrics.map((m) => {
            const assigned = (config.rightAxisMetrics ?? []).length > 0;
            const onRight = (config.rightAxisMetrics ?? []).includes(m);
            return (
              <div key={m} className="flex items-center justify-between gap-2">
                <Label className="truncate text-sm font-normal">{m}</Label>
                <Select value={!assigned ? "auto" : onRight ? "right" : "left"} onValueChange={(v) => {
                  if (v === "auto") { onChange({ rightAxisMetrics: [] }); return; }
                  const rest = (config.rightAxisMetrics ?? []).filter((x) => x !== m);
                  onChange({ rightAxisMetrics: v === "right" ? [...rest, m] : rest });
                }}>
                  <SelectTrigger aria-label={`axis ${m}`} className="h-7 w-24"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="auto">Auto</SelectItem>
                    <SelectItem value="left">Left</SelectItem>
                    <SelectItem value="right">Right</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            );
          })}
        </div>
      )}

      {/* Metabase pairs each axis title with its own Show label switch. Kept apart from the text so
          hiding a title does not make the user retype it to bring it back — with the text alone as
          the control, "off" meant an empty box and the wording was gone for good. */}
      <div className="space-y-2 border-t pt-2">
        <div className="space-y-1">
          <div className="flex items-center justify-between gap-2">
            <Label htmlFor="cz-xlabel" className="text-sm font-normal">{catAx} label</Label>
            <Switch id="cz-show-xlabel" aria-label={`Show ${catAx} label`} checked={config.showXLabel !== false}
              onCheckedChange={(v) => onChange({ showXLabel: v ? undefined : false })} />
          </div>
          {config.showXLabel !== false && (
            <Input id="cz-xlabel" aria-label={`${catAx} label`} className="h-8" placeholder="Auto"
              value={config.xLabel ?? ""} onChange={(e) => onChange({ xLabel: e.target.value })} />
          )}
        </div>
        <div className="space-y-1">
          <div className="flex items-center justify-between gap-2">
            <Label htmlFor="cz-ylabel" className="text-sm font-normal">{valAx} label</Label>
            <Switch id="cz-show-ylabel" aria-label={`Show ${valAx} label`} checked={config.showYLabel !== false}
              onCheckedChange={(v) => onChange({ showYLabel: v ? undefined : false })} />
          </div>
          {config.showYLabel !== false && (
            <Input id="cz-ylabel" aria-label={`${valAx} label`} className="h-8" placeholder="Auto"
              value={config.yLabel ?? ""} onChange={(e) => onChange({ yLabel: e.target.value })} />
          )}
        </div>
      </div>
    </div>
  );
}

/** Horizontal toggle — vertical bar only; row is always horizontal so has nothing to toggle. */
export function OrientationOption({ kind, config, onChange }: SectionProps) {
  const isBar = kind === "bar";
  return isBar ? (
    <Toggle id="cz-horizontal" label="Horizontal" checked={!!config.horizontal} onChange={(v) => onChange({ horizontal: v })} />
  ) : null;
}

/** Manual axis bounds and log scale — not meaningful on a scatter's continuous axes. */
export function AxisRangeOptions({ kind, config, onChange, xColumnType }: SectionProps & {
  /** The SQL type of the result column on `config.xAxis`, which decides whether the x is drawn as a
   *  value axis when no scale is stored (`resolveXScale`). Optional: a caller with no result yet
   *  simply cannot know, and `valueX` below says what it does with that. */
  xColumnType?: string;
}) {
  // On a horizontal chart the CATEGORY axis is the vertical one, so naming these controls after
  // the config keys (`xLabel` is the category axis, `yLabel` the value axis) prints them backwards
  // from what the reader sees: "Y min" would govern the horizontal extent. Metabase relabels the
  // same settings by the axis they appear on; so do these.
  const horiz = kind === "row" || !!config.horizontal;
  const catAx = horiz ? "Y" : "X";   // the dimension axis
  const valAx = horiz ? "X" : "Y";   // the measure axis
  // Is the chart's x a VALUE axis rather than the ordinal category one? Follows `usesXScale` in
  // echarts/options/cartesian.ts, plus one case it does not cover: the METRIC-VS-METRIC scatter,
  // which leaves through `scatterXY` before the category axis is ever built. Both halves of that
  // arm are load-bearing — a scatter WITHOUT an `xMetric` falls through to `baseOption` like any
  // other chart, so testing `kind` alone would grey out a live setting on every category-x scatter.
  // A horizontal chart is not one of these either: its categories run down the left and ECharts
  // rotates a y-axis label too.
  //
  // The scale is RESOLVED, not read: since PR 03 a scatter over a numeric dimension gets a value
  // axis with nothing stored on `xScale` at all, so `config.xScale === "linear"` no longer answers
  // the question. `resolveXScale` is the builder's own function, called rather than restated — a
  // second copy of that rule here is exactly how a control ends up greyed out on charts that would
  // have honoured it, or live on charts that will not. `xColumnType` comes from the last RESULT,
  // which is the same list the builder resolves against; before a query has run it is undefined and
  // this falls back to the stored scale, which is the honest answer when the column's type is not
  // yet known.
  const xScale = resolveXScale(kind, config.xScale, xColumnType);
  const valueX = (!horiz && (xScale === "linear" || xScale === "timeseries"))
    || (kind === "scatter" && config.xMetric != null);
  return (
    <div className="space-y-2.5">
      {kind !== "scatter" && <Toggle id="cz-log" label="Log scale" checked={!!config.logScale} onChange={(v) => onChange({ logScale: v })} />}
      {/* Metabase's `graph.x_axis.axis_enabled`, a FIVE-choice key of which this select offers four
          — Hide / Show / Rotate 45° / Rotate 90° — projected onto `xAxisTicks` + `xLabelRotation`
          through the SHARED mapping the waterfall/box-plot tab uses (`xTicksChoiceOf`/
          `xTicksPatch`). It replaces a switch, because rotation was the part egret had no way to
          ask for at all: the builder now measures the labels and picks an angle, and without this
          control there was no way to overrule it.

          THE FIFTH, "Compact", IS DEFERRED RATHER THAN DISMISSED. It shortens temporal axis labels,
          and the reason it is absent from the waterfall/box-plot tab — their x is always a category
          run through its column format — does NOT hold here: cartesian is the one family with a
          `timeseries` x-scale, which is exactly where a temporal shortener would bite. But that
          axis is `scaleAxis`, whose tick labels ECharts generates rather than this builder, so
          offering Compact means owning that tick generation first. THAT IS NOT PR 03 EITHER — this
          said "that is PR 03", and PR 03 turned out to be about which axis a scatter gets, not about
          laying out the ticks on it. Nothing owns it yet, and PR 03's side-by-side (Task 5) should
          record the divergence rather than paper over it.

          What "Hide" hides is the axis LINE and its TICK MARKS — which is what this row is named
          after — and NOT the labels, where Metabase's single `axis_enabled: false` drops both. The
          divergence is pre-existing and shared with the other two charts that ship this select (the
          waterfall and the box plot) and is deliberately left alone: widening it would change what
          every already-saved chart with the axis off renders. Naming the row for the lines and ticks
          rather than for the axis keeps the wording honest about the narrower meaning; a label
          reading just "X-axis" would not be.

          Stored as `false` only when turned off, so a chart saved before these existed keeps
          drawing both. */}
      <div className="space-y-1">
        <div className="flex items-center justify-between gap-2">
          <Label htmlFor="cz-xticks" className="text-sm font-normal">{catAx} axis lines and ticks</Label>
          <Select value={xTicksChoiceOf(config)} onValueChange={(v) => onChange(xTicksPatch(v as XTicksChoice))}>
            {/* The reason the rotations are greyed out hangs off the TRIGGER, which is focusable and
                keeps its pointer events, and is read out by `aria-describedby`. It cannot hang off
                the disabled items themselves: `data-[disabled]:pointer-events-none` (ui/select.tsx)
                means a mouse never hovers them so a native `title` never fires, Radix skips disabled
                items in its roving focus so a keyboard never lands on one, and a screen reader is
                told only `aria-disabled`. A `title` there would have looked like an explanation and
                been reachable by nobody. */}
            <SelectTrigger
              id="cz-xticks" aria-label={`${catAx} axis lines and ticks`} className="h-7 w-28"
              aria-describedby={valueX ? "cz-xticks-rotation" : undefined}
            ><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="hide">Hide</SelectItem>
              <SelectItem value="show">Show</SelectItem>
              {/* Offered but DISABLED where the rotation would be stored and then never applied: a
                  linear/timeseries x and a metric-vs-metric scatter both draw a VALUE axis, whose
                  labels are ticks ECharts generates rather than the category array, and `scaleAxis`
                  in echarts/options/cartesian.ts carries no `rotate`.

                  IT COULD, and this used to claim otherwise — "rotating generated ticks means
                  generating them first, which is PR 03" is wrong twice over. ECharts applies
                  `axisLabel.rotate` to any axis kind (`AxisBuilder.js:875` reads it with no branch
                  on the axis), so nothing has to be generated first; and PR 03 does not own tick
                  generation, it only decides which axis a scatter gets. The reason `scaleAxis`
                  carries no `rotate` is PARITY: Metabase greys the rotation out on a value axis too,
                  so honouring it would be a divergence dressed up as a fix. A secondary reason, real
                  but thinner in a product this young, is that charts saved as Linear can still carry
                  a rotation from before the switch, and applying it would silently turn their labels.

                  Disabled rather than hidden, for two reasons: a chart that stored 45° and later
                  moved to Linear must still SHOW what it stored instead of an empty trigger, and
                  options that appear and disappear as a neighbouring select changes read as a glitch
                  rather than as a rule. */}
              {(["rotate45", "rotate90"] as const).map((v) => (
                <SelectItem key={v} value={v} disabled={valueX}>
                  {v === "rotate45" ? "Rotate 45°" : "Rotate 90°"}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        {/* Visible, not a tooltip: the two greyed-out rows are invisible until the select is opened,
            so an explanation gated on hovering or focusing something inside it is reachable only
            after the reader has already hit the wall. A caption states the rule before it is met,
            and it costs a line only in the state that needs one. */}
        {valueX && (
          <p id="cz-xticks-rotation" className="text-xs text-muted-foreground">
            Rotation applies to a category axis; this chart plots values across.
          </p>
        )}
      </div>
      {/* Only meaningful with two or more metrics on one axis to separate. Metabase gates it the
          same way, and computes the split rather than asking which metric goes where — egret's
          per-metric Axis selects still win when they name anything. */}
      {kind !== "scatter" && config.yMetrics.length > 1 && (
        <Toggle id="cz-autosplit" label="Split y-axis when necessary" checked={config.autoSplitY !== false}
          onChange={(v) => onChange({ autoSplitY: v ? undefined : false })} />
      )}
      <Toggle id="cz-yticks" label={`${valAx} axis lines and ticks`} checked={config.yAxisTicks !== false}
        onChange={(v) => onChange({ yAxisTicks: v ? undefined : false })} />
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor="cz-ytickcount" className="text-sm font-normal">{valAx} tick marks</Label>
        <Input
          id="cz-ytickcount" aria-label={`${valAx} tick marks`} type="number" min={2} max={20} step={1}
          className="h-7 w-20" placeholder="Auto"
          value={config.yTickCount ?? ""}
          onChange={(e) => {
            const n = Number(e.target.value);
            onChange({ yTickCount: e.target.value === "" || Number.isNaN(n) ? undefined : n });
          }}
        />
      </div>
      {/* The same pair ChartAxesSection offers the waterfall/box-plot/sankey side, and the same
          reason for DecimalInput over `type="number"`: a bound is a data value, and a number input
          goes EMPTY the instant a decimal point is the last character typed. Both copies are fixed
          together — fixing one is how the defect came to live in two files. */}
      {kind !== "scatter" && (
        <div className="grid grid-cols-2 gap-2 border-t pt-2">
          <label className="space-y-1">
            <span className="text-xs text-muted-foreground">{valAx} min</span>
            <DecimalInput className="h-8" placeholder="auto"
              value={config.yMin}
              onChange={(yMin) => onChange({ yMin })} />
          </label>
          <label className="space-y-1">
            <span className="text-xs text-muted-foreground">{valAx} max</span>
            <DecimalInput className="h-8" placeholder="auto"
              value={config.yMax}
              onChange={(yMax) => onChange({ yMax })} />
          </label>
        </div>
      )}
    </div>
  );
}

/**
 * Chart chrome: whether a legend renders, data labels, and shortening large axis/label
 * numbers (25.5K instead of 25,468). Legend lives here rather than in Series because it's
 * about whether a piece of chrome renders at all, not about an individual series.
 */
export function FormatOptions({ config, onChange }: SectionProps) {
  return (
    <div className="space-y-2.5">
      <AutoToggle
        id="cz-legend"
        label="Legend"
        value={config.showLegend}
        onChange={(v) => onChange({ showLegend: v })}
        autoHint="Auto shows the legend once there's a breakout or more than one metric"
      />
      <Toggle id="cz-labels" label="Data labels" checked={!!config.showLabels} onChange={(v) => onChange({ showLabels: v })} />
      {config.showLabels && (
        <div className="flex items-center justify-between gap-2">
          <Label htmlFor="cz-label-density" className="text-sm font-normal">Data label density</Label>
          <Select value={config.labelDensity === "all" ? "all" : "fit"} onValueChange={(v) => onChange({ labelDensity: v as "fit" | "all" })}>
            <SelectTrigger id="cz-label-density" aria-label="Data label density" className="h-7 w-24"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="fit">Fit</SelectItem>
              <SelectItem value="all">All</SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}
      <AutoToggle
        id="cz-compact"
        label="Shorten large numbers"
        value={config.compactValues}
        onChange={(v) => onChange({ compactValues: v })}
        autoHint="Auto shortens once the axis numbers get wide"
      />
    </div>
  );
}

/** Goal (reference line) value and its label. */
export function GoalOptions({ config, onChange }: SectionProps) {
  return (
    <div className="grid grid-cols-2 gap-2 pt-2">
      {/* A goal is a data value — a 4.5% conversion target, a 1250.75 revenue line — so it takes
          DecimalInput for the same reason the axis bounds do. */}
      <label className="space-y-1">
        <span className="text-xs text-muted-foreground">Goal value</span>
        <DecimalInput className="h-8" placeholder="none"
          value={config.goalValue}
          onChange={(goalValue) => onChange({ goalValue })} />
      </label>
      <label className="space-y-1">
        <span className="text-xs text-muted-foreground">Goal label</span>
        <Input className="h-8" placeholder="Goal" value={config.goalLabel ?? ""} onChange={(e) => onChange({ goalLabel: e.target.value || undefined })} />
      </label>
    </div>
  );
}

/** Least-squares trend line overlay — not offered on scatter (no series to fit along an axis). */
export function TrendOption({ kind, config, onChange }: SectionProps) {
  return kind !== "scatter" ? (
    <Toggle id="cz-trend" label="Trend line" checked={!!config.trendLine} onChange={(v) => onChange({ trendLine: v })} />
  ) : null;
}

export function CartesianDisplaySection({
  kind, config, onChange, seriesNames = [], xColumnType,
}: {
  kind: CartesianViz;
  config: CartesianConfig;
  onChange: (patch: Partial<CartesianConfig>) => void;
  seriesNames?: string[];
  /** Passed straight through to `AxisRangeOptions` and `AxesOptions` — see the prop's doc there. */
  xColumnType?: string;
}) {
  return (
    <div className="space-y-2.5 pt-1.5">
      <SeriesOptions kind={kind} config={config} onChange={onChange} seriesNames={seriesNames} />
      <OrientationOption kind={kind} config={config} onChange={onChange} />
      <TrendOption kind={kind} config={config} onChange={onChange} />
      <AxisRangeOptions kind={kind} config={config} onChange={onChange} xColumnType={xColumnType} />
      <AxesOptions kind={kind} config={config} onChange={onChange} xColumnType={xColumnType} />
      <GoalOptions kind={kind} config={config} onChange={onChange} />
      <FormatOptions kind={kind} config={config} onChange={onChange} />
    </div>
  );
}
