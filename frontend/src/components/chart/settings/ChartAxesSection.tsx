import type { ReactNode } from "react";
import { Input } from "@/components/ui/input";
import { DecimalInput } from "./DecimalInput";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

/**
 * The Axes tab, shared by every chart whose axes are "one ordinal dimension across, one measure up".
 *
 * The waterfall and the box plot both have exactly that, and both name their settings with the same
 * keys — so they get the same tab rather than two that drift apart. Structural typing keeps it
 * honest: any config carrying these fields fits, and one that does not will not compile.
 *
 * Deliberately NOT cartesian's `AxesOptions`/`AxisRangeOptions`: those offer an x-axis scale select
 * and a y-axis split, and neither of these charts honours either — their x is always the ordinal
 * sequence of categories and they plot one measure. Rendering them would put two more settings on
 * screen that silently do nothing.
 */
export interface AxesConfig {
  xLabel?: string;
  yLabel?: string;
  showXLabel?: boolean;
  showYLabel?: boolean;
  xAxisTicks?: boolean;
  yAxisTicks?: boolean;
  xLabelRotation?: 45 | 90;
  yTickCount?: number;
  yMin?: number;
  yMax?: number;
  logScale?: boolean;
}

/**
 * Metabase's `graph.x_axis.axis_enabled` — one control with four choices — projected onto the two
 * fields egret stores: `xAxisTicks` (the axis line and its tick marks) and `xLabelRotation` (an
 * explicit rotation, which suppresses the measured one; see `echarts/axisLabelFit.ts`).
 *
 * Exported as a pair of pure functions, the way `stackModeOf`/`stackModePatch` already are, because
 * TWO panels offer this choice — this tab (the waterfall and the box plot, its only two callers in
 * `viz/simple.tsx`; a sankey has no axes and stores neither field) and cartesian's
 * `AxisRangeOptions` — and a second hand-written projection is how the same stored pair ends up
 * meaning different things depending on which tab the user opened. The layout differs between the
 * two panels; the mapping must not.
 */
export type XTicksChoice = "hide" | "show" | "rotate45" | "rotate90";

export function xTicksChoiceOf(cfg: { xAxisTicks?: boolean; xLabelRotation?: 45 | 90 }): XTicksChoice {
  // "Hide" wins over a stored rotation: with the axis off there is nothing to tilt, and the select
  // can only show one of the four.
  if (cfg.xAxisTicks === false) return "hide";
  return cfg.xLabelRotation === 90 ? "rotate90" : cfg.xLabelRotation === 45 ? "rotate45" : "show";
}

/** Both keys are always written, `undefined` included: picking Rotate 45° while hidden has to CLEAR
 *  `xAxisTicks`, and picking Show has to clear the rotation. A patch that omitted the key it is not
 *  setting would leave the other half of the pair behind and let the select display a state it did
 *  not put the config in. */
export function xTicksPatch(v: XTicksChoice): { xAxisTicks: false | undefined; xLabelRotation: 45 | 90 | undefined } {
  return {
    xAxisTicks: v === "hide" ? false : undefined,
    xLabelRotation: v === "rotate45" ? 45 : v === "rotate90" ? 90 : undefined,
  };
}

