# Map Visualization (Design)

**Status:** Approved approach, pending spec review
**Canonical:** this file. Vietnamese reference: `docs/vi/specs/2026-06-24-map-viz-design.md`.

## Goal

Add a **Map** (choropleth) visualization: colour a region by a metric. Three maps ship in MVP —
**US states**, **World countries**, and **Vietnam provinces** — selectable in the builder. Fills the gap
where the E-commerce clone substitutes a row chart for "Revenue by state".

Reuses the existing ECharts stack (`EChartsRenderer`) — ECharts has a native `map` series — so **no new
runtime dependency**.

## Decisions (from brainstorming)

- **Three maps:** US states, World countries, Vietnam provinces.
- **Choropleth only** (regions shaded by value); no point/bubble maps.
- **Pre-processed, committed geojson assets** keyed by the exact code/name the data uses, so runtime
  region-matching is a trivial string compare and CI needs no network.

## The geojson assets (committed) + a one-time prep script

The data values that identify a region:
- US `state` → 2-letter abbreviations (e.g. `NH`).
- World `country` → ISO-A2 codes (e.g. `NO`).
- Vietnam → province names (no VN data yet; province name is the natural future key).

A committed Python prep script `scripts/build_map_geojson.py` downloads three public sources and
normalizes each feature's `properties` to exactly `{ key, name }`, then rounds coordinates to 2 decimals
to shrink size, writing three committed assets under
`frontend/src/components/chart/echarts/maps/`:

