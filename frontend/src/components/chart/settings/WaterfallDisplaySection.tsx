import { Input } from "@/components/ui/input";
import { ColorPicker } from "./ColorPicker";
import { DecimalInput } from "./DecimalInput";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { WaterfallConfig } from "@/api/charts";
import { INCREASE, DECREASE, defaultTotalColor } from "@/components/chart/echarts/options/waterfall";
import { toHexColor } from "@/components/chart/echarts/palette";
import { Toggle } from "@/components/chart/settings/ChartAxesSection";

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
      <ColorPicker label={label} value={value ?? fallback} onChange={onChange} />
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
        // The fallback is the colour the builder draws the bar in, the page's ink resolved for the
        // current theme — a stand-in slate here used to disagree with the near-black bar (KI-057).
        // As a hex, because the picker's native colour input takes nothing else.
        <ColorRow label="Total color" value={config.totalColor} fallback={toHexColor(defaultTotalColor())}
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
        <div className="space-y-1.5">
          <Label htmlFor="wf-density" className="text-sm font-normal">Data label density</Label>
          <Select value={config.labelDensity === "all" ? "all" : "fit"}
            onValueChange={(v) => onChange({ labelDensity: v as "fit" | "all" })}>
            <SelectTrigger className="w-full" id="wf-density" aria-label="Data label density"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="fit">Fit</SelectItem>
              <SelectItem value="all">All</SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}
      {/* Metabase's "Auto formatting: Auto | Compact | Full". Three states, so a two-state switch
          would misreport Auto as one of the other two. */}
      <div className="space-y-1.5">
        <Label htmlFor="wf-compact" className="text-sm font-normal">Shorten large numbers</Label>
        <Select
          value={config.compactValues === undefined ? "auto" : config.compactValues ? "on" : "off"}
          onValueChange={(v) => onChange({ compactValues: v === "auto" ? undefined : v === "on" })}
        >
          <SelectTrigger className="w-full" id="wf-compact" aria-label="Shorten large numbers"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="auto">Auto</SelectItem>
            <SelectItem value="on">Compact</SelectItem>
            <SelectItem value="off">Full</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-2 border-t pt-2">
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">Goal value</span>
          {/* DecimalInput: a goal is a data value, and `type="number"` reports itself EMPTY for
              as long as a decimal point is the last character, which fed `undefined` into the
              config mid-word and lost the keystroke. See DecimalInput. */}
          <DecimalInput placeholder="none" aria-label="Goal value"
            value={config.goalValue}
            onChange={(goalValue) => onChange({ goalValue })} />
        </label>
        {/* GATED ON THE GOAL VALUE, the same rule and the same reason as `GoalOptions` in
            `CartesianDisplaySection` — this builder reads `cfg.goalLabel` only inside
            `if (cfg.goalValue != null)`, so with no goal the box stored a string nothing would
            draw. `!= null` rather than truthiness, because 0 is a legal goal. */}
        {config.goalValue != null && (
          <label className="space-y-1">
            <span className="text-xs text-muted-foreground">Goal label</span>
            <Input className="w-full" placeholder="Goal" aria-label="Goal label" value={config.goalLabel ?? ""}
              onChange={(e) => onChange({ goalLabel: e.target.value || undefined })} />
          </label>
        )}
      </div>
    </div>
  );
}
