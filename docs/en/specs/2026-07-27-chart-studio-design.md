# Chart Studio — Design Spec

**Status:** approved design, pending implementation plan
**Date:** 2026-07-27
**Sub-project:** A · Chart Studio (of the larger "chart platform" effort)
**Vietnamese reference:** `docs/vi/specs/2026-07-27-chart-studio-design.md`

## Context

The chart platform has five layers: (1) builder shell, (2) config model,
(3) rendering pipeline, (4) cross-cutting concerns, (5) backend. We are tackling
it as three sub-projects, each with its own design → plan → build cycle:

- **A · Chart Studio** — layers 1 + 2 (this spec).
- **B · Render & Consistency** — layers 3 + 4 (later).
- **C · Backend hardening** — layer 5 (later).

Sub-project A comes first because it is the surface users interact with, and the
config-model refactor it contains is coupled to a *visible* improvement rather
than being an invisible rewrite.

## Goal

Redesign the chart builder (`ChartExplore.tsx`) into a **Chart Studio**: a clean,
discoverable 2-pane builder whose per-visualization configuration lives in an
isolated **viz registry**, so `ChartExplore` shrinks from a ~1100-line
god-component to a thin shell, and each chart type can be edited, added, and
tested independently.

## Scope

**In scope**
- Builder shell layout & UX: viz switcher, Data/Display panel, toolbar, preview.
- Run behaviour: hybrid auto-run with an **Auto** toggle.
- Preview states: empty / loading / error.
- Config-model extraction: a viz registry; `ChartExplore` becomes a thin shell.

**Out of scope** (deferred to B/C)
- Standardising the ECharts render options (tooltip/legend/axis/colour) — B.
- Accessibility of the rendered canvas, number-format engine, drill/export — B.
  (The studio *uses* today's renderers as-is; it does not change them.)
- Backend params validation and query pipeline — C.
- Adding new chart types.

## Design decisions (chosen during brainstorming)

1. **Layout: Refined 2-pane.** Keep the current structure (always-visible left
   config panel + preview), polished — not the Metabase viz-first drawer, not
   the Superset 3-pane. Rationale: config always visible = most discoverable for
   internal users with many chart types; smallest, safest change.
2. **Run: Hybrid with an Auto toggle.** Default on: the chart re-runs
   automatically (debounced ~500ms) when a *query-relevant* input changes
   (dataset, dimensions, metrics, filters, temporal units, row limit). Turning
   **Auto** off reverts to click-to-Run for heavy queries (Superset-style).
   Display-only changes (sort, colours, labels, stacking…) already re-render
   from cached data without a query and continue to do so.
3. **Config model: a viz registry.** Each visualization is one module that
   declares everything the shell needs; the shell owns no per-viz knowledge.

## Architecture

### Viz registry (layer 2)

A single registry maps each `VizType` to a descriptor:

```ts
interface VizDescriptor<Cfg> {
  key: VizType;
  meta: ChartTypeMeta;                 // name, icon, description (from chartTypes.ts)
  defaultConfig: Cfg;                  // initial config for a new chart
  DataControls: FC<VizControlProps<Cfg>>;    // left-panel Data tab
  DisplayControls: FC<VizControlProps<Cfg>>; // left-panel Display tab
  requiredFields(cfg: Cfg): string[];  // human labels of missing required fields ("X-axis", "a metric")
  buildSpec(cfg: Cfg, shared: SharedQueryState): QuerySpec;  // → the query
  toParams(cfg: Cfg, ...): ChartParams;      // persist
  fromParams(params: ChartParams): Cfg;      // hydrate
}
```

`VizControlProps<Cfg>` gives a descriptor its `config`, an `onChange(patch)`, and
the resolved dataset `columns`/`metrics` plus the shared temporal-unit renderer.
`SharedQueryState` holds cross-viz query bits (filters, row limit, temporal units).

Registry entries live under `components/chart/viz/<key>.tsx` (or grouped, e.g.
`viz/cartesian.tsx` for the six cartesian kinds that share controls). The
existing option builders and renderers in `components/chart/echarts` /
`ChartRenderer` are unchanged — the registry only owns *configuration*, not
rendering.

### Studio shell (layer 1)

`ChartExplore` becomes a thin orchestrator composed of small units:

- **VizSwitcher** — the chip that opens the type gallery.
- **StudioPanel** — dataset picker + Data/Display tabs; renders the active
  descriptor's `DataControls` / `DisplayControls`. No per-viz `if`-ladders.
- **StudioToolbar** — inline chart name, **Auto** toggle, Run, Save, and a `⋯`
  overflow (Export / View as table / View SQL).
- **PreviewPane** — renders the chart via the existing `ChartRenderer`, or one of
  the three states below; hosts the "view as table" fallback.
- **useChartRun** — the hybrid run hook (see Data flow).

The shell holds only shell-level state: `datasetId`, active `viz`, `saveName`,
`autoRun`, the shared query state, and the active descriptor's `config` (a single
opaque object, not 20 hooks). Switching viz swaps `config` to the new
descriptor's `defaultConfig`.

