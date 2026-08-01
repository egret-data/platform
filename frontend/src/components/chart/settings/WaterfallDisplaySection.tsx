import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { WaterfallConfig } from "@/api/charts";
import { INCREASE, DECREASE } from "@/components/chart/echarts/options/waterfall";

/**
 * The waterfall's Display and Axes controls.
 *
 * Metabase gives the waterfall a full three-tab panel — Data | Display | Axes — where egret gave it
 * three data fields and nothing else. These are its Display tab (goal line, the three bar colours,
 * show total, data labels, number formatting) and its Axes tab, named after the same config keys
 * the cartesian charts use so the two read identically side by side on a dashboard.
 *
 * Deliberately NOT reusing cartesian's `AxesOptions`/`AxisRangeOptions`: they offer an x-axis scale
 * select and a y-axis split, and the waterfall builder honours neither — a waterfall's x is always
 * the ordinal sequence of steps and it plots one metric. Rendering those controls here would put
 * two more settings on screen that silently do nothing, which is the defect this program keeps
 * finding rather than one to add.
 */
type Props = {
  config: WaterfallConfig;
  onChange: (patch: Partial<WaterfallConfig>) => void;
};

function ColorRow({ label, value, fallback, onChange }: {
  label: string; value: string | undefined; fallback: string; onChange: (v: string) => void;
}) {
  return (
    <div className="flex items-center justify-between gap-2">
      <Label className="text-sm font-normal">{label}</Label>
      <input
        type="color"
        aria-label={label}
        className="h-7 w-10 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
        value={value ?? fallback}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  );
}

