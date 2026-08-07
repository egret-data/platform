import { CHART_TYPES } from "@/components/chart/chartTypes";
import { MAPS } from "@/components/chart/echarts/maps/registry";
import { Label } from "@/components/ui/label";
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from "@/components/ui/select";
import { isParamsV2 } from "@/api/charts";
import type { MapConfig, MapType, QuerySpec } from "@/api/charts";
import type { SharedQueryState, VizControlProps, VizDescriptor } from "./types";
import { SERIES_PALETTE } from "@/components/chart/echarts/palette";

const LABEL_CLASS = "text-xs font-medium uppercase tracking-wide text-muted-foreground";

/**
 * WHICH MAP the rows are painted onto — Metabase's "Region map".
 *
 * ITS OWN CONTROL, split out of the three-field block below, and the split is the fix rather than an
 * organisational tidy-up. All three used to render from one `{ id: "map", section: "data" }` entry,
 * and `CardVizPanel` filters at CONTROL granularity — so `mapKey` was excluded from dashboard cards
 * as a side effect of travelling with region and metric, which genuinely are the query. A US-states
 * card could not be shown as World or Vietnam, and nothing on screen said why.
 *
 * A reclassification could not have fixed it: whatever `section` this entry carried, all three
 * fields carried it together. The control had to come apart first, which is why F2's fix has a
 * different shape from F1's even though the two share a root cause.
 *
 * It renders FIRST, so the builder's Data tab still reads Map, Region, Metric top to bottom —
 * ungrouped controls render in declaration order, so the order is the array's.
 */
function MapTypeControl({ config, onChange }: VizControlProps<MapConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Map</Label>
      <Select value={config.mapKey} onValueChange={(v) => onChange({ mapKey: v })}>
        <SelectTrigger aria-label="Map" className="h-8 text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          {Object.values(MAPS).map((m) => <SelectItem key={m.key} value={m.key}>{m.label}</SelectItem>)}
        </SelectContent>
      </Select>
    </div>
  );
}

/**
 * WHAT THE ROWS BECOME — Metabase's `map.type`, a select above the fields it decides.
 *
 * It renders SECOND, under Map, because it changes which fields appear below it: reading down the
 * panel you pick the outlines, then what to draw on them, then the columns that draw it.
 *
 * `cardOverridable` is deliberately NOT set here, unlike `mapKey` just above. The distinction that
 * flag encodes is "does this touch the query", and this one is the field `buildSpec` switches on:
 * a region map aggregates a metric by a dimension and a pin map asks for raw records. Letting a
 * dashboard card flip it would re-point the query the dashboard's filters are reasoning about —
 * and it would land on a card whose chart has no latitude column configured at all, so the
 * override could not even render. `mapKey` is overridable precisely because it does the opposite:
 * it paints the SAME rows onto different outlines.
 */
function MapTypeSelect({ config, onChange }: VizControlProps<MapConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Map type</Label>
      <Select value={config.type ?? "region"} onValueChange={(v) => onChange({ type: v as MapType })}>
        <SelectTrigger aria-label="Map type" className="h-8 text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="region">Region map</SelectItem>
          <SelectItem value="pin">Pin map</SelectItem>
        </SelectContent>
      </Select>
    </div>
  );
}

/** The two fields that ARE a REGION map's query — `buildSpec` reads exactly these — so they stay
 *  out of a dashboard card's panel. See `VizControl.cardOverridable`.
 *
 *  Renders NOTHING in pin mode, rather than rendering disabled: Metabase hides these behind
 *  `getHidden: vizSettings["map.type"] !== "region"` for the same reason, and a Region picker on a
 *  pin map is a control that saves and does not paint. */
