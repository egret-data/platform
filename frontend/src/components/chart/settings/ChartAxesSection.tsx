import type { ReactNode } from "react";
import { Input } from "@/components/ui/input";
import { DecimalInput } from "./DecimalInput";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { unpinFromZeroAppliesToBoxplot } from "@/components/chart/echarts/options/boxplot";

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
  /** Metabase's fifth "Show lines and tick marks" value. Optional on the SHARED shape because only
   *  one of this tab's two charts stores it — the box plot's category axis has no label formatter
   *  for it to act through, so its config carries no such key at all. */
  xLabelCompact?: boolean;
  /** Metabase's `graph.y_axis.unpin_from_zero`, and the mirror image of the field above: the BOX
   *  PLOT stores this one and the waterfall does not, because `isYAxisUnpinFromZeroValid` excludes
   *  a waterfall display outright. */
  unpinFromZero?: boolean;
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
export type XTicksChoice = "hide" | "show" | "compact" | "rotate45" | "rotate90";

export function xTicksChoiceOf(cfg: { xAxisTicks?: boolean; xLabelRotation?: 45 | 90; xLabelCompact?: boolean }): XTicksChoice {
  // "Hide" wins over a stored rotation: with the axis off there is nothing to tilt, and the select
  // can only show one of the five.
  if (cfg.xAxisTicks === false) return "hide";
  if (cfg.xLabelRotation === 90) return "rotate90";
  if (cfg.xLabelRotation === 45) return "rotate45";
  // BELOW the rotations, not above, and the order is the tie-break for a state Metabase's single
  // key cannot represent but egret's three CAN: a chart saved with Compact before a rotation was
  // picked would carry both. A rotation is the more visible of the two, so it is what the trigger
  // reports — and `xTicksPatch` clears the other key on every choice, so the pair can only arise
  // from a config written by hand or by an older build, never from this control.
  return cfg.xLabelCompact === true ? "compact" : "show";
}

/** EVERY key is always written, `undefined` included: picking Rotate 45° while hidden has to CLEAR
 *  `xAxisTicks`, picking Show has to clear the rotation, and picking either has to clear Compact.
 *  A patch that omitted the key it is not setting would leave the rest of the group behind and let
 *  the select display a state it did not put the config in.
 *
 *  THIS IS WHERE METABASE'S MUTUAL EXCLUSION LIVES. Upstream `graph.x_axis.axis_enabled` is ONE key
 *  holding one of five values, so "Compact and rotated 45°" is unrepresentable there. egret spells
 *  the same choice across three fields (see `CartesianConfig.xLabelRotation`'s note for why), which
 *  makes that combination expressible — so the exclusion has to be enforced by this function rather
 *  than by the shape. It is the only writer, which is what makes that enough. */
