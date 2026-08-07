import { useEffect, useReducer, useState } from "react";
import { registerMap } from "echarts/core";
import type { AnyChartParams, MapConfig } from "@/api/charts";
import { vizConfig, shellSettings } from "@/components/chart/viz/renderConfig";
import { formatValue } from "@/components/chart/format";
import { EChartsRenderer } from "@/components/chart/echarts/EChartsRenderer";
import { buildMapOption } from "@/components/chart/echarts/options/map";
import { MAPS, indexRegionNames, resolveRegion } from "@/components/chart/echarts/maps/registry";
import type { ResultColumn } from "@/components/chart/ChartTable";
import type { DrillContext } from "@/components/chart/drill";

const registered = new Set<string>();

/** Choropleth map. Lazy-loads + registers the selected geojson, then renders via EChartsRenderer. */
export function MapChart({ columns, rows, params, metricTypes = {}, onValueClick }: {
  columns: ResultColumn[]; rows: unknown[][]; params: AnyChartParams; metricTypes?: Record<string, string>;
  onValueClick?: (ctx: DrillContext, x: number, y: number) => void;
}) {
  const map = vizConfig<MapConfig>("map", params);
  const isPin = map.type === "pin";
  const { columnSettings } = shellSettings(params);
  const fmt = (v: unknown) =>
    map.metric ? formatValue(v, metricTypes[map.metric] ?? "double", columnSettings[map.metric]) : String(v);
  const def = MAPS[map.mapKey ?? "us_states"] ?? MAPS.us_states;
  const normalizeKey = (v: unknown) => resolveRegion(def, v);
  const [, forceUpdate] = useReducer((n: number) => n + 1, 0);
  const [error, setError] = useState(false);
  // Derive readiness from the registration set every render, so switching to a not-yet-registered
  // map never renders its option before registerMap() has run.
  const ready = registered.has(def.echartsName);

  useEffect(() => {
    setError(false);
    if (registered.has(def.echartsName)) return;
    let alive = true;
    def
      .load()
      .then((geojson) => {
        if (!alive) return;
        // Index the names BEFORE registering: the render that follows resolves every region cell
        // through this, and an unindexed map silently matches nothing.
        indexRegionNames(def.key, geojson);
        registerMap(def.echartsName, geojson as never);
        registered.add(def.echartsName);
        forceUpdate();
      })
      .catch(() => alive && setError(true));
    return () => {
      alive = false;
    };
  }, [def]);

  if (error) return <div className="flex h-full items-center justify-center text-sm text-destructive">Couldn't load the map</div>;
  if (!ready) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Loading map…</div>;
  if (rows.length === 0) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>;

  return (
    <div className="h-full w-full">
      <EChartsRenderer
        option={buildMapOption(columns, rows, {
          echartsName: def.echartsName,
          type: map.type,
          region: map.region,
          metric: map.metric,
          normalize: normalizeKey,
          format: fmt,
          // BOTH MODES, and this was briefly written the other way before being corrected.
          // Suppressing them on a pin map is defensible on presentation grounds — two labelled
          // dots among the reader's own pins can read as data — and it is not ours to decide:
          // Hoàng Sa and Trường Sa are on egret's Vietnam map because the map is not correct
          // without them, and "the reader chose pin mode" is not a reason for a Vietnam basemap to
          // lose two archipelagos. They stay, drawn by their own series and `silent`.
          annotations: def.annotations,
          color: map.color,
          latitude: map.latitude,
          longitude: map.longitude,
          label: map.label,
        })}
        // Metabase's region map drills; egret's branch passed no handler — the same seam gap the
        // waterfall, box plot and sankey each had. A region is a row, so it filters by the RAW
        // dimension value, not by the normalised key the geojson is matched on: those differ
        // (`us_states` matches on a two-letter code while the column may hold "California"), and
        // filtering by the key would drill to nothing.
        onPointClick={onValueClick ? (p) => {
          if (isPin) {
            /**
             * A PIN DRILLS BY ITS OWN ROW'S RAW CELLS, found by the row index the datum carries.
             *
             * NOT by matching the clicked coordinates back to a row: that is a float-equality test
             * against numbers that have been through the option, the renderer and a click event,
             * and two pins in one city can round to the same pair anyway. `__row` is put on the
             * datum by `buildPinOption` precisely so this lookup is an index rather than a search.
             *
             * The filter names the coordinate COLUMNS because they are what identifies the record
             * — the label is optional and need not be unique (two orders from one city share it).
             */
            const row = rows[(p.data as { __row?: number } | undefined)?.__row ?? -1];
            if (!row) return;
            const dims = [map.latitude, map.longitude]
              .filter((c): c is string => !!c)
              .map((col) => ({ col, value: String(row[columns.findIndex((c) => c.name === col)]) }));
            if (dims.length) onValueClick({ dims }, p.x, p.y);
            return;
          }
          if (!map.region) return;
          const ri = columns.findIndex((c) => c.name === map.region);
          if (ri < 0 || p.name == null) return;
          const raw = rows.find((r) => normalizeKey(r[ri]) === p.name)?.[ri];
          if (raw == null) return;
          onValueClick({ dims: [{ col: map.region as string, value: String(raw) }] }, p.x, p.y);
        } : undefined}
      />
    </div>
  );
}
