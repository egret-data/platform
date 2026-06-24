import { useEffect, useState } from "react";
import { registerMap } from "echarts/core";
import type { ChartParams } from "@/api/charts";
import { EChartsRenderer } from "@/components/chart/echarts/EChartsRenderer";
import { buildMapOption } from "@/components/chart/echarts/options/map";
import { MAPS } from "@/components/chart/echarts/maps/registry";
import type { ResultColumn } from "@/components/chart/ChartTable";

const registered = new Set<string>();

/** Choropleth map. Lazy-loads + registers the selected geojson, then renders via EChartsRenderer. */
export function MapChart({ columns, rows, params }: { columns: ResultColumn[]; rows: unknown[][]; params: ChartParams }) {
  const def = MAPS[params.map?.mapKey ?? "us_states"] ?? MAPS.us_states;
  const [ready, setReady] = useState(registered.has(def.echartsName));
  const [error, setError] = useState(false);

  useEffect(() => {
    let alive = true;
    if (registered.has(def.echartsName)) {
      setReady(true);
      return;
    }
    setReady(false);
    setError(false);
    def
      .load()
      .then((geojson) => {
        if (!alive) return;
        registerMap(def.echartsName, geojson as never);
        registered.add(def.echartsName);
        setReady(true);
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
          region: params.map?.region,
          metric: params.map?.metric,
          normalize: def.normalize,
        })}
      />
    </div>
  );
}