| asset | source | `key` (matches data) | `name` (tooltip label) |
|---|---|---|---|
| `us-states.geo.json` | PublicaMundi us-states.json | state abbreviation (from a 51-entry name→abbr table in the script) | full state name |
| `world-countries.geo.json` | Natural Earth `ne_110m_admin_0_countries` | `ISO_A2_EH` (uppercased; `ISO_A2_EH` fixes Natural Earth's `-99` gaps for France/Norway/etc.) | `NAME` |
| `vietnam-provinces.geo.json` | geoBoundaries `gbOpen VNM ADM1` (via its API `gjDownloadURL`) | `shapeName` (province name) | `shapeName` |

The script is committed for reproducibility; the **generated assets are committed too**, so neither the
app nor CI fetches anything at build/run time. (Re-running the script requires network.)

## Map registry — `frontend/src/components/chart/echarts/maps/registry.ts`

```ts
export interface MapDef {
  key: string;          // registry id, e.g. "us_states"
  label: string;        // builder label, e.g. "US states"
  echartsName: string;  // name passed to echarts.registerMap / series.map
  load: () => Promise<GeoJsonObject>;     // dynamic import of the committed asset
  normalize: (v: unknown) => string;      // data value -> feature `properties.key`
}
export const MAPS: Record<string, MapDef>;   // us_states, world, vietnam
```
- `us_states` / `world`: `normalize = (v) => String(v ?? "").trim().toUpperCase()` (codes).
- `vietnam`: `normalize = (v) => String(v ?? "").trim()` (province name as-is).
- `load` uses a dynamic `import("./us-states.geo.json")` etc., so the geojson is a lazy chunk (never in
  the main bundle).

## `MapChart` component — `frontend/src/components/chart/MapChart.tsx`

Props: `columns: ResultColumn[]`, `rows: unknown[][]`, `params: ChartParams` (reads `params.map`).
- Resolves `const def = MAPS[params.map?.mapKey]` (default `us_states`).
- A `useEffect` keyed on `def.key`: `await def.load()`, then `echarts.registerMap(def.echartsName, geojson, { nameProperty: "key" })` — registered once per map (idempotent guard), then sets `ready`.
- Until ready: a centered "Loading map…". Empty `rows`: "No data".
- When ready, builds the option via `buildMapOption` and renders `<EChartsRenderer option=… />`.
- `h-full w-full` to fill a dashboard card (post-PR-#45 convention).

## `buildMapOption` — `frontend/src/components/chart/echarts/options/map.ts`

`buildMapOption(columns, rows, cfg: { echartsName, region, metric, regionNormalize, columnSettings, metricTypes })`:
- Build `data = rows.map(r => ({ name: regionNormalize(r[regionIdx]), value: Number(r[metricIdx]) }))`, dropping rows whose key is empty or value NaN.
- `visualMap` (continuous): `min`/`max` from the values, an `inRange.color` teal-brand gradient
  (light → `--primary`), positioned bottom-left, `calculable: true`.
- `series: [{ type: "map", map: echartsName, nameProperty: "key", roam: false, emphasis: …, data }]`.
- `tooltip`: `formatter` showing the feature's display name + the formatted metric value (reuse the
  shared metric formatter as the cartesian/pie builders do).
Returns an `EChartsCoreOption`.

## Register the ECharts modules — `frontend/src/components/chart/echarts/register.ts`

Add `MapChart` (from `echarts/charts`) and `VisualMapComponent` (from `echarts/components`) to the `use([…])`
list. (Tree-shaken; only added because the map viz needs them.)

## Render route — `frontend/src/components/chart/ChartRenderer.tsx`

`if (vizType === "map") return <MapChart columns={columns} rows={rows} params={params} />;`
Because dashboard cards and the builder preview both render through `ChartRenderer`, the map works in both.

## Viz registration

- `frontend/src/api/charts.ts`: add `"map"` to `VizType` (not cartesian); add a `MapConfig` shape to
  `ChartParams`: `map?: { mapKey: string; region?: string; metric?: string }`.
- `frontend/src/components/chart/chartTypes.ts`: a `CHART_TYPES` entry
  `{ key: "map", name: "Map", description: "Shade regions of a map by a metric (US states, world, Vietnam).", icon: Map }` (lucide `Map`).

## Builder controls — `frontend/src/routes/explore/ChartExplore.tsx`

A `{viz === "map" && (…)}` control block with:
- **Map** `<Select>` over `Object.values(MAPS)` (label → mapKey), bound to `map.mapKey` (default `us_states`).
- **Region** `<Select>` over the dataset `columns` (the dimension holding the region code/name).
- **Metric** `<Select>` over the dataset `metrics`.
State `const [mapCfg, setMapCfg] = useState<{ mapKey: string; region?: string; metric?: string }>({ mapKey: "us_states" })`.
- `specForRun()` map branch: `{ metrics: mapCfg.metric ? [mapCfg.metric] : [], dimensions: mapCfg.region ? [mapCfg.region] : [], columns: [], filters, orderby: [], row_limit: rowLimit, rollup_groups: [] }`.
- `paramsForSave()` map branch: `{ spec: specForRun(), map: mapCfg }`.
- Hydrate: `else if (c.viz_type === "map") { setMapCfg(p.map ?? { mapKey: "us_states" }); }`.
- `run()` + `save()` guard: `if (viz === "map" && (!mapCfg.region || !mapCfg.metric)) { toast.error("Map needs a region and a metric"); return; }`.
- Preview: object falls through to `ChartRenderer` (the `viz !== "table" && viz !== "pivot_table"` branch), so the map renders in the preview with no preview change.

## Data flow

1. User picks **Map** → a map (US/World/Vietnam) → a region dimension → a metric → Run.
2. `specForRun` sends an aggregate query (metric by region). Backend returns `[region, value]` rows.
3. `ChartRenderer` → `MapChart` lazy-loads + registers the chosen geojson, normalizes each row's region to
   the feature key, and renders a choropleth with a teal `visualMap`.

## Error handling

- Map asset fails to load → MapChart shows "Couldn't load the map" (caught), the per-card error boundary
  (PR #43) already contains any render throw.
- A region value with no matching feature is simply not shaded (ECharts ignores unmatched `data` names);
  no crash.
- Empty rows → "No data".

## Testing (Vitest)

- `map.test.ts`: `buildMapOption` produces `series[0].data` with normalized keys (e.g. `"nh"`→`"NH"` for
  US) and the correct `visualMap.min/max`; a NaN value / empty region is dropped; `map` equals the
  `echartsName`. Registry `normalize` per map (US/World uppercase; VN trims).
- (The geojson load + `registerMap` are exercised by the live verify, not unit tests — jsdom can't render
  ECharts maps.)
- Live verify (Playwright, throwaway chart, 0 console errors): US "Revenue by state" choropleth on
  `ecom_orders`; switch the Map select to **World** with `accounts.country` + a metric; switch to
  **Vietnam** (renders the province outline even without data). Save onto a throwaway dashboard card →
  renders + the visualMap legend shows. Delete the throwaway. Full sweep green.
- Follow-up (separate, after merge): point the seed script's "Revenue by state" card at the real US map.

## Files

- Create: `scripts/build_map_geojson.py`; `frontend/src/components/chart/echarts/maps/registry.ts`;
  `…/maps/us-states.geo.json`, `…/maps/world-countries.geo.json`, `…/maps/vietnam-provinces.geo.json`
  (generated); `frontend/src/components/chart/MapChart.tsx`;
  `frontend/src/components/chart/echarts/options/map.ts`; `…/__tests__/map.test.ts`.
- Modify: `frontend/src/components/chart/echarts/register.ts`; `frontend/src/api/charts.ts`;
  `frontend/src/components/chart/chartTypes.ts`; `frontend/src/components/chart/ChartRenderer.tsx`;
  `frontend/src/routes/explore/ChartExplore.tsx`.

## Non-goals

- No backend change. No point/bubble/heat maps. No map pan/zoom (`roam: false`) in MVP. No per-region
  drill-through. Maps beyond the three shipped are out of scope (the registry makes adding more cheap).

## Conventions

Frontend sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`
(from `frontend/`). Importing `.geo.json` may need `resolveJsonModule` (already on for a Vite TS app) — the
dynamic import returns the parsed object. Configured git identity; `Co-Authored-By: Claude Opus 4.8 (1M
context) <noreply@anthropic.com>` footer; stage explicit paths.
