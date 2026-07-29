import type { PieConfig } from "@/api/charts";
import { SERIES_PALETTE } from "@/components/chart/echarts/palette";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";
import { AutoToggle } from "./AutoToggle";

function Toggle({ id, label, checked, onChange }: { id: string; label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

// `slices` here is the distinct slice VALUES present in the current result (for the rows
// below) — not to be confused with `PieConfig.sliceNames`, which is a value -> label rename
// map. Same-sounding names, different shapes; keep them apart.
export function PieDisplaySection({ config, onChange, slices = [] }: { config: PieConfig; onChange: (patch: Partial<PieConfig>) => void; slices?: string[] }) {
  // Ranked exactly like SeriesOrderSection ranks cartesian series: by the stored sliceOrder
  // position, with anything unranked keeping its natural (incoming) place after the ranked
  // ones — buildPieOption applies this identical rule to ring 0, so the row order here always
  // matches what the chart itself draws.
  const rank = new Map((config.sliceOrder ?? []).map((n, i) => [n, i]));
  const ranked = [...slices].sort(
    (a, b) => (rank.get(a) ?? Number.MAX_SAFE_INTEGER) - (rank.get(b) ?? Number.MAX_SAFE_INTEGER),
  );
  const move = (i: number, delta: number) => {
    const j = i + delta;
    if (j < 0 || j >= ranked.length) return;
    const a = ranked[i];
    const b = ranked[j];
    // Merge into the STORED order, not the visible list: a slice hidden or not present in
    // this run's data still has a remembered position, and swapping two others must not
    // throw it away (same reasoning as SeriesOrderSection.move).
    const base = config.sliceOrder?.length ? [...config.sliceOrder] : [...ranked];
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
      {config.donut && (
        <Toggle id="pz-total" label="Show total" checked={!!config.showTotal} onChange={(v) => onChange({ showTotal: v })} />
      )}
      <AutoToggle
        id="pz-labels"
        label="Show labels"
        value={config.showLabels}
        onChange={(v) => onChange({ showLabels: v })}
        autoHint="Auto shows slice labels, hiding whichever ones would overlap"
      />
      <div className="flex items-center justify-between gap-2">
        <Label className="text-sm font-normal">Percentages</Label>
        <Select value={config.percentVisibility ?? "off"} onValueChange={(v) => onChange({ percentVisibility: v as PieConfig["percentVisibility"] })}>
          <SelectTrigger aria-label="Percentages" className="h-7 w-28"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="off">Off</SelectItem>
            <SelectItem value="inside">In chart</SelectItem>
            <SelectItem value="both">Both</SelectItem>
          </SelectContent>
        </Select>
      </div>
      {config.percentVisibility && config.percentVisibility !== "off" && (
        <div className="flex items-center justify-between gap-2">
          <Label htmlFor="pz-decimals" className="text-sm font-normal">Decimal places</Label>
          <Input id="pz-decimals" aria-label="Decimal places" type="number" min={0} max={10} step={1}
            className="h-7 w-16 text-sm"
            value={config.decimals ?? 2}
            onChange={(e) => {
              const n = Number(e.target.value);
              onChange({ decimals: Number.isNaN(n) ? undefined : Math.max(0, Math.trunc(n)) });
            }} />
        </div>
      )}
      <Toggle id="pz-sort" label="Sort by value" checked={config.sortRows !== false} onChange={(v) => onChange({ sortRows: v })} />
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor="pz-threshold" className="text-sm font-normal">Group slices below</Label>
        <div className="flex items-center gap-1">
          <Input id="pz-threshold" aria-label="Group slices below" type="number" min={0} max={100} step={0.5}
            className="h-7 w-16 text-sm"
            value={config.sliceThreshold ?? 2.5}
            onChange={(e) => { const n = Number(e.target.value); onChange({ sliceThreshold: Number.isNaN(n) ? 0 : n }); }} />
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
          {ranked.map((name, i) => {
            const hidden = !!config.hiddenSlices?.includes(name);
            return (
              <div key={name} className="flex items-center gap-1.5">
                <button type="button" aria-label={`Move ${name} up`} disabled={i === 0}
                  className="text-xs text-muted-foreground disabled:opacity-40" onClick={() => move(i, -1)}>↑</button>
                <button type="button" aria-label={`Move ${name} down`} disabled={i === ranked.length - 1}
                  className="text-xs text-muted-foreground disabled:opacity-40" onClick={() => move(i, 1)}>↓</button>
                <input
                  type="color"
                  aria-label={`color ${name}`}
                  className="h-7 w-8 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
                  value={config.sliceColors?.[name] ?? SERIES_PALETTE[i % SERIES_PALETTE.length]}
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
        </div>
      )}
    </div>
  );
}
