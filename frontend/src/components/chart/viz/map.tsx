import { CHART_TYPES } from "@/components/chart/chartTypes";
import { MAPS } from "@/components/chart/echarts/maps/registry";
import { Label } from "@/components/ui/label";
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from "@/components/ui/select";
import { FieldZone } from "@/components/chart/config/FieldZone";
import { isParamsV2 } from "@/api/charts";
import type { MapConfig, MapType, QuerySpec } from "@/api/charts";
import type { SharedQueryState, VizControlProps, VizDescriptor } from "./types";
import { SERIES_PALETTE } from "@/components/chart/echarts/palette";

import { FIELD_LABEL as LABEL_CLASS } from "@/components/chart/settings/FieldLabel";

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
        <SelectTrigger aria-label="Map" className="h-8 w-full text-sm"><SelectValue /></SelectTrigger>
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
function MapTypeSelect({ config, onChange, columns }: VizControlProps<MapConfig>) {
  return (
    <div className="space-y-1">
      <Label className={LABEL_CLASS}>Map type</Label>
      <Select
        value={config.type ?? "region"}
        onValueChange={(v) => onChange(
          v === "pin" ? { type: "pin", ...defaultCoordinates(config, columns) } : { type: v as MapType },
        )}
      >
        <SelectTrigger aria-label="Map type" className="h-8 w-full text-sm"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="region">Region map</SelectItem>
          <SelectItem value="pin">Pin map</SelectItem>
        </SelectContent>
      </Select>
    </div>
  );
}

/**
 * THE PICKERS' DEFAULTS, from the dataset's own declared semantic types — Metabase's
 * `getDefaultDimensionAndMetric`-style default for `map.latitude_column`, which it derives from a
 * column whose `semantic_type` is `type/Latitude`.
 *
 * THIS IS NOT THE GUESS #434 REJECTED, and the distinction is the whole point. #434 refused to
 * infer a coordinate column from its NAME, because "lat" in a column name is a coincidence a
 * heuristic cannot tell from a fact, and a wrong guess plots confidently in the wrong hemisphere.
 * A semantic type is not a guess about the data: it is a statement someone made about the data, in
 * the dataset's own Columns tab, exactly as they would state that a column is a currency or a URL.
 * If it is wrong, a person wrote it down wrongly, and the Columns tab is where they fix it.
 *
 * WRITTEN ON THE USER'S ACTION, NOT IN AN EFFECT. It would be tidier to resolve these at render
 * time so that any route into pin mode gets them — but a `useEffect` that patches config on mount
 * is precisely how #421's phantom unsaved-changes prompt happened: a chart nobody edited comes up
 * dirty, and the user is asked to save something they did not do. Switching the Map type IS an
 * edit the user just made, so a default written there is part of that edit and dirties nothing
 * extra. That also makes it visible: the two pickers below fill in as the select changes, rather
 * than the chart quietly querying columns the panel never showed.
 *
 * ONLY WHEN BOTH ARE UNSET, so it can never overwrite a choice. A half-configured chart is left
 * alone too: filling only the missing half would pair a declared column with a chosen one behind
 * the user's back, which is the confident-wrong-hemisphere failure by another route.
 */
