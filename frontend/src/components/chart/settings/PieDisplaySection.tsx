import type { PieConfig } from "@/api/charts";
import { SERIES_PALETTE } from "@/components/chart/echarts/palette";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

function Toggle({ id, label, checked, onChange }: { id: string; label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

export function PieDisplaySection({ config, onChange, sliceNames = [] }: { config: PieConfig; onChange: (patch: Partial<PieConfig>) => void; sliceNames?: string[] }) {
  return (
    <div className="space-y-2.5 pt-1.5">
      <Toggle id="pz-legend" label="Legend" checked={!!config.showLegend} onChange={(v) => onChange({ showLegend: v })} />
      <Toggle id="pz-donut" label="Donut" checked={!!config.donut} onChange={(v) => onChange({ donut: v })} />
      {config.donut && (
        <Toggle id="pz-total" label="Show total" checked={!!config.showTotal} onChange={(v) => onChange({ showTotal: v })} />
      )}
      <Toggle id="pz-labels" label="Show labels" checked={!!config.showLabels} onChange={(v) => onChange({ showLabels: v })} />
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

      {sliceNames.length > 0 && (
        <div className="space-y-1.5 border-t pt-2">
          <div className="text-xs text-muted-foreground">Slice colors</div>
          {sliceNames.map((name, i) => (
            <div key={name} className="flex items-center gap-2">
              <input
                type="color"
                aria-label={`color ${name}`}
                className="h-7 w-8 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
                value={config.sliceColors?.[name] ?? SERIES_PALETTE[i % SERIES_PALETTE.length]}
                onChange={(e) => onChange({ sliceColors: { ...config.sliceColors, [name]: e.target.value } })}
              />
              <span className="flex-1 truncate text-sm">{name}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
