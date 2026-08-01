import { CHART_TYPES } from "@/components/chart/chartTypes";
import { MAPS } from "@/components/chart/echarts/maps/registry";
import { Label } from "@/components/ui/label";
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from "@/components/ui/select";
import { isParamsV2 } from "@/api/charts";
import type { MapConfig, QuerySpec } from "@/api/charts";
import type { SharedQueryState, VizControlProps, VizDescriptor } from "./types";
import { SERIES_PALETTE } from "@/components/chart/echarts/palette";

const LABEL_CLASS = "text-xs font-medium uppercase tracking-wide text-muted-foreground";

function MapControls({ config, onChange, columns, metrics }: VizControlProps<MapConfig>) {
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Map</Label>
        <Select value={config.mapKey} onValueChange={(v) => onChange({ mapKey: v })}>
          <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
          <SelectContent>
            {Object.values(MAPS).map((m) => <SelectItem key={m.key} value={m.key}>{m.label}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Region</Label>
        <Select value={config.region ?? ""} onValueChange={(v) => onChange({ region: v || undefined })}>
          <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a region column…" /></SelectTrigger>
          <SelectContent>
            {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Metric</Label>
        <Select value={config.metric ?? ""} onValueChange={(v) => onChange({ metric: v || undefined })}>
          <SelectTrigger className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
          <SelectContent>
            {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
    </>
  );
}

/** Metabase's `map.colors`. ONE accent, because both ends of the choropleth ramp are derived from
 *  it (see `choroplethScale`) — offering two swatches would let them disagree, which is the state
 *  the derivation exists to prevent. */
function MapColorControl({ config, onChange }: VizControlProps<MapConfig>) {
  return (
    <div className="flex items-center justify-between gap-2">
      <Label className="text-sm font-normal">Color</Label>
      <input
        type="color" aria-label="Color"
        className="h-7 w-10 shrink-0 cursor-pointer rounded border bg-transparent p-0.5"
        value={config.color ?? SERIES_PALETTE[0]}
        onChange={(e) => onChange({ color: e.target.value })}
      />
    </div>
  );
}

export function mapDescriptor(): VizDescriptor<MapConfig> {
  return {
    key: "map",
    meta: CHART_TYPES.find((t) => t.key === "map")!,
    prefix: "map.",
    defaultConfig: { mapKey: "us_states" },
    fromParams: (p) => {
      if (isParamsV2(p)) {
        return {
          mapKey: (p.settings["map.key"] as string) ?? "us_states",
          region: p.settings["map.region"] as string | undefined,
          metric: p.settings["map.metric"] as string | undefined,
          color: p.settings["map.colors"] as string | undefined,
        };
      }
      return p.map ?? { mapKey: "us_states" };
    },
    toSettings: (cfg) => {
      const out: Record<string, unknown> = { "map.key": cfg.mapKey };
      if (cfg.region !== undefined) out["map.region"] = cfg.region;
      if (cfg.metric !== undefined) out["map.metric"] = cfg.metric;
      if (cfg.color !== undefined) out["map.colors"] = cfg.color;
      return out;
    },
    requiredFields: (cfg) => {
      const out: string[] = [];
      if (!cfg.region) out.push("a region");
      if (!cfg.metric) out.push("a metric");
      return out;
    },
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => ({
      metrics: cfg.metric ? [cfg.metric] : [],
      dimensions: cfg.region ? [cfg.region] : [],
      columns: [], filters: shared.filters, orderby: [], row_limit: shared.rowLimit, rollup_groups: [],
    }),
    controls: [
      { id: "map", section: "data", render: MapControls },
      { id: "color", section: "display", render: MapColorControl },
    ],
  };
}
