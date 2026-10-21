import { useEffect, useReducer, useState } from "react";
import { registerMap } from "echarts/core";
import type { AnyChartParams, MapConfig } from "@/api/charts";
import { vizConfig, shellSettings } from "@/components/chart/viz/renderConfig";
import { formatValue } from "@/components/chart/format";
import { EChartsRenderer } from "@/components/chart/echarts/EChartsRenderer";
import {
  buildMapOption, pinCoverage, pinNote, pinProblem, regionCoverage, regionNote, type MapBuildConfig,
} from "@/components/chart/echarts/options/map";
import {
  MAPS, indexRegionExtent, indexRegionNames, regionExtent, regionKeys, resolveRegion,
} from "@/components/chart/echarts/maps/registry";
import type { ResultColumn } from "@/components/chart/ChartTable";
import type { DrillContext } from "@/components/chart/drill";

const registered = new Set<string>();

/** Choropleth map. Lazy-loads + registers the selected geojson, then renders via EChartsRenderer. */
export function MapChart({ columns, rows, params, metricTypes = {}, onValueClick, onFinished }: {
  columns: ResultColumn[]; rows: unknown[][]; params: AnyChartParams; metricTypes?: Record<string, string>;
  onValueClick?: (ctx: DrillContext, x: number, y: number) => void;
  /** See `EChartsRenderer`'s own doc comment. Forwarded to it on the happy path below; fired
   *  directly from the early returns that never reach ECharts at all (an error, an unregistered
   *  region, an empty result, an unplottable pin result — see `problem` below) — every one of
   *  those IS a finished draw, just not one ECharts drew. The "still loading the geojson" state
   *  deliberately does NOT fire it: a print page waiting on this card has more drawing coming and
   *  must keep waiting for it. */
  onFinished?: () => void;
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
        // And the bounding box, for the same reason and at the same moment: the pin map's "is this
        // coordinate anywhere near this map" test reads it on the very next render. Both derive
        // from the geojson already in hand, so neither costs a byte over the wire.
        indexRegionExtent(def.key, geojson);
        registerMap(def.echartsName, geojson as never);
        registered.add(def.echartsName);
        forceUpdate();
      })
      .catch(() => alive && setError(true));
    return () => {
      alive = false;
    };
  }, [def]);

  const cfg: MapBuildConfig = {
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
    // Undefined until the geojson has been indexed, which is a state this render CAN be in on a
    // cached map (including every render before `ready`); every reader treats undefined as "no
    // opinion" rather than "outside" — and `pinProblem` below is one such reader: `bucketOf`
    // (echarts/options/map.ts) only uses `extent` to split an already-drawable pin between
    // "plotted" and "offMap", both of which count toward `drawnPins`, so whether a pin is
    // drawable AT ALL — the question `pinProblem` answers — never depends on it being indexed yet.
    extent: isPin ? regionExtent(def.key) : undefined,
    // Undefined until indexed, like `extent`: before then the builder has no opinion on which rows
    // land, and `regionNote` stays silent rather than counting every row as unmatched.
    regions: isPin ? undefined : regionKeys(def.key),
    tooltipColumns: map.tooltipColumns,
    // The extra columns print through the dataset's own column formats — the same `formatValue`
    // the table and every other tooltip use, keyed on the RESULT column's type rather than a
    // metric type, because a raw-records query returns columns and no metrics at all.
    formatColumn: (col, v) =>
      formatValue(v, columns.find((c) => c.name === col)?.type ?? "varchar", columnSettings[col]),
    // The shell's own per-column title, which every other chart's axes and tooltips already honour
    // — not a second naming mechanism. Absent falls back to the machine name inside the builder.
    columnLabels: Object.fromEntries(
      Object.entries(columnSettings)
        .map(([col, s]) => [col, s.column_title ?? ""])
        .filter(([, title]) => title !== ""),
    ),
  };
  /**
   * WHAT THE CARD CAN AND CANNOT SHOW, decided BEFORE the chart is built — Metabase's
   * `checkRenderable`, and the same seam `ChartRenderer` puts `sankeyProblem` on.
   *
   * Computed here, ABOVE the `error`/`!ready`/`rows.length === 0` early returns below (moved up
   * from where this used to sit, after them) so the `useEffect` right after this block can fire
   * `onFinished` for `problem` too — a fourth terminal, non-drawing state that a fixed-wait-free
   * print page must not wait on forever. Safe to hoist: as the `extent` comment above establishes,
   * `problem`'s truthiness cannot change once the geojson finishes loading, so computing it before
   * `ready` settles is not computing it with different information, only earlier.
   *
   * TWO PRESENTATIONS BECAUSE THERE ARE TWO SITUATIONS, and neither is a new one for this file.
   * A card that cannot draw anything is REPLACED by a sentence — which is exactly what the three
   * early returns below already do for "Couldn't load the map", "Loading map…" and "No data", so
   * `pinProblem` joins that list rather than inventing a presentation. A card that draws but is
   * not showing every row keeps its pins and gains a line underneath, because replacing it would
   * throw away the pins that are right in order to complain about the ones that are not.
   * `pinProblem` and `pinNote` are mutually exclusive by construction, so the card can never say
   * both.
   */
  const coverage = isPin ? pinCoverage(columns, rows, cfg) : undefined;
  const problem = coverage && pinProblem(coverage, def.label);
  // The region map's half of the same accounting: rows naming no region are not drawn, and the
  // line under the card says how many and which (KI-046). Never on a pin map, never before the
  // geojson is indexed (`regions` undefined), so the two notes cannot both appear.
  const note = coverage
    ? pinNote(coverage, def.label)
    : cfg.regions && regionNote(regionCoverage(columns, rows, cfg), def.label);

  // The FOUR terminal non-drawing states — "no map", "no data" and now "nothing plottable" — count
  // as finished for the same reason a chart's own empty state does: nothing further is coming.
  // "Loading map…" deliberately does NOT fire it, and `problem` is gated on `ready` too even
  // though its VALUE is already final before then (see the comment above `extent` in `cfg`): what
  // matters for `onFinished` is not when the answer is knowable but when it matches what is
  // actually PAINTED, and the screen still reads "Loading map…" until `ready` flips. Firing early
  // here would tell a print page the card is done while a loading message is what gets captured.
  useEffect(() => {
    if (error || (ready && (rows.length === 0 || problem))) onFinished?.();
  }, [error, ready, rows.length, problem, onFinished]);

  if (error) return <div className="flex h-full items-center justify-center text-sm text-destructive">Couldn't load the map</div>;
  if (!ready) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">Loading map…</div>;
  if (rows.length === 0) return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>;
  if (problem) {
    return (
      <div className="flex h-full items-center justify-center p-6 text-center text-sm text-muted-foreground">
        {problem}
      </div>
    );
  }

  return (
    // `flex-col` + `min-h-0 flex-1` rather than the plain `h-full` this used to be: the note is a
    // sibling of the chart, and a chart sized `h-full` beside it would total more than the card.
    // `min-h-0` is load-bearing — a flex item's default `min-height: auto` refuses to shrink below
    // its content, which is how a chart in a flex column overflows instead of fitting.
    <div className="flex h-full w-full flex-col">
      <div className="min-h-0 w-full flex-1">
      <EChartsRenderer
        option={buildMapOption(columns, rows, cfg)}
        onFinished={onFinished}
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
      {/* Plain text, not an alert: nothing is broken, the card is simply accounting for its own
          rows. `shrink-0` so it keeps its line when the card is short — the chart gives way, not
          the sentence explaining it. */}
      {note && <p className="shrink-0 px-2 pt-1 text-center text-xs text-muted-foreground">{note}</p>}
    </div>
  );
}
