# Chart Studio A2 — Viz Registry & Params v2 — Design Spec

**Status:** approved design, pending implementation plan
**Date:** 2026-07-28
**Sub-project:** A2 · Viz registry (second half of A · Chart Studio)
**Parent spec:** `docs/en/specs/2026-07-27-chart-studio-design.md`
**Vietnamese reference:** `docs/vi/specs/2026-07-28-chart-studio-a2-viz-registry-design.md`

## Context

A1 (PR #344, follow-up #345) delivered the studio surface: run behaviour with an
Auto toggle, meaningful preview states, and the toolbar. It left layer 2 of the
parent spec untouched — the config model.

`ChartExplore.tsx` is still a 1113-line god-component: 40 `useState` hooks and
the same per-viz `if`-ladder repeated in six places (`specForRun`,
`paramsForSave`, the validation inside `run()`, the Data-tab JSX, the hydration
effect, `resetControls`). Adding or changing a chart type means editing all six.

A2 extracts that into a viz registry and shrinks the shell.

## Goal

Each visualization becomes one self-contained module declaring everything the
shell needs. `ChartExplore` keeps only shell state and reads the registry — no
per-viz knowledge anywhere else. Stored chart params move to a normalized,
flat, namespaced shape (v2).

## Scope

**In scope**

- The `VizDescriptor` interface, the registry, and 14 descriptor modules covering
  all 19 viz types.
- Shell decomposition: `VizSwitcher`, `StudioPanel`, `PreviewPane`, `useChartRun`.
- `ChartParams` v2: flat namespaced settings + separate query spec.
- `toRenderParams()` adapter so `ChartRenderer` and dashboards keep today's shape.
- Migration `0099` backfilling `analytics.charts.params` to v2.

**Out of scope**

- Rendering: `ChartRenderer`, `echarts/options/*`, `PivotTable`, `ChartTable` are
  untouched. Standardising them is sub-project B.
- Backend params validation — sub-project C. The backend keeps treating `params`
  as opaque JSONB.
- New chart types, and any change to how charts look.

## Prior art: what Metabase and Superset actually do

Both were read directly (they are checked out beside this repo) rather than
recalled, because the parent spec's registry sketch had not been checked against
them.

| Concern | Metabase | Superset |
|---|---|---|
| Registry | `Map<display, Visualization>` + `registerVisualization` (`frontend/src/metabase/visualizations/index.ts`) | `ChartPlugin` registry |
| Module per viz | `visualizations/Funnel/` | `plugins/plugin-chart-echarts/src/BoxPlot/` |
| Query building | in the question/MBQL layer | `buildQuery.ts` per plugin |
| Render transform | viz component | `transformProps.ts` per plugin |
| Controls | declarative settings definitions, section is a **property of each setting** (`getSection: () => "Data"` — `Funnel.tsx:86`) | declarative `controlPanel.ts`: `controlPanelSections` labelled Query / Chart Options, composed from `sharedControls` |
| Stored config | `visualization_settings`: **flat, namespaced** (`funnel.dimension`, `graph.dimensions`, `column_settings`), separate from `dataset_query` | `form_data`: **flat** |

Three conclusions drove the decisions below: registry-of-descriptors is the right
core (both agree); the Data/Display split belongs on the setting, not on the
module boundary; and stored settings should be flat and namespaced, not nested
per viz.

## Design decisions

1. **One descriptor per viz family, in a registry.** As the parent spec proposed
   and both references confirm.
2. **Controls are a list, each tagged with its section** — not two fixed
   `DataControls` / `DisplayControls` components. Moving a control between tabs,
   or adding a third tab later, then touches one entry instead of 14 modules.
3. **Params v2 is flat and namespaced** (`cartesian.xAxis`, `pivot.rows`,
   `column_settings`), with the query spec kept separate. This buys what the
   nested shape cannot: switching bar→line keeps the axis and metrics because
   both read `cartesian.*`; the dashboard's per-card `vizOverride` merges by key;
   adding a setting never needs another migration.
4. **Typed in memory, flat on disk.** Each descriptor declares its prefix and a
   codec (`fromSettings` / `toSettings`), so a descriptor still works with a typed
   `Cfg` object; only storage is flat.
5. **Controls stay JSX, deliberately.** Metabase's declarative settings framework
   (widget registry, `getValue` / `readDependencies` dependency graph, `getHidden`)
   is thousands of lines. For 19 internal visualizations the cost outweighs the
   benefit today. The price is accepted knowingly: default logic is not shared with
   the dashboard's card-settings editor, which stays hand-rolled.
6. **The renderer is insulated by an adapter**, so this sub-project cannot regress
   rendering or dashboards (see below).

Not copied, on purpose: Metabase's settings dependency graph (decision 5), and
`isSensible` / `checkRenderable` (suggesting which viz types suit a result set).
The latter is cheap and worth doing later — it is not part of A2.

## Architecture

### Descriptor

```ts
interface VizDescriptor<Cfg> {
  key: VizType;
  meta: ChartTypeMeta;                       // name, icon, description (chartTypes.ts)
  prefix: string;                            // settings namespace, e.g. "cartesian"
  defaultConfig: Cfg;

  fromSettings(settings: Settings, spec: QuerySpec): Cfg;  // reads v2 and v1
  toSettings(cfg: Cfg): Settings;                          // writes v2
  buildSpec(cfg: Cfg, shared: SharedQueryState): QuerySpec;
  requiredFields(cfg: Cfg): string[];        // "X-axis", "a metric" — moved from requiredFields.ts

  controls: Array<{
    id: string;
    section: "data" | "display";
    render: FC<VizControlProps<Cfg>>;
  }>;
}
```

`VizControlProps<Cfg>` carries `config`, `onChange(patch)`, the dataset's
`columns` / `metrics`, and the shared temporal-unit renderer. `SharedQueryState`
holds the cross-viz query bits: filters, row limit, temporal units.

`vizRegistry: Record<VizType, VizDescriptor>` is the only lookup the shell makes.
Modules live in `components/chart/viz/`: `cartesian.tsx` covers the six cartesian
kinds (they share one config and one namespace, as `graph.*` does in Metabase),
and the other thirteen get a file each.

A descriptor reads and writes **only its own namespace**: `toSettings` returns the
`<prefix>.*` keys, and the shell merges them over the settings map it already
holds. The shared keys (`column_settings`, `conditional_formatting`) belong to the
shell, so no descriptor can clobber another's settings when the viz type changes.

Descriptors own configuration only. Rendering stays in `ChartRenderer` and
`echarts/options/*`.

### Shell

`ChartExplore` keeps shell state only: `datasetId`, `viz`, `saveName`, `autoRun`,
`shared` (filters / row limit / temporal units), and one opaque `settings` map.
Switching viz keeps the map and lets the new descriptor's `defaultConfig` fill
the gaps.

Extracted units:

| Unit | Purpose |
|---|---|
| `VizSwitcher` | change viz type (gallery) |
| `StudioPanel` | dataset picker + Data/Display tabs, rendering the descriptor's controls by section |
| `PreviewPane` | chart or one of the A1 preview states |
| `useChartRun` | `run()` + auto-run + the stale-freeze rule from #345 |

`StudioToolbar` and `ChartPreviewStates` already exist from A1.

All six `if`-ladders disappear: `specForRun` → `buildSpec`, `paramsForSave` →
`toSettings`, `run()` validation → `requiredFields`, Data-tab JSX → `controls`,
hydration → `fromSettings`, `resetControls` → `defaultConfig`.

### Params v2

```ts
interface ChartParams {
  version: 2;
  spec: QuerySpec;                    // the query — unchanged shape, still top level
  settings: Record<string, unknown>;  // flat, namespaced
}
```

`spec` stays at the top level deliberately: dashboards inject their parameter
filters into `params.spec`, so `dashboard/params.ts` needs no change.

Namespaces:

| Prefix | Viz types | Example keys |
|---|---|---|
| `cartesian.` | bar, row, line, area, scatter, combo | `cartesian.xAxis`, `cartesian.yMetrics`, `cartesian.series`, `cartesian.stacked`, `cartesian.seriesColors` |
| `pie.` | pie | `pie.dimension`, `pie.metric`, `pie.donut`, `pie.sliceColors` |
| `pivot.` | pivot_table | `pivot.rows`, `pivot.cols`, `pivot.metrics`, `pivot.showRowTotals`, `pivot.collapsedRows`, `pivot.sort` |
| `table.` | table | `table.columns`, `table.metrics`, `table.state` |
| `object.` | object | `object.columns` |
| `map.` | map | `map.key`, `map.region`, `map.metric` |
| `scalar.` `progress.` `gauge.` `funnel.` `waterfall.` `smartscalar.` `boxplot.` `sankey.` | one each | e.g. `gauge.metric`, `gauge.min`, `gauge.max` |
| *(shared, no viz prefix)* | all | `column_settings`, `conditional_formatting` |

Two clean-ups fall out of this. In v1 eight pivot fields sit loose at the top
level (`rows`, `cols`, `metrics`, `showRowTotals`, `condense`, `collapsedRows`,
`pivotSort`, `showColTotals`) — they move under `pivot.`. And `table` / `object`
store no config at all today: their columns are re-derived from `spec` during
hydration (`p.spec?.dimensions` + `p.spec?.columns`), which now becomes explicit.

### Render boundary

```
params v2 ──▶ toRenderParams(viz, params) ──▶ v1 shape ──▶ applyVizOverride(v1 keys) ──▶ ChartRenderer
```

`toRenderParams()` rebuilds today's nested shape. `ChartRenderer`, `PivotTable`,
`ChartTable` and `MapChart` are not touched, and dashboards keep working
unchanged — including the per-card `vizOverride` stored in dashboard JSONB, which
is a v1-shaped partial. Applying the override *after* the adapter means **no
stored override needs migrating**. The adapter and the overrides go together when
sub-project B reworks rendering.

### Migration

Two layers, because either alone leaves a gap:

- **Read side** — every `fromSettings` accepts v1 (no `version` field). This is the
  hydration effect's existing fallback chain (`p.cartesian ?? derive from spec`)
  moved into the descriptors. A chart restored from an old backup still opens.
- **Backfill** — `0099_chart_params_v2.sql` rewrites `analytics.charts.params` per
  `viz_type`, including soft-deleted rows. `analytics.dashboards` is deliberately
  left alone (see render boundary).

## Delivery

Three sequential PRs, each deployed to kind and live-checked, matching the parent
spec's always-shippable migration:

| PR | Contents | Live check |
|---|---|---|
| 1 | v2 types + adapter + registry + shell (`StudioPanel`, `VizSwitcher`, `PreviewPane`, `useChartRun`) + the cartesian family. The other 13 viz types keep their current inline code behind a temporary bridge. | a new cartesian chart, and an existing v1 chart opened |
| 2 | `table`, `pivot_table`, `object`, `map` — the four with the most state | saved pivot + table charts |
| 3 | the nine remaining viz types; delete the bridge, `ChartSettingsSections` and `requiredFields.ts`; migration `0099` | one chart per viz type, plus a dashboard with a card override |

The backfill lands in PR 3 because only then does every descriptor read and write
v2; running it earlier would fight the bridge.

## Testing

- **Per descriptor (pure):** `requiredFields`, `buildSpec`, `toSettings`,
  `fromSettings`.
- **Round-trip:** `v1 params → fromSettings → Cfg → toSettings → v2 →
  toRenderParams → v1`, asserted per viz type. This is the fragile seam, so it is
  tested for every viz, not sampled.
- **Shell:** auto-run fires on a query-relevant change when Auto is on and not when
  off; display-only changes never query; a saved chart runs on open; a stale query
  keeps the previous result (the #345 rule).
- **Dashboard:** a card with a `vizOverride` renders identically before and after.
- Existing chart tests stay green; no rendering test should need editing — if one
  does, the adapter is wrong.

## Success criteria

- `ChartExplore.tsx` contains no per-viz branching; adding a chart type touches
  one descriptor module and the registry.
- Every saved chart, including v1 rows and dashboard cards with overrides, renders
  exactly as before.
- Switching between two viz types in the same family keeps the settings that still
  apply.
- FE CI green locally per PR (lint, typecheck, tests, build) plus a live check on
  kind.

## Risks

- **Silent params corruption** is the real risk, not compile errors — mitigated by
  the per-viz round-trip tests, the two-layer migration, and live checks against
  charts saved before the change.
- **A large refactor of a central component** — mitigated by the three-PR split,
  the bridge that keeps unmigrated viz types on their current code, and one
  live-checked deploy per PR.
- **Flat settings lose per-viz compile-time typing** — mitigated by the codec:
  descriptors work with typed `Cfg` objects and only serialize flat.