function Toggle({ id, label, checked, onChange }: {
  id: string; label: string; checked: boolean; onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between gap-2">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

/** Bar colours and the Total bar. `waterfall.total_color` is hidden while the total is off, as
 *  Metabase hides it — a colour for a bar that is not drawn is a control with nothing to act on. */
export function WaterfallBarsSection({ config, onChange }: Props) {
  const showTotal = config.showTotal !== false;
  return (
    <div className="space-y-2.5">
      <ColorRow label="Increase color" value={config.increaseColor} fallback={INCREASE}
        onChange={(v) => onChange({ increaseColor: v })} />
      <ColorRow label="Decrease color" value={config.decreaseColor} fallback={DECREASE}
        onChange={(v) => onChange({ decreaseColor: v })} />
      <Toggle id="wf-total" label="Show total" checked={showTotal}
        onChange={(v) => onChange({ showTotal: v ? undefined : false })} />
      {showTotal && (
        // No `fallback` from the palette: the total takes the page's ink, which is a CSS token and
        // theme-dependent, so there is no one hex to preview. A native colour input must be given
        // something concrete, and a mid-grey is the honest stand-in for "follows the theme".
        <ColorRow label="Total color" value={config.totalColor} fallback="#4C5773"
          onChange={(v) => onChange({ totalColor: v })} />
      )}
    </div>
  );
}

/** Data labels, large-number shortening, and the goal line — Metabase's remaining Display rows. */
export function WaterfallFormatSection({ config, onChange }: Props) {
  return (
    <div className="space-y-2.5">
      <Toggle id="wf-labels" label="Show values on data points" checked={!!config.showLabels}
        onChange={(v) => onChange({ showLabels: v || undefined })} />
      {config.showLabels && (
        <div className="flex items-center justify-between gap-2">
          <Label htmlFor="wf-density" className="text-sm font-normal">Data label density</Label>
          <Select value={config.labelDensity === "all" ? "all" : "fit"}
            onValueChange={(v) => onChange({ labelDensity: v as "fit" | "all" })}>
            <SelectTrigger id="wf-density" aria-label="Data label density" className="h-7 w-24"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="fit">Fit</SelectItem>
              <SelectItem value="all">All</SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}
      {/* Metabase's "Auto formatting: Auto | Compact | Full". Three states, so a two-state switch
          would misreport Auto as one of the other two. */}
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor="wf-compact" className="text-sm font-normal">Shorten large numbers</Label>
        <Select
          value={config.compactValues === undefined ? "auto" : config.compactValues ? "on" : "off"}
          onValueChange={(v) => onChange({ compactValues: v === "auto" ? undefined : v === "on" })}
        >
          <SelectTrigger id="wf-compact" aria-label="Shorten large numbers" className="h-7 w-24"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="auto">Auto</SelectItem>
            <SelectItem value="on">Compact</SelectItem>
            <SelectItem value="off">Full</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div className="grid grid-cols-2 gap-2 border-t pt-2">
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">Goal value</span>
          <Input className="h-8" type="number" placeholder="none" aria-label="Goal value"
            value={config.goalValue ?? ""}
            onChange={(e) => {
              const n = Number(e.target.value);
              onChange({ goalValue: e.target.value === "" || Number.isNaN(n) ? undefined : n });
            }} />
        </label>
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">Goal label</span>
          <Input className="h-8" placeholder="Goal" aria-label="Goal label" value={config.goalLabel ?? ""}
            onChange={(e) => onChange({ goalLabel: e.target.value || undefined })} />
        </label>
      </div>
    </div>
  );
}

/** A section heading naming which axis the settings under it belong to — Metabase's own "X-axis" /
 *  "Y-axis" pills. Grouping by axis is the point: the previous layout put both titles together and
 *  then a lump of toggles, so answering "what can I change about the Y axis" meant hopping. */
function AxisHeading({ children }: { children: string }) {
  return (
    <div className="inline-block rounded bg-muted px-1.5 py-0.5 text-xs font-medium text-muted-foreground">
      {children}
    </div>
  );
}

/** Title + its Show switch, the pair Metabase puts at the top of each axis. */
function AxisTitleRow({ id, axis, label, shown, onLabel, onShown }: {
  id: string; axis: string; label: string | undefined; shown: boolean;
  onLabel: (v: string | undefined) => void; onShown: (v: boolean) => void;
}) {
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor={id} className="text-sm font-normal">Show label</Label>
        <Switch id={`${id}-shown`} aria-label={`Show ${axis} label`} checked={shown} onCheckedChange={onShown} />
      </div>
      {shown && (
        <Input id={id} aria-label={`${axis} label`} className="h-8" placeholder="Auto"
          value={label ?? ""} onChange={(e) => onLabel(e.target.value || undefined)} />
      )}
    </div>
  );
}

/**
 * The Axes tab, grouped by axis the way Metabase groups it.
 *
 * "Show lines and tick marks" is a SELECT rather than a switch because Metabase folds label
 * rotation into the same control (`graph.x_axis.axis_enabled`: Hide / Show / Compact / Rotate 45° /
 * Rotate 90°), and rotation is the part egret had no way to ask for at all — it tilted long names
 * at 30° on a guess and offered no override. **"Compact" is not offered**: it shortens temporal
 * axis labels, and a waterfall's x is a category run through its column format, so the option would
 * do nothing here.
 */
export function WaterfallAxesSection({ config, onChange }: Props) {
  // Metabase gates the min/max pair behind "Auto y-axis range" rather than showing two boxes that
  // usually say "auto". Derived from the two values instead of a fourth stored key, so there is no
  // state where the switch and the inputs can disagree.
  const autoRange = config.yMin == null && config.yMax == null;
  const xTicks = config.xAxisTicks === false
    ? "hide"
    : config.xLabelRotation === 90 ? "rotate90" : config.xLabelRotation === 45 ? "rotate45" : "show";

  return (
    <div className="space-y-3">
      <div className="space-y-2">
        <AxisHeading>X-axis</AxisHeading>
        <AxisTitleRow id="wf-xlabel" axis="X" label={config.xLabel} shown={config.showXLabel !== false}
          onLabel={(v) => onChange({ xLabel: v })} onShown={(v) => onChange({ showXLabel: v ? undefined : false })} />
        <div className="space-y-1">
          <Label htmlFor="wf-xticks" className="text-sm font-normal">Show lines and tick marks</Label>
          <Select
            value={xTicks}
            onValueChange={(v) => onChange({
              xAxisTicks: v === "hide" ? false : undefined,
              xLabelRotation: v === "rotate45" ? 45 : v === "rotate90" ? 90 : undefined,
            })}
          >
            <SelectTrigger id="wf-xticks" aria-label="X show lines and tick marks" className="h-8 w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="hide">Hide</SelectItem>
              <SelectItem value="show">Show</SelectItem>
              <SelectItem value="rotate45">Rotate 45°</SelectItem>
              <SelectItem value="rotate90">Rotate 90°</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      <div className="space-y-2 border-t pt-3">
        <AxisHeading>Y-axis</AxisHeading>
        <AxisTitleRow id="wf-ylabel" axis="Y" label={config.yLabel} shown={config.showYLabel !== false}
          onLabel={(v) => onChange({ yLabel: v })} onShown={(v) => onChange({ showYLabel: v ? undefined : false })} />
        <Toggle id="wf-autorange" label="Auto y-axis range" checked={autoRange}
          onChange={(v) => onChange(v ? { yMin: undefined, yMax: undefined } : { yMin: 0 })} />
        {!autoRange && (
          <div className="grid grid-cols-2 gap-2">
            <label className="space-y-1">
              <span className="text-xs text-muted-foreground">Y min</span>
              <Input className="h-8" type="number" placeholder="auto" aria-label="Y min"
                value={config.yMin ?? ""}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  onChange({ yMin: e.target.value === "" || Number.isNaN(n) ? undefined : n });
                }} />
            </label>
            <label className="space-y-1">
              <span className="text-xs text-muted-foreground">Y max</span>
              <Input className="h-8" type="number" placeholder="auto" aria-label="Y max"
                value={config.yMax ?? ""}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  onChange({ yMax: e.target.value === "" || Number.isNaN(n) ? undefined : n });
                }} />
            </label>
          </div>
        )}
        {/* Metabase's Scale select offers Linear / Power / Log. **Power is not offered**: ECharts'
            value axis has no power scale, Metabase implements its own, and a third option that
            silently drew a linear axis would be worse than its absence. */}
        <div className="space-y-1">
          <Label htmlFor="wf-scale" className="text-sm font-normal">Scale</Label>
          <Select value={config.logScale ? "log" : "linear"}
            onValueChange={(v) => onChange({ logScale: v === "log" || undefined })}>
            <SelectTrigger id="wf-scale" aria-label="Y scale" className="h-8 w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="linear">Linear</SelectItem>
              <SelectItem value="log">Log</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-1">
          <Label htmlFor="wf-yticks" className="text-sm font-normal">Show lines and tick marks</Label>
          <Select value={config.yAxisTicks === false ? "hide" : "show"}
            onValueChange={(v) => onChange({ yAxisTicks: v === "hide" ? false : undefined })}>
            <SelectTrigger id="wf-yticks" aria-label="Y show lines and tick marks" className="h-8 w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="hide">Hide</SelectItem>
              <SelectItem value="show">Show</SelectItem>
            </SelectContent>
          </Select>
        </div>
        {/* Label above a full-width input, like every other field on this tab and like Metabase's.
            Inline-right would make it the only row that reads differently. */}
        <div className="space-y-1">
          <Label htmlFor="wf-ytickcount" className="text-sm font-normal">Number of tick marks</Label>
          <Input
            id="wf-ytickcount" aria-label="Number of tick marks" type="number" min={2} max={20}
            className="h-8" placeholder="auto" value={config.yTickCount ?? ""}
            onChange={(e) => {
              const n = Number(e.target.value);
              onChange({ yTickCount: e.target.value === "" || Number.isNaN(n) ? undefined : n });
            }}
          />
        </div>
      </div>
    </div>
  );
}