export function xTicksPatch(v: XTicksChoice): {
  xAxisTicks: false | undefined; xLabelRotation: 45 | 90 | undefined; xLabelCompact: true | undefined;
} {
  return {
    xAxisTicks: v === "hide" ? false : undefined,
    xLabelRotation: v === "rotate45" ? 45 : v === "rotate90" ? 90 : undefined,
    xLabelCompact: v === "compact" ? true : undefined,
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

/** Metabase's `graph.x_axis.axis_enabled` on the CATEGORY axis: Hide / Show / Compact / Rotate 45°
 *  / 90°, in Metabase's own order (`lib/settings/graph.ts:746-778`). */
export function CategoryTicksSelect({ axis, id, value, onChange, rotationsDisabled, offerCompact, compactDisabled, describedBy }: {
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
  /**
   * Whether "Compact" EXISTS on this chart at all — a STRUCTURAL question, which is why it is
   * absence rather than the greying below.
   *
   * Compact is a formatting flag on the category label text, so it needs a label formatter to act
   * through — and, since the flag is spread OVER the stored format, a stored format for it to be
   * spread over. The waterfall and the cartesian family both run their categories through a
   * `drawLabel` closure that names PERIODS, so there is always a label to shorten whether or not
   * anyone has set a format.
   *
   * THE BOX PLOT NOW HAS A FORMATTER AND STILL DOES NOT QUALIFY, which is a narrower reason than
   * the one recorded here before. `options/boxplot.ts`'s `catLabel` draws its category through that
   * column's own `column_settings` — it used to draw the raw group key and this comment said so —
   * but it names no period, so with nothing stored it still falls back to the raw cell and Compact
   * has nothing to act on. Offering it would give a control that works on the box plots whose
   * category someone has already formatted and silently does nothing on the rest, decided from a
   * different panel. A greyed item says "not here, but elsewhere on this chart"; "here, if you
   * first go and set an unrelated thing" is not a state a control can express.
   *
   * This overturns a decision recorded in this file's own section doc and in
   * `CartesianDisplaySection.tsx`, both of which withheld Compact from the waterfall on the grounds
   * that "a waterfall's x is a category". `options/waterfall.ts`'s `drawLabel` carries the
   * measurement that falsifies it for that chart and confirms it for this one.
   */
  offerCompact?: boolean;
  /** Greys "Compact" out for the SAME reason and in the same states as `rotationsDisabled` — a
   *  chart drawing a VALUE axis across has ECharts-generated tick labels rather than the category
   *  array, so neither the rotation nor the formatter is in play. Kept as its own prop rather than
   *  folded into `rotationsDisabled`, because the two coincide today by fact and not by definition:
   *  a caller that ever wanted one without the other should not have to unpick a shared name. */
  compactDisabled?: boolean;
  /** Id of the caption explaining that greying, hung off the TRIGGER — the disabled items
   *  themselves are unreachable by mouse, keyboard and screen reader alike. */
  describedBy?: string;
}) {
  return (
    <div className="space-y-1">
      <Label htmlFor={id} className="text-sm font-normal">Show lines and tick marks</Label>
      <Select value={value} onValueChange={(v) => onChange(v as XTicksChoice)}>
        <SelectTrigger id={id} aria-label={`${axis} show lines and tick marks`} className="w-full"
          aria-describedby={describedBy}><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="hide">Hide</SelectItem>
          <SelectItem value="show">Show</SelectItem>
          {offerCompact && <SelectItem value="compact" disabled={compactDisabled}>Compact</SelectItem>}
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
        <SelectTrigger id={id} aria-label={`${axis} show lines and tick marks`} className="w-full"><SelectValue /></SelectTrigger>
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
        <div className="space-y-2">
          <label className="space-y-1">
            <span className="text-xs text-muted-foreground">{axis} min</span>
            <DecimalInput placeholder="auto" aria-label={`${axis} min`}
              value={min} onChange={(yMin) => onChange({ yMin })} />
          </label>
          <label className="space-y-1">
            <span className="text-xs text-muted-foreground">{axis} max</span>
            <DecimalInput placeholder="auto" aria-label={`${axis} max`}
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
      <Input className="w-full"
        id={id} aria-label="Number of tick marks" type="number" min={2} max={20}
        placeholder="auto" value={value ?? ""}
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
      <div className="space-y-1.5">
        {/* "X label" / "Y label", the name cartesian's Axes tab gives the same row (KI-054). */}
        <Label htmlFor={id} className="text-sm font-normal">{axis} label</Label>
        <Switch id={`${id}-shown`} aria-label={`Show ${axis} label`} checked={shown} onCheckedChange={onShown} />
      </div>
      {shown && (
        <Input className="w-full" id={id} aria-label={`${axis} label`} placeholder="Auto"
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
 * at 30° on a guess and offered no override.
 *
 * **"Compact" USED TO BE WITHHELD FROM BOTH CHARTS HERE, and that was half wrong.** The reason
 * recorded was that "it shortens temporal axis labels, and these charts' x is a category run
 * through its column format, so the option would do nothing here". That is true of the box plot and
 * false of the waterfall: `options/waterfall.ts` runs its steps through a `drawLabel` with the same
 * two arms as the cartesian builder's, `namesAPeriod` and `periodName`, so a month-bucketed
 * waterfall draws "January 2026" and has a label to shorten. The box plot names no period: its
 * category axis now has an `axisLabel.formatter` (`options/boxplot.ts`'s `catLabel`, which reads
 * the category's own `column_settings`) but falls back to the raw cell whenever nothing is stored,
 * so there is no state of it in which Compact reliably does anything — see `offerCompact`'s own
 * comment, which carries the narrowed argument. The two charts that share this tab differ on this
 * one point, so it is a PROP rather than a fact about the tab — which is what stops the next reader
 * inferring the withheld half from the offered one, as this comment invited.
 *
 * The same is true in the other direction for "Unpin from zero": the box plot has it and the
 * waterfall does not, because `isYAxisUnpinFromZeroValid` excludes a waterfall display outright.
 * Two charts, two settings, and each chart gets exactly one of them.
 */
export function ChartAxesSection<C extends AxesConfig>({ config, onChange, idPrefix = "ax", offerCompact, offerUnpin }: {
  config: C;
  onChange: (patch: Partial<C>) => void;
  /** Distinct per chart so two panels on one page cannot share a label's `htmlFor`. */
  idPrefix?: string;
  /** See `CategoryTicksSelect`'s prop of the same name — true for the waterfall, false for the box
   *  plot, and the asymmetry is measured rather than assumed. */
  offerCompact?: boolean;
  /** …and its mirror image: true for the box plot, false for the waterfall. */
  offerUnpin?: boolean;
}) {
  return (
    <div className="space-y-3">
      <AxisGroup axis="X">
        <AxisTitleRow id={`${idPrefix}-xlabel`} axis="X" label={config.xLabel} shown={config.showXLabel !== false}
          onLabel={(v) => onChange({ xLabel: v } as Partial<C>)} onShown={(v) => onChange({ showXLabel: v ? undefined : false } as Partial<C>)} />
        {/* No `rotationsDisabled`: these two charts' x is always the ordinal sequence of categories,
            so the rotation always applies. Cartesian's tab passes it, because a linear/timeseries x
            draws a value axis whose labels `scaleAxis` gives no `rotate`. */}
        {/* No `compactDisabled` beside `offerCompact` for the same reason there is no
            `rotationsDisabled`: these two charts' x is always the ordinal sequence of categories,
            so where the option exists at all it always applies. */}
        <CategoryTicksSelect axis="X" id={`${idPrefix}-xticks`} value={xTicksChoiceOf(config)}
          offerCompact={offerCompact}
          onChange={(v) => onChange(xTicksPatch(v) as Partial<C>)} />
      </AxisGroup>

      <AxisGroup axis="Y" className="border-t pt-3">
        <AxisTitleRow id={`${idPrefix}-ylabel`} axis="Y" label={config.yLabel} shown={config.showYLabel !== false}
          onLabel={(v) => onChange({ yLabel: v } as Partial<C>)} onShown={(v) => onChange({ showYLabel: v ? undefined : false } as Partial<C>)} />
        <ValueRangeRows axis="Y" idPrefix={idPrefix} min={config.yMin} max={config.yMax}
          onChange={(patch) => onChange(patch as Partial<C>)} />
        {/* IMMEDIATELY BELOW the auto-range switch, which is where Metabase puts it — `auto_range` is
            index 4 and `unpin_from_zero` index 5 in the same Y-axis group
            (`lib/settings/graph.ts:791`, `:817`) — and the adjacency is the explanation: the row
            DISAPPEARS when auto range is turned off, and a control that vanishes is much easier to
            read as a rule when the thing that banished it is the row above.

            HIDDEN, NOT GREYED, which is the opposite of what the rotations two blocks up do, so it
            needs saying. Metabase hides it (`getHidden`, `graph.ts:826-829`), and the local
            precedent agrees: `TickCountRow` below is gated away the same way by the same kind of
            neighbour. The rotations are greyed instead because a chart may have STORED one and must
            still show what it stored; there is nothing to show here, since `unpinFromZeroApplies`
            being false already means the builder ignores the stored flag outright. */}
        {offerUnpin && unpinFromZeroAppliesToBoxplot(config) && (
          <Toggle id={`${idPrefix}-unpin`} label="Unpin from zero" checked={config.unpinFromZero === true}
            onChange={(v) => onChange({ unpinFromZero: v || undefined } as Partial<C>)} />
        )}
        {/* Metabase's Scale select offers Linear / Power / Log. **Power is not offered**: ECharts'
            value axis has no power scale, Metabase implements its own, and a third option that
            silently drew a linear axis would be worse than its absence. */}
        <div className="space-y-1">
          <Label htmlFor={`${idPrefix}-scale`} className="text-sm font-normal">Y scale</Label>
          <Select value={config.logScale ? "log" : "linear"}
            onValueChange={(v) => onChange({ logScale: v === "log" || undefined } as Partial<C>)}>
            <SelectTrigger id={`${idPrefix}-scale`} aria-label="Y scale" className="w-full"><SelectValue /></SelectTrigger>
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
