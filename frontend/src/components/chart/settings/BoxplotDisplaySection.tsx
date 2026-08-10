import { Input } from "@/components/ui/input";
import { DecimalInput } from "./DecimalInput";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { BoxplotConfig } from "@/api/charts";
import { SERIES_PALETTE } from "@/components/chart/echarts/palette";
import { pointsModeFor } from "@/components/chart/echarts/options/boxplot";
import { Toggle } from "@/components/chart/settings/ChartAxesSection";

/**
 * The box plot's Display controls, in Metabase's own order: whiskers, points, mean, values, goal.
 *
 * egret had no display settings at all — the chart drew Tukey whiskers, outliers and a mean with no
 * way to ask for anything else, which is most of what a box plot is read for.
 */
type Props = {
  config: BoxplotConfig;
  onChange: (patch: Partial<BoxplotConfig>) => void;
};

function RadioRow({ label, value, options, onChange }: {
  label: string;
  value: string;
  options: { value: string; label: string }[];
  onChange: (v: string) => void;
}) {
  return (
    <div className="space-y-1.5">
      <Label className="text-sm font-normal">{label}</Label>
      <RadioGroup value={value} onValueChange={onChange} aria-label={label} className="gap-1.5">
        {options.map((o) => (
          <div key={o.value} className="flex items-center gap-2">
            <RadioGroupItem value={o.value} id={`bp-${label}-${o.value}`} />
            <Label htmlFor={`bp-${label}-${o.value}`} className="text-sm font-normal">{o.label}</Label>
          </div>
        ))}
      </RadioGroup>
    </div>
  );
}

export function BoxplotDisplaySection({ config, onChange }: Props) {
  const whiskers = config.whiskers ?? "tukey";
  // Min/Max whiskers reach the extremes, so nothing is an outlier: Metabase drops "Outliers only"
  // from the list and reads a stored "outliers" as "none". Reflecting that here keeps the selected
  // radio and the chart in agreement — otherwise the panel shows a choice that draws nothing.
  const points = pointsModeFor(config);
  const pointOptions = [
    { value: "none", label: "None" },
    ...(whiskers === "min-max" ? [] : [{ value: "outliers", label: "Outliers only" }]),
    { value: "all", label: "All points" },
  ];

  return (
    <div className="space-y-3">
      <RadioRow
        label="Whiskers extend to"
        value={whiskers}
        options={[
          { value: "tukey", label: "1.5 × interquartile range" },
          { value: "min-max", label: "Min/Max" },
        ]}
        onChange={(v) => onChange({ whiskers: v === "tukey" ? undefined : "min-max" })}
      />
      <RadioRow
        label="Show points"
        value={points}
        options={pointOptions}
        onChange={(v) => onChange({ points: v as BoxplotConfig["points"] })}
      />
      <Toggle id="bp-mean" label="Show mean" checked={config.showMean !== false}
        onChange={(v) => onChange({ showMean: v ? undefined : false })} />
      <Toggle id="bp-values" label="Show values on data points" checked={!!config.showLabels}
        onChange={(v) => onChange({ showLabels: v || undefined })} />
      {config.showLabels && (
        <div className="space-y-1">
          <Label htmlFor="bp-valuemode" className="text-sm font-normal">Values to display</Label>
          <Select value={config.labelMode ?? "median"}
            onValueChange={(v) => onChange({ labelMode: v as "median" | "all" })}>
            <SelectTrigger id="bp-valuemode" aria-label="Values to display" className="w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="median">Median only</SelectItem>
              <SelectItem value="all">All</SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}
      {/* One colour for the whole chart, because every other mark derives from it — the box fill is
          this at 15%, the mean is it darkened, the points are it faded. Offering separate swatches
          would let them disagree, which is the state Metabase's derivation exists to prevent. */}
      <div className="flex items-center justify-between gap-2">
        <Label className="text-sm font-normal">Color</Label>
        <input
          type="color" aria-label="Color"
          className="w-10 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
          value={config.color ?? SERIES_PALETTE[0]}
          onChange={(e) => onChange({ color: e.target.value })}
        />
      </div>
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor="bp-compact" className="text-sm font-normal">Shorten large numbers</Label>
        <Select
          value={config.compactValues === undefined ? "auto" : config.compactValues ? "on" : "off"}
          onValueChange={(v) => onChange({ compactValues: v === "auto" ? undefined : v === "on" })}
        >
          <SelectTrigger id="bp-compact" aria-label="Shorten large numbers"><SelectValue /></SelectTrigger>
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
            <Input placeholder="Goal" aria-label="Goal label" value={config.goalLabel ?? ""}
              onChange={(e) => onChange({ goalLabel: e.target.value || undefined })} />
          </label>
        )}
      </div>
    </div>
  );
}
