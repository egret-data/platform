import { useState } from "react";
import type { PieConfig } from "@/api/charts";
import { colorsForKeys } from "@/components/chart/echarts/keyColors";
import { OTHER_SLICE_COLOR } from "@/components/chart/echarts/palette";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";
import { AutoToggle } from "./AutoToggle";
import { resolvePercentVisibility, DEFAULT_SLICE_THRESHOLD } from "@/components/chart/echarts/options/pie";

// A Radix Select item cannot carry an empty value, so "leave it unset" needs a sentinel that
// no real percentVisibility can collide with.
const AUTO = "auto";

/**
 * The "Group slices below" percentage, which keeps the number BEING TYPED into it.
 *
 * `type="number"` cannot hold a half-written decimal. The moment "4." exists the contents stop
 * being a valid number, so the browser reports `value === ""` (`validity.badInput`) — and the old
 * handler ran that through `Number("")`, which is 0, and stored it. Measured in Chrome, typing
 * 4 . 5 over the default went: "4" → **"0"** → "5". The dot wiped the field and the user who meant
 * 4.5 got 5. Clearing the box did the same thing and stayed there.
 *
 * 0 IS NOT "UNSET" HERE. It is the setting that switches the "Other" fold off entirely
 * (charts.ts: "0 = off"), so that was not a cosmetic slip — the control silently chose a
 * different chart, then displayed "0" with no route back to the default.
 *
 * So: a TEXT input, with the in-progress characters held here and a number committed only once
 * they parse. Empty means UNSET — `undefined`, which the pie resolves to DEFAULT_SLICE_THRESHOLD —
 * which is also what makes the default reachable again. The draft is dropped on blur so the box
 * settles on whatever the chart is actually using ("4." normalises to 4, empty back to 2.5).
 *
 * This is the shape Metabase's own `ChartSettingInputNumeric` uses for this very setting
 * (`pie.slice_threshold`): a `TextInput`, an `inputValue` state, and `""` → `onChange(undefined)`.
 */
function SliceThresholdInput({ value, onChange }: { value: number | undefined; onChange: (v: number | undefined) => void }) {
  // null = "not being edited"; show what is stored. Any string, including "", is a live draft.
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <Input
      id="pz-threshold" aria-label="Group slices below"
      // inputMode gets the numeric keypad on touch without handing the field back to the
      // number-input value sanitiser that caused this.
      type="text" inputMode="decimal"
      className="h-7 w-16 text-sm"
      placeholder={String(DEFAULT_SLICE_THRESHOLD)}
      value={draft ?? String(value ?? DEFAULT_SLICE_THRESHOLD)}
      onChange={(e) => {
        const raw = e.target.value;
        setDraft(raw);
        if (raw.trim() === "") return onChange(undefined);
        const n = Number(raw);
        // Anything unparseable ("4.5.6", "abc") is left in the box and NOT committed — the last
        // good number stays in the config, and blur puts it back on screen.
        if (Number.isFinite(n)) onChange(n);
      }}
      onBlur={() => setDraft(null)}
    />
  );
}