### Preview states

A reusable `ChartPreviewStates` module (empty / loading / error):

- **Empty** (not yet run): the viz icon + "Set up your \<Viz\>" + the descriptor's
  `requiredFields(cfg)` rendered as "Add: X-axis, a metric".
- **Loading**: a bar-chart-shaped skeleton.
- **Error**: icon + message + **Retry**.

### Data flow

```
config change ──▶ is it query-relevant? ──yes──▶ (Auto on?) ──▶ debounce 500ms ──▶ run()
                                        └─no──▶ re-render from cached rows (no query)
                (Auto off) ──▶ mark "needs run" ──▶ user clicks Run ──▶ run()
run(): buildSpec(config, shared) ──▶ useSpecData ──▶ rows ──▶ ChartRenderer
open saved chart: fromParams(params) ──▶ config + spec ──▶ auto-runs on load
```

Opening a saved chart hydrates its spec and renders immediately (no manual Run).

## Components & isolation

| Unit | Purpose | Depends on |
|---|---|---|
| `viz/<key>` descriptor | one viz's config, controls, spec/param mapping | dataset columns/metrics, config types |
| `vizRegistry` | look up descriptor by `VizType` | descriptors |
| `VizSwitcher` | change viz type | registry meta, gallery |
| `StudioPanel` | dataset + Data/Display for the active viz | active descriptor |
| `StudioToolbar` | name, Auto, Run, Save, overflow | shell state |
| `PreviewPane` + `ChartPreviewStates` | render chart or a state | ChartRenderer (unchanged) |
| `useChartRun` | hybrid auto/manual run | useSpecData |

Each descriptor answers: *what does this viz need, how do you configure it, how
does it become a query/params?* — without the shell reading its internals.

## Migration strategy

Incremental and always-shippable:

1. Introduce the registry + shell scaffolding; move **one** viz family (cartesian)
   into a descriptor; the shell drives it; everything else stays inline behind a
   temporary bridge.
2. Move the remaining vizzes into descriptors a few at a time, deleting their
   inline branches from `ChartExplore` as they go.
3. Land the toolbar/Auto-toggle/states polish alongside step 1 so each slice is
   visibly better.

Each slice is a small PR, deployed and reviewable, with the demo dataset + sample
charts kept in place for review.

## Testing

- **Descriptors**: unit-test `requiredFields`, `buildSpec`, `toParams`/`fromParams`
  per viz (pure functions).
- **Shell**: test that a query-relevant change auto-runs when Auto is on and does
  not when off; that display-only changes never fire a query; that a saved chart
  auto-runs on open.
- **States**: empty shows the right missing-fields; loading/error render.
- **A11y**: toolbar/controls have labels; keep existing coverage green.

## Success criteria

- `ChartExplore.tsx` no longer contains per-viz `if`-ladders; adding or editing a
  chart type touches only its descriptor.
- Building a chart feels live (Auto on) but stays controllable (Auto off).
- Empty/loading/error are always meaningful; a saved chart renders on open.
- No regression in existing chart behaviour (rendering unchanged in this
  sub-project).

## Risks

- Registry extraction touches a large, central component — mitigated by the
  incremental, always-shippable migration and per-slice live verification.
- Auto-run could fire heavy queries — mitigated by debounce + the Auto toggle +
  only firing on query-relevant changes.
