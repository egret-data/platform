import { Input } from "@/components/ui/input";
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
 * axis labels, and these charts' x is a category run through its column format, so the option
 * would do nothing here.
 */
export function ChartAxesSection<C extends AxesConfig>({ config, onChange, idPrefix = "ax" }: {
  config: C;
  onChange: (patch: Partial<C>) => void;
  /** Distinct per chart so two panels on one page cannot share a label's `htmlFor`. */
  idPrefix?: string;
}) {
  // Metabase gates the min/max pair behind "Auto y-axis range" rather than showing two boxes that
  // usually say "auto". Derived from the two values instead of a fourth stored key, so there is no
  // state where the switch and the inputs can disagree.
  const autoRange = config.yMin == null && config.yMax == null;
  const xTicks = xTicksChoiceOf(config);

  return (
    <div className="space-y-3">
      <div className="space-y-2">
        <AxisHeading>X-axis</AxisHeading>
        <AxisTitleRow id={`${idPrefix}-xlabel`} axis="X" label={config.xLabel} shown={config.showXLabel !== false}
          onLabel={(v) => onChange({ xLabel: v } as Partial<C>)} onShown={(v) => onChange({ showXLabel: v ? undefined : false } as Partial<C>)} />
        <div className="space-y-1">
          <Label htmlFor={`${idPrefix}-xticks`} className="text-sm font-normal">Show lines and tick marks</Label>
          <Select
            value={xTicks}
            onValueChange={(v) => onChange(xTicksPatch(v as XTicksChoice) as Partial<C>)}
          >
            <SelectTrigger id={`${idPrefix}-xticks`} aria-label="X show lines and tick marks" className="h-8 w-full"><SelectValue /></SelectTrigger>
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
        <AxisTitleRow id={`${idPrefix}-ylabel`} axis="Y" label={config.yLabel} shown={config.showYLabel !== false}
          onLabel={(v) => onChange({ yLabel: v } as Partial<C>)} onShown={(v) => onChange({ showYLabel: v ? undefined : false } as Partial<C>)} />
        <Toggle id={`${idPrefix}-autorange`} label="Auto y-axis range" checked={autoRange}
          onChange={(v) => onChange((v ? { yMin: undefined, yMax: undefined } : { yMin: 0 }) as Partial<C>)} />
        {!autoRange && (
          <div className="grid grid-cols-2 gap-2">
            <label className="space-y-1">
              <span className="text-xs text-muted-foreground">Y min</span>
              <Input className="h-8" type="number" placeholder="auto" aria-label="Y min"
                value={config.yMin ?? ""}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  onChange({ yMin: e.target.value === "" || Number.isNaN(n) ? undefined : n } as Partial<C>);
                }} />
            </label>
            <label className="space-y-1">
              <span className="text-xs text-muted-foreground">Y max</span>
              <Input className="h-8" type="number" placeholder="auto" aria-label="Y max"
                value={config.yMax ?? ""}
                onChange={(e) => {
                  const n = Number(e.target.value);
                  onChange({ yMax: e.target.value === "" || Number.isNaN(n) ? undefined : n } as Partial<C>);
                }} />
            </label>
          </div>
        )}
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
        <div className="space-y-1">
          <Label htmlFor={`${idPrefix}-yticks`} className="text-sm font-normal">Show lines and tick marks</Label>
          <Select value={config.yAxisTicks === false ? "hide" : "show"}
            onValueChange={(v) => onChange({ yAxisTicks: v === "hide" ? false : undefined } as Partial<C>)}>
            <SelectTrigger id={`${idPrefix}-yticks`} aria-label="Y show lines and tick marks" className="h-8 w-full"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="hide">Hide</SelectItem>
              <SelectItem value="show">Show</SelectItem>
            </SelectContent>
          </Select>
        </div>
        {/* Label above a full-width input, like every other field on this tab and like Metabase's.
            Inline-right would make it the only row that reads differently. */}
        <div className="space-y-1">
          <Label htmlFor={`${idPrefix}-ytickcount`} className="text-sm font-normal">Number of tick marks</Label>
          <Input
            id={`${idPrefix}-ytickcount`} aria-label="Number of tick marks" type="number" min={2} max={20}
            className="h-8" placeholder="auto" value={config.yTickCount ?? ""}
            onChange={(e) => {
              const n = Number(e.target.value);
              onChange({ yTickCount: e.target.value === "" || Number.isNaN(n) ? undefined : n } as Partial<C>);
            }}
          />
        </div>
      </div>
    </div>
  );
}