function Toggle({ id, label, checked, onChange }: { id: string; label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

// `slices` here is ring 0's slice VALUES in the order the chart draws them (for the rows below) —
// not to be confused with `PieConfig.sliceNames`, which is a value -> label rename map.
// Same-sounding names, different shapes; keep them apart.
//
// `foldedSlices` is the subset of them the chart merged into its "Other" bucket. Both come from
// `pieSliceRows` (echarts/options/pie.ts), the builder's own pass — this section cannot work either
// one out, because it is handed slice NAMES and the fold and the sort are both decided by their
// VALUES.
export function PieDisplaySection({ config, onChange, slices = [], foldedSlices = [] }: {
  config: PieConfig;
  onChange: (patch: Partial<PieConfig>) => void;
  slices?: string[];
  foldedSlices?: string[];
}) {
  // Ranked exactly like SeriesOrderSection ranks cartesian series: by the stored sliceOrder
  // position, with anything unranked keeping its natural place after the ranked ones —
  // buildPieRings applies this identical rule to ring 0. "Natural" is what `slices` arrives in,
  // which is now the order the chart draws (value-descending unless `sortRows` says otherwise);
  // it used to be the order the ROWS arrived in, which is why the same rule applied to the same
  // stored order still produced a list in an order the pie never drew.
  // The number of rings the chart will actually draw, which is what decides the percentage
  // default — the same `dimensions` list buildPieOption turns into rings.
  const ringCount = config.dimensions?.length ?? 0;
  const hasHole = !!config.donut || ringCount > 1;
  const effectivePercent = resolvePercentVisibility(config.percentVisibility, ringCount);
  const autoPercentHint = ringCount > 1
    ? "puts each innermost-ring slice's share beside its name in the legend"
    : "leaves percentages off a single-ring pie";

  const rank = new Map((config.sliceOrder ?? []).map((n, i) => [n, i]));
  const ranked = [...slices].sort(
    (a, b) => (rank.get(a) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b) ?? Number.MAX_SAFE_INTEGER),
  );
  /**
   * A FOLDED SLICE GETS NO ROW. The threshold merged it into "Other", so the chart draws no slice
   * of its own for it and a row would be offering a colour, a rename and a hide toggle that all
   * land on nothing — Metabase renders nothing for a row it marked `isOther` either. The bucket
   * itself is reported below instead, as a line rather than a row: it takes no curation on either
   * side (the builder identifies it by a sentinel no stored key can carry).
   *
   * Hidden slices are NOT in here — the fold is judged after they are dropped, so a hidden slice is
   * never folded — which is what keeps its own switch, the only route back to showing it, on screen.
   */
  const folded = new Set(foldedSlices);
  const listed = ranked.filter((n) => !folded.has(n));
  const foldedInOrder = ranked.filter((n) => folded.has(n));
  /**
   * The swatch a slice shows before any override — buildPieRings' own answer, asked the same way.
   *
   * It used to be `SERIES_PALETTE[i % length]`, a by-POSITION rule the renderer stopped following
   * when ring 0 moved to `colorsForKeys`: the panel then offered a colour the pie did not draw, and
   * a slice's swatch moved every time another slice was reordered.
   *
   * The key set is the slices the chart DRAWS. `hiddenSlices` rows are dropped before the ring is
   * built at all (buildPieRings), so a hidden slice is not a key and spends no colour — a hidden
   * row's own swatch therefore shows what it will be given when it comes back, which is its name
   * added to that set. `sliceColors` goes in as `existing` for the same reason it does there: an
   * override consumes its slot, so the slice that would have hashed onto it moves along.
   *
   * COLUMN FORMATS USED TO DIVERGE HERE and no longer do. `buildPieRings` keyed ring 0's colours
   * (and its hide, rename and order) on the slice's formatted LABEL while this panel is handed raw
   * values, so `columnSettings: { category: { prefix: "» " } }` made all four swatches disagree
   * with the chart. It now keys every one of them on the raw dimension value, which is the string
   * this panel writes — so hashing the raw values here asks the builder's own question. (Metabase
   * likewise hashes `getKeyFromDimensionValue(value)`, never the formatted name.)
   *
   * THE FOLD USED TO DIVERGE HERE and no longer does. Slices below the threshold are merged into
   * "Other" by `buildSlices` and drop out of the ring-0 key set, and this panel — handed slice
   * names with no values — could not run that fold, so it kept them as keys. That was not only a
   * missing row: a `sliceColors` pin on a FOLDED slice reaches `colorsForKeys` here and CONSUMES
   * its colour, while in the chart the folded slice is not a key at all and the colour stays free
   * for whoever hashes onto it. Measured: with `Gizmo` folded and pinned to `#f2a86f`, the panel
   * showed `Widget` as `#98d9d9` and the pie drew it `#F2A86F`. `foldedSlices` is what closes it —
   * the key set below is now the ring's own. (The synthetic bucket itself was never a problem: it
   * is created inside the builder and never reaches `slices`, and a real dimension value that
   * happens to be the string "Other" is a genuine key on both sides, which is why the builder
   * identifies the bucket by its OTHER_KEY sentinel rather than by name.)
   *
   * On a NESTED pie the swatch is the family's BASE colour, which ring 0 is then drawn as the dark
   * variant of (`ring0Variant`). That is deliberate rather than a divergence: whatever this control
   * shows is what a nudge of the picker stores, and the builder reads a stored colour as the base.
   * Showing the darkened one would make the pie darken it again on every edit.
   */
  const drawn = listed.filter((n) => !config.hiddenSlices?.includes(n));
  const drawnColors = colorsForKeys(drawn, config.sliceColors);
  const defaultSliceColor = (name: string) =>
    drawnColors[name] ?? colorsForKeys(listed.filter((n) => n === name || !config.hiddenSlices?.includes(n)), config.sliceColors)[name];

  const move = (i: number, delta: number) => {
    const j = i + delta;
    if (j < 0 || j >= listed.length) return;
    const a = listed[i];
    const b = listed[j];
    // Merge into the STORED order, not the visible list: a slice hidden or not present in
    // this run's data still has a remembered position, and swapping two others must not
    // throw it away (same reasoning as SeriesOrderSection.move).
    const base = config.sliceOrder?.length ? [...config.sliceOrder] : [...listed];
    for (const n of [a, b]) if (!base.includes(n)) base.push(n);
    const ia = base.indexOf(a);
    const ib = base.indexOf(b);
    [base[ia], base[ib]] = [base[ib], base[ia]];
    onChange({ sliceOrder: base });
  };
  return (
    <div className="space-y-2.5 pt-1.5">
      <AutoToggle
        id="pz-legend"
        label="Legend"
        value={config.showLegend}
        onChange={(v) => onChange({ showLegend: v })}
        autoHint="Auto shows the legend — a pie's colours are meaningless without it"
      />
      <Toggle id="pz-donut" label="Donut" checked={!!config.donut} onChange={(v) => onChange({ donut: v })} />
      {/* Offered whenever the chart HAS a hole to write in — a multi-ring pie always does,
          whatever `donut` says (buildPieOption's holeStart), so gating this on `donut` alone
          would leave the multi-ring total, which now defaults on, with no way to turn it off.
          Auto/On/Off rather than a Switch for that same reason: `undefined` genuinely means
          "on for a sunburst, off for a one-ring donut", which a Switch would have to lie
          about in one of the two cases. */}
      {hasHole && (
        <AutoToggle
          id="pz-total"
          label="Show total"
          value={config.showTotal}
          onChange={(v) => onChange({ showTotal: v })}
          autoHint="Auto fills a multi-ring pie's hole with the grand total, and leaves a single-ring donut's hole empty"
        />
      )}
      <AutoToggle
        id="pz-labels"
        label="Show labels"
        value={config.showLabels}
        onChange={(v) => onChange({ showLabels: v })}
        autoHint="Auto shows slice labels, hiding whichever ones would overlap"
      />
      {/* Auto is a real option, not decoration: with nothing stored, a multi-ring pie draws
          its percentages in the legend (see resolvePercentVisibility, which the chart and this
          control share so they cannot drift). A four-value select with no Auto would have to
          show "Off" while the chart drew them. A stored value that isn't one of the four shows
          — and renders as — Off, which is what the chart does with it. */}
      <div className="flex items-center justify-between gap-2">
        <Label className="text-sm font-normal">Percentages</Label>
        <Select
          value={config.percentVisibility === undefined ? AUTO : effectivePercent}
          onValueChange={(v) => onChange({
            percentVisibility: v === AUTO ? undefined : (v as PieConfig["percentVisibility"]),
          })}
        >
          <SelectTrigger aria-label="Percentages" className="h-7 w-28"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value={AUTO} title={`Auto ${autoPercentHint}`}>Auto</SelectItem>
            <SelectItem value="off">Off</SelectItem>
            <SelectItem value="legend">In legend</SelectItem>
            <SelectItem value="inside">In chart</SelectItem>
            <SelectItem value="both">Both</SelectItem>
          </SelectContent>
        </Select>
      </div>
      {effectivePercent !== "off" && (
        <div className="flex items-center justify-between gap-2">
          <Label htmlFor="pz-decimals" className="text-sm font-normal">Decimal places</Label>
          {/* EMPTY, not "2", when nothing is stored: the chart no longer has a fixed default to
              show. Unset means the pie derives the precision from its own shares (options/pie.ts,
              computeMaxPercentDecimals) — a number in this box would be a control claiming a value
              the chart isn't using, which is the same fault this panel already fixed for the
              legend, the slice labels, the centre total and Percentages' own Auto. Clearing the
              box is also the only way back to that, which typing a default over it never was. */}
          <Input id="pz-decimals" aria-label="Decimal places" type="number" min={0} max={10} step={1}
            className="h-7 w-16 text-sm" placeholder="Auto"
            value={config.decimals ?? ""}
            onChange={(e) => {
              const raw = e.target.value;
              if (raw === "") return onChange({ decimals: undefined });
              const n = Number(raw);
              onChange({ decimals: Number.isNaN(n) ? undefined : Math.max(0, Math.trunc(n)) });
            }} />
        </div>
      )}
      <Toggle id="pz-sort" label="Sort by value" checked={config.sortRows !== false} onChange={(v) => onChange({ sortRows: v })} />
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor="pz-threshold" className="text-sm font-normal">Group slices below</Label>
        <div className="flex items-center gap-1">
          <SliceThresholdInput
            value={config.sliceThreshold}
            onChange={(sliceThreshold) => onChange({ sliceThreshold })}
          />
          <span className="text-sm text-muted-foreground">%</span>
        </div>
      </div>

      {ranked.length > 0 && (
        <div className="space-y-1.5 border-t pt-2">
          <div className="text-xs text-muted-foreground">Slices</div>
          {/* Renaming, hiding, reordering and per-slice colour all key on the RAW value of the
              first dimension — the innermost ring — matching Metabase's pie.rows. Any nested
              ring beyond it is drawn from other dimensions and is unaffected by any of this;
              it exists to be scanned, not curated. */}
          <div className="text-xs text-muted-foreground">
            Applies to the innermost ring only — outer rings (additional nested dimensions) aren't affected.
          </div>
          {listed.map((name, i) => {
            const hidden = !!config.hiddenSlices?.includes(name);
            return (
              <div key={name} className="flex items-center gap-1.5">
                <button type="button" aria-label={`Move ${name} up`} disabled={i === 0}
                  className="text-xs text-muted-foreground disabled:opacity-40" onClick={() => move(i, -1)}>↑</button>
                <button type="button" aria-label={`Move ${name} down`} disabled={i === listed.length - 1}
                  className="text-xs text-muted-foreground disabled:opacity-40" onClick={() => move(i, 1)}>↓</button>
                <input
                  type="color"
                  aria-label={`color ${name}`}
                  className="h-7 w-8 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
                  value={config.sliceColors?.[name] ?? defaultSliceColor(name)}
                  onChange={(e) => onChange({ sliceColors: { ...config.sliceColors, [name]: e.target.value } })}
                />
                <Input
                  className="h-7 flex-1"
                  aria-label={`label ${name}`}
                  placeholder={name}
                  value={config.sliceNames?.[name] ?? ""}
                  onChange={(e) => {
                    // Clearing must delete the key, not store "" — buildPieOption only renames
                    // when `cfg.sliceNames[s.name]` is truthy, so an empty string sitting in the
                    // map would be indistinguishable from "not renamed" there, but would still
                    // show as a blank input here forever after. Deleting keeps both in sync.
                    const next = { ...config.sliceNames };
                    if (e.target.value) next[name] = e.target.value; else delete next[name];
                    onChange({ sliceNames: next });
                  }}
                />
                <Switch aria-label={`Show ${name}`} checked={!hidden}
                  onCheckedChange={(v) => onChange({
                    hiddenSlices: v ? (config.hiddenSlices ?? []).filter((n) => n !== name) : [...(config.hiddenSlices ?? []), name],
                  })} />
              </div>
            );
          })}
          {/* The fold, reported so the reader can see where the missing categories went — pinned
              last, which is where the chart draws the bucket (`buildPieRings` ranks it after every
              named slice, however `sliceOrder` is written). Deliberately NOT a row: there is
              nothing here to curate, and the controls a row carries would all land on nothing.
              Metabase shows even less — its list widget simply renders nothing for a folded slice —
              but a silently shorter list is the thing that sent a reader looking for the row that
              renames the slice they can see in the legend. */}
          {foldedInOrder.length > 0 && (
            <div className="flex items-start gap-1.5 pt-0.5 text-xs text-muted-foreground">
              <span className="mt-0.5 h-3 w-3 shrink-0 rounded-sm" style={{ background: OTHER_SLICE_COLOR }} />
              <span>Other — {foldedInOrder.join(", ")}, grouped by the threshold above</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
