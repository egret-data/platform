import type { CartesianConfig, CartesianViz } from "@/api/charts";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { seriesColor, SUMMARIZE_COLOR } from "@/components/chart/echarts/palette";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

function Toggle({ id, label, checked, onChange }: { id: string; label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

/**
 * Auto / On / Off — for a setting that now defaults from the data at render time (see
 * buildCartesianOption), so `undefined` is a real, visible third state rather than "off".
 * A two-state Switch can't represent that without contradicting the chart it controls.
 */
function AutoToggle({
  id, label, value, onChange, autoHint,
}: { id: string; label: string; value: boolean | undefined; onChange: (v: boolean | undefined) => void; autoHint: string }) {
  const current = value === undefined ? "auto" : value ? "on" : "off";
  return (
    <div className="flex items-center justify-between gap-2">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Select value={current} onValueChange={(v) => onChange(v === "auto" ? undefined : v === "on")}>
        <SelectTrigger id={id} aria-label={label} className="h-7 w-24"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="auto" title={autoHint}>Auto</SelectItem>
          <SelectItem value="on">On</SelectItem>
          <SelectItem value="off">Off</SelectItem>
        </SelectContent>
      </Select>
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
export function SeriesOptions({ kind, config, onChange, seriesNames = [] }: SectionProps & { seriesNames?: string[] }) {
  const canStack = kind === "bar" || kind === "row" || kind === "area";
  const isLineish = kind === "line" || kind === "area";
  // Default colour a series shows before any override: lone (non-breakout) metric gets the
  // summarize green, everything else cycles the accent palette — mirrors the renderer.
  const defaultColor = (i: number) =>
    !config.series && seriesNames.length === 1 ? SUMMARIZE_COLOR : seriesColor(i);
  return (
    <div className="space-y-2.5">
      {canStack && <Toggle id="cz-stacked" label="Stacked" checked={!!config.stacked} onChange={(v) => onChange({ stacked: v })} />}
      {canStack && config.stacked && <Toggle id="cz-normalized" label="100% stacked" checked={!!config.normalized} onChange={(v) => onChange({ normalized: v })} />}
      {canStack && config.stacked && !config.normalized && <Toggle id="cz-stacktotal" label="Stack totals" checked={!!config.showStackTotal} onChange={(v) => onChange({ showStackTotal: v })} />}
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
              <SelectItem value="interpolate">Interpolate</SelectItem>
              <SelectItem value="zero">Zero</SelectItem>
              <SelectItem value="none">None</SelectItem>
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

      {seriesNames.length > 0 && (
        <div className="space-y-1.5 border-t pt-2">
          <div className="text-xs text-muted-foreground">Series</div>
          {seriesNames.map((name, i) => (
            <div key={name} className="flex items-center gap-2">
              <input
                type="color"
                aria-label={`color ${name}`}
                className="h-7 w-8 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
                value={config.seriesColors?.[name] ?? defaultColor(i)}
                onChange={(e) => onChange({ seriesColors: { ...config.seriesColors, [name]: e.target.value } })}
              />
              <Input
                className="h-7 flex-1"
                aria-label={`label ${name}`}
                placeholder={name}
                value={config.seriesLabels?.[name] ?? ""}
                onChange={(e) => {
                  const next = { ...config.seriesLabels };
                  if (e.target.value) next[name] = e.target.value; else delete next[name];
                  onChange({ seriesLabels: next });
                }}
              />
            </div>
          ))}
        </div>
      )}

      {kind === "combo" && config.yMetrics.length > 0 && (
        <div className="space-y-1.5 border-t pt-2">
          <div className="text-xs text-muted-foreground">Series type</div>
          {config.yMetrics.map((m) => (
            <div key={m} className="flex items-center justify-between gap-2">
              <Label className="truncate text-sm font-normal">{m}</Label>
              <Select value={config.seriesTypes?.[m] ?? "bar"} onValueChange={(v) => onChange({ seriesTypes: { ...config.seriesTypes, [m]: v as "bar" | "line" } })}>
                <SelectTrigger aria-label={`type ${m}`} className="h-7 w-24"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="bar">Bar</SelectItem>
                  <SelectItem value="line">Line</SelectItem>
                </SelectContent>
              </Select>
            </div>
          ))}
        </div>
      )}
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
          {config.yMetrics.map((m) => {
            const onRight = (config.rightAxisMetrics ?? []).includes(m);
            return (
              <div key={m} className="flex items-center justify-between gap-2">
                <Label className="truncate text-sm font-normal">{m}</Label>
                <Select value={onRight ? "right" : "left"} onValueChange={(v) => {
                  const rest = (config.rightAxisMetrics ?? []).filter((x) => x !== m);
                  onChange({ rightAxisMetrics: v === "right" ? [...rest, m] : rest });
                }}>
                  <SelectTrigger aria-label={`axis ${m}`} className="h-7 w-24"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="left">Left</SelectItem>
                    <SelectItem value="right">Right</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            );
          })}
        </div>
      )}

      <div className="grid grid-cols-2 gap-2 border-t pt-2">
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">X label</span>
          <Input className="h-8" value={config.xLabel ?? ""} onChange={(e) => onChange({ xLabel: e.target.value })} />
        </label>
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">Y label</span>
          <Input className="h-8" value={config.yLabel ?? ""} onChange={(e) => onChange({ yLabel: e.target.value })} />
        </label>
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
