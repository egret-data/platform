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

/** A number field that stores `undefined` for "auto" rather than 0 — the difference between "no
 *  opinion" and "pin it to zero". */
function NumberRow({ label, value, placeholder, onChange, min, max }: {
  label: string; value: number | undefined; placeholder: string;
  onChange: (v: number | undefined) => void; min?: number; max?: number;
}) {
  return (
    <div className="flex items-center justify-between gap-2">
      <Label className="text-sm font-normal">{label}</Label>
      <Input
        type="number" aria-label={label} className="h-7 w-24" placeholder={placeholder}
        min={min} max={max} value={value ?? ""}
        onChange={(e) => {
          const n = Number(e.target.value);
          onChange(e.target.value === "" || Number.isNaN(n) ? undefined : n);
        }}
      />
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

/** The Axes tab: each axis's title and whether it shows, its lines and ticks, and the value axis's
 *  range and scale. Same keys and same wording as the cartesian charts'. */
export function WaterfallAxesSection({ config, onChange }: Props) {
  return (
    <div className="space-y-2.5">
      <div className="space-y-2">
        <div className="space-y-1">
          <div className="flex items-center justify-between gap-2">
            <Label htmlFor="wf-xlabel" className="text-sm font-normal">X label</Label>
            <Switch id="wf-show-xlabel" aria-label="Show X label" checked={config.showXLabel !== false}
              onCheckedChange={(v) => onChange({ showXLabel: v ? undefined : false })} />
          </div>
          {config.showXLabel !== false && (
            <Input id="wf-xlabel" aria-label="X label" className="h-8" placeholder="Auto"
              value={config.xLabel ?? ""} onChange={(e) => onChange({ xLabel: e.target.value || undefined })} />
          )}
        </div>
        <div className="space-y-1">
          <div className="flex items-center justify-between gap-2">
            <Label htmlFor="wf-ylabel" className="text-sm font-normal">Y label</Label>
            <Switch id="wf-show-ylabel" aria-label="Show Y label" checked={config.showYLabel !== false}
              onCheckedChange={(v) => onChange({ showYLabel: v ? undefined : false })} />
          </div>
          {config.showYLabel !== false && (
            <Input id="wf-ylabel" aria-label="Y label" className="h-8" placeholder="Auto"
              value={config.yLabel ?? ""} onChange={(e) => onChange({ yLabel: e.target.value || undefined })} />
          )}
        </div>
      </div>
      <div className="space-y-2.5 border-t pt-2">
        {/* Stored as `false` only when turned off, so a waterfall saved before these existed keeps
            drawing both. */}
        <Toggle id="wf-xticks" label="X axis lines and ticks" checked={config.xAxisTicks !== false}
          onChange={(v) => onChange({ xAxisTicks: v ? undefined : false })} />
        <Toggle id="wf-yticks" label="Y axis lines and ticks" checked={config.yAxisTicks !== false}
          onChange={(v) => onChange({ yAxisTicks: v ? undefined : false })} />
        <Toggle id="wf-log" label="Log scale" checked={!!config.logScale}
          onChange={(v) => onChange({ logScale: v || undefined })} />
        <NumberRow label="Y tick marks" value={config.yTickCount} placeholder="Auto" min={2} max={20}
          onChange={(v) => onChange({ yTickCount: v })} />
        {/* Label ABOVE the input for this pair, not beside it: two side-by-side rows in half the
            panel width wrap "Y min" onto two lines. The cartesian range controls stack for the same
            reason. */}
        <div className="grid grid-cols-2 gap-2 border-t pt-2">
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
      </div>
    </div>
  );
}
