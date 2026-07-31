import type { CartesianConfig, CartesianViz } from "@/api/charts";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { seriesColor, SUMMARIZE_COLOR } from "@/components/chart/echarts/palette";
import { stackModeOf, stackModePatch, type StackMode } from "@/components/chart/echarts/options/cartesian";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";
import { AutoToggle } from "./AutoToggle";

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
/** The colour a series shows before any override: a lone (non-breakout) metric gets the summarize
 *  green, everything else cycles the accent palette — mirrors the renderer's own choice. */
export function defaultSeriesColor(hasBreakout: boolean, count: number, i: number): string {
  return !hasBreakout && count === 1 ? SUMMARIZE_COLOR : seriesColor(i);
}

export function SeriesOptions({ kind, config, onChange }: SectionProps & { seriesNames?: string[] }) {
  const canStack = kind === "bar" || kind === "row" || kind === "area";
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
export function AxesOptions({ kind, config, onChange }: SectionProps) {
  return (
    <div className="space-y-2.5">
      {kind !== "scatter" && kind !== "row" && !config.horizontal && (
        <div className="flex items-center justify-between gap-2">
          <Label className="text-sm font-normal">X axis scale</Label>
          <Select value={config.xScale ?? "ordinal"} onValueChange={(v) => onChange({ xScale: v as CartesianConfig["xScale"] })}>
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
            <Label htmlFor="cz-xlabel" className="text-sm font-normal">X label</Label>
            <Switch id="cz-show-xlabel" aria-label="Show X label" checked={config.showXLabel !== false}
              onCheckedChange={(v) => onChange({ showXLabel: v ? undefined : false })} />
          </div>
          {config.showXLabel !== false && (
            <Input id="cz-xlabel" aria-label="X label" className="h-8" placeholder="Auto"
              value={config.xLabel ?? ""} onChange={(e) => onChange({ xLabel: e.target.value })} />
          )}
        </div>
        <div className="space-y-1">
          <div className="flex items-center justify-between gap-2">
            <Label htmlFor="cz-ylabel" className="text-sm font-normal">Y label</Label>
            <Switch id="cz-show-ylabel" aria-label="Show Y label" checked={config.showYLabel !== false}
              onCheckedChange={(v) => onChange({ showYLabel: v ? undefined : false })} />
          </div>
          {config.showYLabel !== false && (
            <Input id="cz-ylabel" aria-label="Y label" className="h-8" placeholder="Auto"
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
export function AxisRangeOptions({ kind, config, onChange }: SectionProps) {
  return (
    <div className="space-y-2.5">
      {kind !== "scatter" && <Toggle id="cz-log" label="Log scale" checked={!!config.logScale} onChange={(v) => onChange({ logScale: v })} />}
      {/* Metabase's "Show lines and tick marks", one per axis. Stored as `false` only when turned
          off, so a chart saved before these existed keeps drawing both. */}
      <Toggle id="cz-xticks" label="X axis lines and ticks" checked={config.xAxisTicks !== false}
        onChange={(v) => onChange({ xAxisTicks: v ? undefined : false })} />
      {/* Only meaningful with two or more metrics on one axis to separate. Metabase gates it the
          same way, and computes the split rather than asking which metric goes where — egret's
          per-metric Axis selects still win when they name anything. */}
      {kind !== "scatter" && config.yMetrics.length > 1 && (
        <Toggle id="cz-autosplit" label="Split y-axis when necessary" checked={config.autoSplitY !== false}
          onChange={(v) => onChange({ autoSplitY: v ? undefined : false })} />
      )}
      <Toggle id="cz-yticks" label="Y axis lines and ticks" checked={config.yAxisTicks !== false}
        onChange={(v) => onChange({ yAxisTicks: v ? undefined : false })} />
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor="cz-ytickcount" className="text-sm font-normal">Y tick marks</Label>
        <Input
          id="cz-ytickcount" aria-label="Y tick marks" type="number" min={2} max={20} step={1}
          className="h-7 w-20" placeholder="Auto"
          value={config.yTickCount ?? ""}
          onChange={(e) => {
            const n = Number(e.target.value);
            onChange({ yTickCount: e.target.value === "" || Number.isNaN(n) ? undefined : n });
          }}
        />
      </div>
      {kind !== "scatter" && (
        <div className="grid grid-cols-2 gap-2 border-t pt-2">
          <label className="space-y-1">
            <span className="text-xs text-muted-foreground">Y min</span>
            <Input className="h-8" type="number" placeholder="auto"
              value={config.yMin ?? ""}
              onChange={(e) => { const n = Number(e.target.value); onChange({ yMin: e.target.value === "" || Number.isNaN(n) ? undefined : n }); }} />
          </label>
          <label className="space-y-1">
            <span className="text-xs text-muted-foreground">Y max</span>
            <Input className="h-8" type="number" placeholder="auto"
              value={config.yMax ?? ""}
              onChange={(e) => { const n = Number(e.target.value); onChange({ yMax: e.target.value === "" || Number.isNaN(n) ? undefined : n }); }} />
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
      <label className="space-y-1">
        <span className="text-xs text-muted-foreground">Goal value</span>
        <Input className="h-8" type="number" placeholder="none"
          value={config.goalValue ?? ""}
          onChange={(e) => { const n = Number(e.target.value); onChange({ goalValue: e.target.value === "" || Number.isNaN(n) ? undefined : n }); }} />
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
  kind, config, onChange, seriesNames = [],
}: {
  kind: CartesianViz;
  config: CartesianConfig;
  onChange: (patch: Partial<CartesianConfig>) => void;
  seriesNames?: string[];
}) {
  return (
    <div className="space-y-2.5 pt-1.5">
      <SeriesOptions kind={kind} config={config} onChange={onChange} seriesNames={seriesNames} />
      <OrientationOption kind={kind} config={config} onChange={onChange} />
      <TrendOption kind={kind} config={config} onChange={onChange} />
      <AxisRangeOptions kind={kind} config={config} onChange={onChange} />
      <AxesOptions kind={kind} config={config} onChange={onChange} />
      <GoalOptions kind={kind} config={config} onChange={onChange} />
      <FormatOptions kind={kind} config={config} onChange={onChange} />
    </div>
  );
}