function MapControls({ config, onChange, columns, metrics }: VizControlProps<MapConfig>) {
  if ((config.type ?? "region") !== "region") return null;
  return (
    <>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Region</Label>
        <Select value={config.region ?? ""} onValueChange={(v) => onChange({ region: v || undefined })}>
          <SelectTrigger aria-label="Region" className="h-8 text-sm"><SelectValue placeholder="Pick a region column…" /></SelectTrigger>
          <SelectContent>
            {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Metric</Label>
        <Select value={config.metric ?? ""} onValueChange={(v) => onChange({ metric: v || undefined })}>
          <SelectTrigger aria-label="Metric" className="h-8 text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
          <SelectContent>
            {metrics.map((m) => <SelectItem key={m.metric_name} value={m.metric_name}>{m.metric_name}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
    </>
  );
}

/**
 * A PIN MAP'S QUERY: where each marker goes, and what it is called.
 *
 * ONLY NUMERIC COLUMNS ARE OFFERED AS COORDINATES — Metabase's `fieldFilter: isNumeric` on both
 * `map.latitude_column` and `map.longitude_column`. Picking a varchar produces a chart of zero
 * pins and no error message, which is the silent-empty-map failure the region picker already had
 * once (see `resolveRegion`).
 *
 * WHAT EGRET CANNOT COPY FROM METABASE HERE, stated because it looks like an omission: Metabase
 * DEFAULTS these two fields, by finding the column whose semantic type is `type/Latitude`. egret's
 * `semantic_type` vocabulary has no latitude or longitude in it (ColumnsTab offers category,
 * country, currency, email, url, image), so there is nothing to search for and the fields start
 * empty. Guessing from column NAMES was considered and rejected: a heuristic that silently picks
 * the wrong column plots a map that is confidently in the wrong hemisphere, which is worse than an
 * empty picker that asks. The real fix is a latitude/longitude semantic type, which is a backend
 * enum plus the dataset column editor — a different change from this one.
 */
function PinControls({ config, onChange, columns }: VizControlProps<MapConfig>) {
  if (config.type !== "pin") return null;
  const numeric = columns.filter((c) => c.type_generic === "numeric");
  const field = (
    label: string, value: string | undefined, opts: typeof columns, onPick: (v: string | undefined) => void,
  ) => (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>{label}</Label>
      <Select value={value ?? ""} onValueChange={(v) => onPick(v || undefined)}>
        <SelectTrigger aria-label={label} className="h-8 text-sm">
          <SelectValue placeholder={`Pick a ${label.toLowerCase()} column…`} />
        </SelectTrigger>
        <SelectContent>
          {opts.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
        </SelectContent>
      </Select>
    </div>
  );
  return (
    <>
      {field("Latitude", config.latitude, numeric, (v) => onChange({ latitude: v }))}
      {field("Longitude", config.longitude, numeric, (v) => onChange({ longitude: v }))}
      {/* Any column, numeric or not — the label is what the pin SAYS, not where it goes, and the
          names a reader wants ("San Francisco") are exactly the non-numeric ones. */}
      {field("Label", config.label, columns, (v) => onChange({ label: v }))}
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
    defaultConfig: { mapKey: "us_states", type: "region" },
    fromParams: (p) => {
      if (isParamsV2(p)) {
        return {
          mapKey: (p.settings["map.key"] as string) ?? "us_states",
          // A chart saved before pin mode existed carries no `map.type`, and every one of them is
          // a region map. Resolved HERE rather than left undefined so that config in hand always
          // states its type — the panel's Select needs a defined value, and every reader of
          // `cfg.type` downstream would otherwise have to repeat the same `?? "region"`.
          type: ((p.settings["map.type"] as MapType | undefined) ?? "region"),
          region: p.settings["map.region"] as string | undefined,
          metric: p.settings["map.metric"] as string | undefined,
          color: p.settings["map.colors"] as string | undefined,
          latitude: p.settings["map.latitude_column"] as string | undefined,
          longitude: p.settings["map.longitude_column"] as string | undefined,
          label: p.settings["map.label_column"] as string | undefined,
        };
      }
      // v1 params predate pin mode entirely, so the nested blob can only describe a region map.
      return { type: "region", ...(p.map ?? { mapKey: "us_states" }) };
    },
    toSettings: (cfg) => {
      // `map.type` is written ALWAYS, unlike the optional fields below, so that a chart switched
      // from pin back to region stores that fact rather than falling through to the same default an
      // un-migrated chart gets. The keys below are Metabase's own names.
      const out: Record<string, unknown> = { "map.key": cfg.mapKey, "map.type": cfg.type ?? "region" };
      if (cfg.region !== undefined) out["map.region"] = cfg.region;
      if (cfg.metric !== undefined) out["map.metric"] = cfg.metric;
      if (cfg.color !== undefined) out["map.colors"] = cfg.color;
      if (cfg.latitude !== undefined) out["map.latitude_column"] = cfg.latitude;
      if (cfg.longitude !== undefined) out["map.longitude_column"] = cfg.longitude;
      if (cfg.label !== undefined) out["map.label_column"] = cfg.label;
      return out;
    },
    // Each branch names ONLY its own fields. A pin map that still demanded a region and a metric
    // would be permanently unrunnable, and the preview would ask for two columns it never uses.
    requiredFields: (cfg) => {
      const out: string[] = [];
      if (cfg.type === "pin") {
        if (!cfg.latitude) out.push("a latitude");
        if (!cfg.longitude) out.push("a longitude");
        return out;
      }
      if (!cfg.region) out.push("a region");
      if (!cfg.metric) out.push("a metric");
      return out;
    },
    buildSpec: (cfg, shared: SharedQueryState): QuerySpec => {
      const base = {
        filters: shared.filters, orderby: shared.orderby, row_limit: shared.rowLimit, rollup_groups: [],
      };
      if (cfg.type === "pin") {
        // RAW RECORDS, WITH NO BACKEND CHANGE. `chartquery.rs` takes the no-GROUP-BY branch
        // whenever `metrics` is empty and `columns` is not, emitting
        // `SELECT <col> AS <col> FROM … WHERE … ORDER BY … LIMIT` — which is exactly a pin map's
        // shape, one row per marker. Both halves matter: a stray metric here would send it back
        // down the aggregate branch and collapse every pin sharing a coordinate into one.
        const columns = [cfg.latitude, cfg.longitude, cfg.label].filter((c): c is string => !!c);
        return { metrics: [], dimensions: [], columns, ...base };
      }
      return {
        metrics: cfg.metric ? [cfg.metric] : [],
        dimensions: cfg.region ? [cfg.region] : [],
        // A map draws regions, not a sequence, so it derives no order of its own — but it still
        // carries one an author asked for, which decides which regions survive `row_limit`.
        columns: [], ...base,
      };
    },
    controls: [
      { id: "mapKey", section: "data", cardOverridable: true, render: MapTypeControl },
      { id: "mapType", section: "data", render: MapTypeSelect },
      { id: "map", section: "data", render: MapControls },
      { id: "pin", section: "data", render: PinControls },
      { id: "color", section: "display", render: MapColorControl },
    ],
  };
}
