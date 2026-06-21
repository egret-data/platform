import type { PieConfig } from "@/api/charts";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";

function Toggle({ id, label, checked, onChange }: { id: string; label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between">
      <Label htmlFor={id} className="text-sm font-normal">{label}</Label>
      <Switch id={id} aria-label={label} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

export function PieDisplaySection({ config, onChange }: { config: PieConfig; onChange: (patch: Partial<PieConfig>) => void }) {
  return (
    <div className="space-y-2.5 pt-1.5">
      <Toggle id="pz-legend" label="Legend" checked={!!config.showLegend} onChange={(v) => onChange({ showLegend: v })} />
      <Toggle id="pz-donut" label="Donut" checked={!!config.donut} onChange={(v) => onChange({ donut: v })} />
      <Toggle id="pz-labels" label="Show labels" checked={!!config.showLabels} onChange={(v) => onChange({ showLabels: v })} />
    </div>
  );
}
