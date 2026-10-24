import type { PieConfig } from "@/api/charts";
import { colorsForKeys } from "@/components/chart/echarts/keyColors";
import { OTHER_SLICE_COLOR } from "@/components/chart/echarts/palette";
import {
  DndContext, KeyboardSensor, PointerSensor, useSensor, useSensors, closestCenter,
  type DragEndEvent,
} from "@dnd-kit/core";
import { SortableContext, verticalListSortingStrategy, sortableKeyboardCoordinates } from "@dnd-kit/sortable";
import { GripVertical } from "lucide-react";
import { SortableRow } from "@/components/chart/config/SortableRow";
import { reorderSeriesOrder } from "./SeriesListSection";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";
import { AutoToggle } from "./AutoToggle";
import { DecimalInput } from "./DecimalInput";
import { resolvePercentVisibility, resolveShowLabels, DEFAULT_SLICE_THRESHOLD } from "@/components/chart/echarts/options/pie";
import { ColorPicker } from "./ColorPicker";

// A Radix Select item cannot carry an empty value, so "leave it unset" needs a sentinel that
// no real percentVisibility can collide with.
const AUTO = "auto";

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
  // The number of rings the chart will actually draw — the same `dimensions` list buildPieOption
  // turns into rings. It decides whether there is a HOLE (a multi-ring pie always has one,
  // whatever `donut` says) and what "Auto" means for slice labels (`resolveShowLabels`); it does
  // NOT decide the percentage default, which is "legend" at every ring count, matching Metabase's
  // constant `getDefaultPercentVisibility`.
  const ringCount = config.dimensions?.length ?? 0;
  const hasHole = !!config.donut || ringCount > 1;
  const effectivePercent = resolvePercentVisibility(config.percentVisibility);
  const autoPercentHint = "puts each innermost-ring slice's share beside its name in the legend";

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
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

  function onDragEnd(e: DragEndEvent) {
    // The SAME rule and the same helper the series list uses: move inside the STORED order so a
    // slice hidden or absent from this run keeps its remembered position.
    const next = reorderSeriesOrder(config.sliceOrder, listed, e.active.id, e.over?.id);
    if (next) onChange({ sliceOrder: next });
  }
  return (
    <div className="space-y-2.5 pt-1.5">
      <AutoToggle
        id="pz-legend"
        label="Legend"
        value={config.showLegend}
        onChange={(v) => onChange({ showLegend: v })}
        autoHint="Auto shows the legend — a pie's colors are meaningless without it"
      />
      <Toggle id="pz-donut" label="Donut" checked={!!config.donut} onChange={(v) => onChange({ donut: v })} />
      {/* Offered whenever the chart HAS a hole to write in — a multi-ring pie always does,
          whatever `donut` says (pieGeometry), so gating this on `donut` alone would leave the
          multi-ring total, which defaults on, with no way to turn it off.

          STILL GATED, and deliberately so, even though Metabase offers `pie.show_total`
          unconditionally: Metabase has no `pie.donut` setting and applies its 3/5 inner radius to
          every pie, so a holeless pie is a shape it cannot draw and its unconditional control is
          not a claim about one. egret's holeless pie has nowhere to put a total — `pieHoleModel`
          requires `holeRatio > 0` — so an offered switch here would move nothing on screen, which
          is the exact fault this section exists to remove.

          Auto/On/Off rather than a Switch because `undefined` is a real third state the chart
          resolves at render time, and a two-state Switch bound to `!!config.showTotal` would read
          OFF while the hole carried the total. */}
      {hasHole && (
        <AutoToggle
          id="pz-total"
          label="Show total"
          value={config.showTotal}
          onChange={(v) => onChange({ showTotal: v })}
          autoHint="Auto writes the grand total into the hole"
        />
      )}
      {/* Auto is not one behaviour: `resolveShowLabels` (shared with the builder, so the two
          cannot drift) leaves a one-ring pie's arcs bare and names its slices in the legend
          instead, and draws them on the arcs from two rings out — where the legend lists ring 0
          only and the outer rings would otherwise have no key at all. Metabase's
          `getDefaultShowLabels` makes the same split. The hint therefore has to be computed:
          a fixed "Auto shows slice labels" was true of exactly half the pies it appeared on. */}
      <AutoToggle
        id="pz-labels"
        label="Show labels"
        value={config.showLabels}
        onChange={(v) => onChange({ showLabels: v })}
        autoHint={resolveShowLabels(undefined, ringCount)
          ? "Auto writes each slice's name on its arc, hiding whichever ones would overlap"
          : "Auto leaves the arcs bare — a single ring's slices are named in the legend"}
      />
      {/* Auto is a real option, not decoration: with nothing stored, a pie draws its percentages
          in the legend at any ring count (see resolvePercentVisibility, which the chart and this
          control share so they cannot drift). A four-value select with no Auto would have to
          show "Off" while the chart drew them. A stored value that isn't one of the four shows
          — and renders as — Off, which is what the chart does with it. */}
      <div className="space-y-1.5">
        <Label className="text-sm font-normal">Percentages</Label>
        <Select
          value={config.percentVisibility === undefined ? AUTO : effectivePercent}
          onValueChange={(v) => onChange({
            percentVisibility: v === AUTO ? undefined : (v as PieConfig["percentVisibility"]),
          })}
        >
          <SelectTrigger className="w-full" aria-label="Percentages"><SelectValue /></SelectTrigger>
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
        <div className="space-y-1.5">
          <Label htmlFor="pz-decimals" className="text-sm font-normal">Decimal places</Label>
          {/* EMPTY, not "2", when nothing is stored: the chart no longer has a fixed default to
              show. Unset means the pie derives the precision from its own shares (options/pie.ts,
              computeMaxPercentDecimals) — a number in this box would be a control claiming a value
              the chart isn't using, which is the same fault this panel already fixed for the
              legend, the slice labels, the centre total and Percentages' own Auto. Clearing the
              box is also the only way back to that, which typing a default over it never was. */}
          <Input id="pz-decimals" aria-label="Decimal places" type="number" min={0} max={10} step={1}
            className="w-full text-sm" placeholder="Auto"
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
      <div className="space-y-1.5">
        <Label htmlFor="pz-threshold" className="text-sm font-normal">Group slices below</Label>
        <div className="flex items-center gap-1">
          {/* The control this whole family now shares (DecimalInput). This box is where the
              defect was first measured: 0 is not "unset" here, it is the setting that switches the
              "Other" fold OFF entirely (charts.ts: "0 = off"), so `Number("")` reaching the config
              mid-decimal did not blank a field, it silently chose a different chart and then
              showed "0" with no route back to the default.

              `unsetText` rather than a placeholder alone: this control displays the percentage the
              pie is ACTUALLY folding at, so an empty box would be claiming there is no threshold
              when there is one. Clearing it is still the only way back to that default, and now
              lands there instead of on 0. */}
          <DecimalInput
            id="pz-threshold" aria-label="Group slices below"
            className="text-sm"
            placeholder={String(DEFAULT_SLICE_THRESHOLD)}
            unsetText={String(DEFAULT_SLICE_THRESHOLD)}
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
          <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
          <SortableContext items={listed} strategy={verticalListSortingStrategy}>
          {listed.map((name) => {
            const hidden = !!config.hiddenSlices?.includes(name);
            return (
              <SortableRow key={name} id={name} className="flex items-center gap-1.5">
              {({ attributes, listeners }) => (<>
                {/* The pie had its OWN reorder buttons, which is why the first pass at this — which
                    only touched SeriesListSection and TableColumnsSection — left eight 9×16 targets
                    here. Same `IconButton` as the other two lists now. */}
                <button type="button"
                  className="flex size-6 shrink-0 cursor-grab items-center justify-center rounded text-muted-foreground hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset"
                  {...attributes} {...listeners}
                  aria-label={`reorder ${name}`}>
                  <GripVertical className="h-3.5 w-3.5" />
                </button>
                <ColorPicker
                  label={`color ${name}`}
                  value={config.sliceColors?.[name] ?? defaultSliceColor(name)}
                  onChange={(next) => onChange({ sliceColors: { ...config.sliceColors, [name]: next } })}
                />
                <Input
                  className="flex-1"
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
              </>)}
              </SortableRow>
            );
          })}
          </SortableContext>
          </DndContext>
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
