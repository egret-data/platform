import type { CartesianConfig, CartesianViz } from "@/api/charts";
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

export function CartesianDisplaySection({
  kind, config, onChange,
}: {
  kind: CartesianViz;
  config: CartesianConfig;
  onChange: (patch: Partial<CartesianConfig>) => void;
}) {
  const isBar = kind === "bar";
  const isLineish = kind === "line" || kind === "area";
  const canStack = kind === "bar" || kind === "area";
  return (
    <div className="space-y-2.5 pt-1.5">
      <Toggle id="cz-legend" label="Legend" checked={!!config.showLegend} onChange={(v) => onChange({ showLegend: v })} />
      <Toggle id="cz-labels" label="Data labels" checked={!!config.showLabels} onChange={(v) => onChange({ showLabels: v })} />
      {canStack && <Toggle id="cz-stacked" label="Stacked" checked={!!config.stacked} onChange={(v) => onChange({ stacked: v })} />}
      {canStack && config.stacked && <Toggle id="cz-normalized" label="100% stacked" checked={!!config.normalized} onChange={(v) => onChange({ normalized: v })} />}
      {isBar && <Toggle id="cz-horizontal" label="Horizontal" checked={!!config.horizontal} onChange={(v) => onChange({ horizontal: v })} />}
      {isLineish && <Toggle id="cz-smooth" label="Smooth" checked={!!config.smooth} onChange={(v) => onChange({ smooth: v })} />}
      {kind !== "scatter" && <Toggle id="cz-log" label="Log scale" checked={!!config.logScale} onChange={(v) => onChange({ logScale: v })} />}

      {config.yMetrics.length > 1 && !config.horizontal && (
        <div className="space-y-1.5 border-t pt-2">
          <div className="text-xs text-muted-foreground">Y axis</div>
          {config.yMetrics.map((m) => {
            const onRight = (config.rightAxisMetrics ?? []).includes(m);
            return (
              <div key={m} className="flex items-center justify-between gap-2">
                <Label className="truncate text-sm font-normal">{m}</Label>
                <Select value={onRight ? "right" : "left"} onValueChange={(v) => {
                  const rest = (config.rightAxisMetrics ?? []).filter((x) => x !== m);
                  onChange({ rightAxisMetrics: v === "right" ? [...rest, m] : rest });
                }}>
                  <SelectTrigger aria-label={`axis ${m}`} className="h-7 w-24"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="left">Left</SelectItem>
                    <SelectItem value="right">Right</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            );
          })}
        </div>
      )}

      {kind === "combo" && config.yMetrics.length > 0 && (
        <div className="space-y-1.5 border-t pt-2">
          <div className="text-xs text-muted-foreground">Series type</div>
          {config.yMetrics.map((m) => (
            <div key={m} className="flex items-center justify-between gap-2">
              <Label className="truncate text-sm font-normal">{m}</Label>
              <Select value={config.seriesTypes?.[m] ?? "bar"} onValueChange={(v) => onChange({ seriesTypes: { ...config.seriesTypes, [m]: v as "bar" | "line" } })}>
                <SelectTrigger aria-label={`type ${m}`} className="h-7 w-24"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="bar">Bar</SelectItem>
                  <SelectItem value="line">Line</SelectItem>
                </SelectContent>
              </Select>
            </div>
          ))}
        </div>
      )}

      <div className="grid grid-cols-2 gap-2 border-t pt-2">
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">Goal value</span>
          <Input className="h-8" type="number" placeholder="none"
            value={config.goalValue ?? ""}
            onChange={(e) => { const n = Number(e.target.value); onChange({ goalValue: e.target.value === "" || Number.isNaN(n) ? undefined : n }); }} />
        </label>
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">Goal label</span>
          <Input className="h-8" placeholder="Goal" value={config.goalLabel ?? ""} onChange={(e) => onChange({ goalLabel: e.target.value || undefined })} />
        </label>
      </div>

      <div className="grid grid-cols-2 gap-2 border-t pt-2">
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">X label</span>
          <Input className="h-8" value={config.xLabel ?? ""} onChange={(e) => onChange({ xLabel: e.target.value })} />
        </label>
        <label className="space-y-1">
          <span className="text-xs text-muted-foreground">Y label</span>
          <Input className="h-8" value={config.yLabel ?? ""} onChange={(e) => onChange({ yLabel: e.target.value })} />
        </label>
      </div>
    </div>
  );
}