function defaultCoordinates(config: MapConfig, columns: VizControlProps<MapConfig>["columns"]) {
  if (config.latitude || config.longitude) return {};
  // Numeric as well as declared: a column typed as text cannot be a coordinate whatever it says
  // about itself, and `PinControls` would not offer it, so defaulting to it would produce a picker
  // holding a value that is not in its own list.
  const declared = (role: string) =>
    columns.find((c) => c.semantic_type === role && c.type_generic === "numeric")?.column_name;
  const latitude = declared("latitude");
  const longitude = declared("longitude");
  // BOTH OR NEITHER, for the same reason as above: one coordinate is not a location.
  return latitude && longitude ? { latitude, longitude } : {};
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
          <SelectTrigger aria-label="Region" className="h-8 w-full text-sm"><SelectValue placeholder="Pick a region column…" /></SelectTrigger>
          <SelectContent>
            {columns.map((c) => <SelectItem key={c.column_name} value={c.column_name}>{c.column_name}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
      <div className="space-y-1">
        <Label className={LABEL_CLASS}>Metric</Label>
        <Select value={config.metric ?? ""} onValueChange={(v) => onChange({ metric: v || undefined })}>
          <SelectTrigger aria-label="Metric" className="h-8 w-full text-sm"><SelectValue placeholder="Pick a metric…" /></SelectTrigger>
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
 * THESE FIELDS NOW DEFAULT, and the note that used to stand here was wrong about why they could
 * not. It said egret's `semantic_type` "is a backend enum plus the dataset column editor — a
 * different change from this one". It is not an enum: `migrations/0098_dataset_semantic_types_
 * certification.sql` adds `semantic_type text NOT NULL DEFAULT ''` with no CHECK constraint, the
 * Rust side types it `String` on both the row struct and both DTOs, `ds_update_column` performs no
 * allow-list validation, and the OpenAPI schema types it as a bare string with no `enum`. The
 * vocabulary lived entirely in one `<select>` in `ColumnsTab`. Adding latitude and longitude to it
 * therefore needed no migration, no Rust change and no generated-type change — and `DatasetColumn`
 * already carried `semantic_type` all the way into `VizControlProps.columns`, so this file could
 * have read it on the line below the numeric filter at any point since #434. See
 * `defaultCoordinates` for why a declared type is not the name-guess #434 rejected.
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
        <SelectTrigger aria-label={label} className="h-8 w-full text-sm">
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

/**
 * ADDITIONAL TOOLTIP COLUMNS — Metabase's `graph.tooltip_columns`, and the one place in egret where
 * that feature ports as itself rather than as an analogy. See `MapConfig.tooltipColumns` for the
 * argument; the short form is that #433's objection was about aggregation and a pin map has none.
 *
 * ON THE DATA TAB, NOT DISPLAY, and with no `cardOverridable`, because unlike Metabase's this one
 * CHANGES THE QUERY: egret derives the SELECT list from the viz config, so a column that is not
 * asked for is not fetched and there is no "already in the result" set to choose from. #433's
 * control sits in exactly the same place for exactly the same reason. A dashboard card's spec is
 * never overridden, so letting a card add one would be a control that saves and does not paint.
 *
 * WHAT IS ON OFFER is every dataset column except the three the pin map already selects. Latitude
 * and longitude are excluded because the tooltip prints the coordinate on its own second line
 * regardless, and the label because it is the tooltip's heading — offering any of them would let a
 * reader add a duplicate of something already on screen.
 *
 * HIDDEN ONLY WHEN THERE IS NOTHING TO OFFER **AND** NOTHING STORED — #433's rule, and it carries
 * the same caveat that makes it safe: egret does not filter a stored setting at render time (a
 * chart's settings are stored JSON, and quietly rewriting one is how a config loses things without
 * saying so), so the second clause is what stops a hidden control from stranding a value that
 * still paints.
 */
function PinTooltipColumns({ config, onChange, columns }: VizControlProps<MapConfig>) {
  if (config.type !== "pin") return null;
  const used = new Set([config.latitude, config.longitude, config.label].filter((c): c is string => !!c));
  const selected = config.tooltipColumns ?? [];
  const options = columns.map((c) => c.column_name).filter((c) => !used.has(c));
  if (options.length === 0 && selected.length === 0) return null;
  const labels: Record<string, string> = {};
  for (const c of columns) {
    const v = c.verbose_name?.trim();
    if (v) labels[c.column_name] = v;
  }
  return (
    <div className="space-y-1">
      <FieldZone
        label="Tooltip columns" selected={selected} options={options} labels={labels}
        onChange={(v) => onChange({ tooltipColumns: v })}
      />
      <p className="text-xs text-muted-foreground">
        Fetched with each row and shown in the pin's tooltip only.
      </p>
    </div>
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
          // Metabase's own key name. Read defensively rather than cast blind: this one is an ARRAY
          // in stored JSON, and every other key here is a scalar, so a params blob that is wrong
          // about it would hand `buildSpec` a non-iterable and take the whole card down.
          tooltipColumns: Array.isArray(p.settings["map.tooltip_columns"])
            ? (p.settings["map.tooltip_columns"] as unknown[]).filter((c): c is string => typeof c === "string")
            : undefined,
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
      if (cfg.tooltipColumns !== undefined) out["map.tooltip_columns"] = cfg.tooltipColumns;
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
        //
        // The tooltip columns are appended LAST and deduped against what is already selected and
        // against themselves. Appending is safe here in a way it would not be on an aggregate
        // query: with no GROUP BY, an extra projection cannot change which rows come back or what
        // any other column holds. A duplicate name would be a duplicate `<col> AS <col>` in the
        // SELECT, which the engine would reject outright.
        const chosen = [cfg.latitude, cfg.longitude, cfg.label].filter((c): c is string => !!c);
        const extra = (cfg.tooltipColumns ?? []).filter(
          (c, i, all) => !chosen.includes(c) && all.indexOf(c) === i,
        );
        return { metrics: [], dimensions: [], columns: [...chosen, ...extra], ...base };
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
      { id: "pinTooltip", section: "data", render: PinTooltipColumns },
      { id: "color", section: "display", render: MapColorControl },
    ],
  };
}