/** Label on the left, switch on the right — the row shape this panel and Metabase's both use. */
export function Toggle({ id, label, checked, onChange }: {
  id: string; label: string; checked: boolean; onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between gap-2">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

/**
 * A block of settings under a heading naming which axis they belong to — Metabase's own "X-axis" /
 * "Y-axis" groups (every `graph.x_axis.*` / `graph.y_axis.*` setting carries a `group` of one or the
 * other, and its panel renders one block per group). Grouping by axis is the point: a flat run put
 * both titles together and then a lump of toggles, so answering "what can I change about the Y
 * axis" meant hopping.
 *
 * `role="group"` with the heading as its accessible name, rather than a styled div: it is what makes
 * the grouping REAL rather than a visual hint — a screen reader announces which axis it has entered,
 * and a test can assert that a control is inside the right block instead of merely that a heading
 * with the right words appears somewhere on the page.
 *
 * Exported because cartesian's Axes tab needs the same two blocks and the same headings. It is the
 * heading that is shared, NOT the settings: cartesian keeps its own controls (`AxesOptions` /
 * `AxisRangeOptions`), which offer an x-axis scale and a left/right split that neither the waterfall
 * nor the box plot honours.
 *
 * `axis` is the axis LETTER the reader sees, not the config key — on a horizontal chart the category
 * axis is the vertical one, and cartesian passes "Y" for it.
 */
export function AxisGroup({ axis, className, children }: {
  axis: string; className?: string; children: ReactNode;
}) {
  const name = `${axis}-axis`;
  return (
    <div role="group" aria-label={name} className={className ? `space-y-2 ${className}` : "space-y-2"}>
      <div className="inline-block rounded bg-muted px-1.5 py-0.5 text-xs font-medium text-muted-foreground">
        {name}
      </div>
      {children}
    </div>
  );
}

/**
 * THE ROWS BOTH AXES TABS SHARE, one implementation each.
 *
 * `ChartAxesSection` (waterfall, box plot) and cartesian's `AxisRangeOptions` (bar, line, area,
 * combo, row, scatter) store the SAME KEYS — `yMin`/`yMax`, `yAxisTicks`, `yTickCount`,
 * `xAxisTicks`/`xLabelRotation` — and until these were pulled out they presented them two different
 * ways: the waterfall's tab gated the bounds behind an "Auto y-axis range" switch and offered
 * lines-and-ticks as a select, cartesian's showed a bare always-visible min/max pair and a switch.
 * Which panel a reader saw depended only on which chart they had opened.
 *
 * That is the same failure as the four per-series settings spread over three parallel lists earlier
 * in this programme, and it has the same fix: not "copy the better markup across" — which is how it
 * arose — but ONE component with the differences passed in. The differences are real and few: the
 * axis LETTER (a horizontal cartesian chart's measure runs across, so its value axis is X), the id
 * prefix, and whether the rotations are reachable on this chart's category axis.
 *
 * Each takes a value and a patch callback rather than the config, so the mapping from control to
 * stored keys (`xTicksChoiceOf`/`xTicksPatch`) stays in one place and visible at the call site.
 */

/** Metabase's `graph.x_axis.axis_enabled` on the CATEGORY axis: Hide / Show / Rotate 45° / 90°. */
export function CategoryTicksSelect({ axis, id, value, onChange, rotationsDisabled, describedBy }: {
  /** The axis letter the READER sees — "Y" for the category axis of a horizontal chart. */
  axis: string;
  id: string;
  value: XTicksChoice;
  onChange: (v: XTicksChoice) => void;
  /** Greys the two rotations out where the chart draws a VALUE axis across, whose tick labels
   *  ECharts generates and `scaleAxis` gives no `rotate`. Never set by the waterfall/box-plot tab,
   *  whose x is always the ordinal sequence of categories. Disabled rather than hidden: a chart
   *  that stored 45° and later changed scale must still show what it stored. */
  rotationsDisabled?: boolean;
  /** Id of the caption explaining that greying, hung off the TRIGGER — the disabled items
   *  themselves are unreachable by mouse, keyboard and screen reader alike. */
  describedBy?: string;
}) {
  return (
    <div className="space-y-1">
      <Label htmlFor={id} className="text-sm font-normal">Show lines and tick marks</Label>
      <Select value={value} onValueChange={(v) => onChange(v as XTicksChoice)}>
        <SelectTrigger id={id} aria-label={`${axis} show lines and tick marks`} className="h-8 w-full"
          aria-describedby={describedBy}><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="hide">Hide</SelectItem>
          <SelectItem value="show">Show</SelectItem>
          {(["rotate45", "rotate90"] as const).map((v) => (
            <SelectItem key={v} value={v} disabled={rotationsDisabled}>
              {v === "rotate45" ? "Rotate 45°" : "Rotate 90°"}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

/** Metabase's `graph.y_axis.axis_enabled` on the VALUE axis — the same control with no rotation,
 *  since a value axis' labels are ticks ECharts generates rather than the category array. */
export function ValueTicksSelect({ axis, id, shown, onChange }: {
  axis: string; id: string; shown: boolean; onChange: (shown: boolean) => void;
}) {
  return (
    <div className="space-y-1">
      <Label htmlFor={id} className="text-sm font-normal">Show lines and tick marks</Label>
      <Select value={shown ? "show" : "hide"} onValueChange={(v) => onChange(v !== "hide")}>
        <SelectTrigger id={id} aria-label={`${axis} show lines and tick marks`} className="h-8 w-full"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="hide">Hide</SelectItem>
          <SelectItem value="show">Show</SelectItem>
        </SelectContent>
      </Select>
    </div>
  );
}

/**
 * Metabase's automatic value-axis range and the two bounds it gates.
 *
 * DERIVED, NOT STORED. The switch is `yMin == null && yMax == null`, so there is no fourth key and
 * no state in which it and the boxes can disagree — turning it off has to SEED a bound or the
 * switch would flip straight back on its own next render, which is why "off" emits `{ yMin: 0 }`.
 *
 * DecimalInput, not `type="number"`: an axis bound is a data value, so "4." is a state a user
 * passes through on the way to 4.5 — and a number input reports itself EMPTY for exactly as long as
 * the point is the last character, which cleared the bound mid-word and dropped the keystroke. See
 * DecimalInput for the measurement.
 */
export function ValueRangeRows({ axis, idPrefix, min, max, onChange }: {
  axis: string; idPrefix: string;
  min: number | undefined; max: number | undefined;
  onChange: (patch: { yMin?: number; yMax?: number }) => void;
}) {
  const autoRange = min == null && max == null;
  return (
    <>
      <Toggle id={`${idPrefix}-autorange`} label={`Auto ${axis.toLowerCase()}-axis range`} checked={autoRange}
        onChange={(v) => onChange(v ? { yMin: undefined, yMax: undefined } : { yMin: 0 })} />
      {!autoRange && (
        <div className="grid grid-cols-2 gap-2">
          <label className="space-y-1">
            <span className="text-xs text-muted-foreground">{axis} min</span>
            <DecimalInput className="h-8" placeholder="auto" aria-label={`${axis} min`}
              value={min} onChange={(yMin) => onChange({ yMin })} />
          </label>
          <label className="space-y-1">
            <span className="text-xs text-muted-foreground">{axis} max</span>
            <DecimalInput className="h-8" placeholder="auto" aria-label={`${axis} max`}
              value={max} onChange={(yMax) => onChange({ yMax })} />
          </label>
        </div>
      )}
    </>
  );
}

/** Metabase's `graph.y_axis.split_number`. Its `getHidden` is
 *  `settings["graph.y_axis.axis_enabled"] === false` — with the axis' lines and ticks off there are
 *  no tick marks to count — and BOTH callers apply that gate, which is why it is not applied here:
 *  the row that owns the gate is the one above it in each panel. */
export function TickCountRow({ id, value, onChange }: {
  id: string; value: number | undefined; onChange: (n: number | undefined) => void;
}) {
  return (
    <div className="space-y-1">
      <Label htmlFor={id} className="text-sm font-normal">Number of tick marks</Label>
      <Input
        id={id} aria-label="Number of tick marks" type="number" min={2} max={20}
        className="h-8" placeholder="auto" value={value ?? ""}
        onChange={(e) => {
          const n = Number(e.target.value);
          onChange(e.target.value === "" || Number.isNaN(n) ? undefined : n);
        }}
      />
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
 * axis labels, and these charts' x is a category run through its column format, so the option
 * would do nothing here.
 */
export function ChartAxesSection<C extends AxesConfig>({ config, onChange, idPrefix = "ax" }: {
  config: C;
  onChange: (patch: Partial<C>) => void;
  /** Distinct per chart so two panels on one page cannot share a label's `htmlFor`. */
  idPrefix?: string;
}) {
  return (
    <div className="space-y-3">
      <AxisGroup axis="X">
        <AxisTitleRow id={`${idPrefix}-xlabel`} axis="X" label={config.xLabel} shown={config.showXLabel !== false}
          onLabel={(v) => onChange({ xLabel: v } as Partial<C>)} onShown={(v) => onChange({ showXLabel: v ? undefined : false } as Partial<C>)} />
        {/* No `rotationsDisabled`: these two charts' x is always the ordinal sequence of categories,
            so the rotation always applies. Cartesian's tab passes it, because a linear/timeseries x
            draws a value axis whose labels `scaleAxis` gives no `rotate`. */}
        <CategoryTicksSelect axis="X" id={`${idPrefix}-xticks`} value={xTicksChoiceOf(config)}
          onChange={(v) => onChange(xTicksPatch(v) as Partial<C>)} />
      </AxisGroup>

      <AxisGroup axis="Y" className="border-t pt-3">
        <AxisTitleRow id={`${idPrefix}-ylabel`} axis="Y" label={config.yLabel} shown={config.showYLabel !== false}
          onLabel={(v) => onChange({ yLabel: v } as Partial<C>)} onShown={(v) => onChange({ showYLabel: v ? undefined : false } as Partial<C>)} />
        <ValueRangeRows axis="Y" idPrefix={idPrefix} min={config.yMin} max={config.yMax}
          onChange={(patch) => onChange(patch as Partial<C>)} />
        {/* Metabase's Scale select offers Linear / Power / Log. **Power is not offered**: ECharts'
            value axis has no power scale, Metabase implements its own, and a third option that
            silently drew a linear axis would be worse than its absence. */}
        <div className="space-y-1">
          <Label htmlFor={`${idPrefix}-scale`} className="text-sm font-normal">Scale</Label>
          <Select value={config.logScale ? "log" : "linear"}
            onValueChange={(v) => onChange({ logScale: v === "log" || undefined } as Partial<C>)}>
            <SelectTrigger id={`${idPrefix}-scale`} aria-label="Y scale" className="h-8 w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="linear">Linear</SelectItem>
              <SelectItem value="log">Log</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <ValueTicksSelect axis="Y" id={`${idPrefix}-yticks`} shown={config.yAxisTicks !== false}
          onChange={(shown) => onChange({ yAxisTicks: shown ? undefined : false } as Partial<C>)} />
        {config.yAxisTicks !== false && (
          <TickCountRow id={`${idPrefix}-ytickcount`} value={config.yTickCount}
            onChange={(yTickCount) => onChange({ yTickCount } as Partial<C>)} />
        )}
      </AxisGroup>
    </div>
  );
}
