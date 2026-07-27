import type { ReactNode } from "react";
import type { CartesianConfig, VizType } from "@/api/charts";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { FieldZone } from "@/components/chart/config/FieldZone";

const LABEL = "text-xs font-medium uppercase tracking-wide text-muted-foreground";

interface NamedColumn { column_name: string }
interface NamedMetric { metric_name: string }

/**
 * Left-panel Data controls for the cartesian family (bar/row/line/area/scatter/
 * combo): X-axis, optional scatter X-metric, Y-axis metrics, and the breakout
 * Series. Extracted from ChartExplore so the cartesian config lives in one place
 * (the shared temporal-unit row is injected via `renderUnitRow`).
 */
export function CartesianDataControls({ viz, config, onChange, columns, metrics, renderUnitRow }: {
  viz: VizType;
  config: CartesianConfig;
  onChange: (patch: Partial<CartesianConfig>) => void;
  columns: NamedColumn[];
  metrics: NamedMetric[];
  renderUnitRow: (col?: string) => ReactNode;
}) {
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL}>X-axis</Label>
        <Select value={config.xAxis ?? ""} onValueChange={(v) => onChange({ xAxis: v || undefined })}>
          <SelectTrigger aria-label="X-axis" className="h-8 text-sm"><SelectValue placeholder="Pick a dimension…" /></SelectTrigger>
          <SelectContent>
            {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      {renderUnitRow(config.xAxis)}

      {viz === "scatter" && (
        <div className="space-y-1">
          <Label className={LABEL}>X metric (optional)</Label>
          <Select value={config.xMetric ?? "__none"} onValueChange={(v) => onChange({ xMetric: v === "__none" ? undefined : v })}>
            <SelectTrigger aria-label="X metric" className="h-8 text-sm"><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="__none">None — use the dimension</SelectItem>
              {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
            </SelectContent>
          </Select>
        </div>
      )}

      <FieldZone label="Y-axis" selected={config.yMetrics} onChange={(v) => onChange({ yMetrics: v })}
        options={metrics.map((m) => m.metric_name)} />

      <div className="space-y-1">
        <Label className={LABEL}>Series</Label>
        <Select value={config.series ?? "__none"} onValueChange={(v) => onChange({ series: v === "__none" ? undefined : v })}>
          <SelectTrigger aria-label="Series breakout" className="h-8 text-sm"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="__none">None</SelectItem>
            {columns.filter((c) => c.column_name !== config.xAxis).map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
          </SelectContent>
        </Select>
        {config.series && config.yMetrics.length > 1 && (
          <p className="text-xs text-muted-foreground">With a series, only the first metric is plotted.</p>
        )}
      </div>
    </>
  );
}
