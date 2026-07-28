# Chart Platform B — Render & Consistency — Design Spec

**Status:** approved design, pending implementation plan
**Date:** 2026-07-28
**Sub-project:** B · Render & Consistency (layers 3 + 4 of the chart platform)
**Parent spec:** `docs/en/specs/2026-07-27-chart-studio-design.md`
**Vietnamese reference:** `docs/vi/specs/2026-07-28-chart-render-consistency-design.md`

## Context

A (Chart Studio) is finished: A1 gave the studio surface, A2 the viz registry and
params v2 (`{version, spec, settings}` with flat namespaced settings). To keep A2
from touching rendering, it left a deliberate seam — `toRenderParams()` converts
v2 back to the pre-A2 nested shape so every renderer, and the dashboard's per-card
`vizOverride`, could stay untouched.

That seam is now the main thing standing between the codebase and one shape. And
the render layer itself is inconsistent in a way A2 could not address:

| builder | uses `CHART_THEME` | formats numbers |
|---|---|---|
| cartesian | yes (23 references) | yes |
| waterfall, boxplot | yes (8, 6) | yes |
| pie, funnel, gauge, sankey | **no** | yes |
| map | **no** | **no** |

So five of the eight option builders pick their own fonts, label sizes and grid,
and the map renders raw numbers with no column formatting at all. Tooltips and
legends are declared ad hoc in each file.

## Goal

One shape end to end (v2 everywhere, no adapter), and one visual language across
every chart type: the same theme, tooltip, legend, axis and number formatting,
composed rather than repeated.

## Scope

Two slices, in this order — the debt first, because while renderers still read v1
through the adapter, every render change has to be made twice.

**B1 · Retire the v1 shape**
- `ChartRenderer` and the option builders take a viz's config from its descriptor
  instead of the nested v1 keys.
- `vizOverride` becomes a partial *settings* map; migration `0100` rewrites the
  stored overrides in `analytics.dashboard_cards.options`.
- `CardVizPanel` composes the same units the builder does; `ChartSettingsSections`
  and `toRenderParams` are deleted.

**B2 · One visual language**
- A composed base option: theme, tooltip, legend, axis, aria — every builder
  starts from it instead of restating it.
- Number formatting applied uniformly (tooltip, axis, data label), including the
  map, through one formatter.
- **The dashboard card editor offers no number formatting at all** — `DashboardView`
  hands it `[...spec.dimensions, ...spec.columns]`, so a cartesian chart's metrics
  are never in the list and the Formatting section has nothing to show. Found while
  verifying B1; it predates A2 and belongs with the formatting work.
- Canvas accessibility audited per viz: `ARIA_A11Y` is already applied everywhere,
  but the generated descriptions and decal patterns are unverified.

**Out of scope**
- Backend params validation and the query pipeline — sub-project C.
- New chart types; changing what any chart *means*.
- Redesigning the studio UX — that was A.

## B1 design

### Renderers read descriptors

`ChartRenderer` currently destructures `params.cartesian`, `params.pie`, … Instead
it resolves the active descriptor and asks it:

```ts
const cfg = getDescriptor(vizType)!.fromParams(params);   // reads v1 and v2 alike
```

Each option builder keeps its current signature (config in, ECharts option out);
only where the config *comes from* changes. Because `fromParams` still reads the
v1 nested shape, a chart that somehow escaped migration `0099` keeps rendering —
the adapter goes away without becoming a hard cut-over.

The shell-owned settings (`column_settings`, `conditional_formatting`, table
state) are read from `params.settings` by a small helper rather than from the v1
top-level keys.

### `vizOverride` becomes a settings partial

Today a per-card override is a v1-shaped partial deep-merged onto `chart.params`.
In v2 it is what it always wanted to be — a flat partial of `settings`:

```ts
applyVizOverride(chart, override) =>
  { viz_type: override.viz_type ?? chart.viz_type,
    params: { ...chart.params, settings: { ...chart.params.settings, ...override.settings } } }
```

Merging by key needs no per-viz knowledge, which is exactly why flat settings were
chosen in A2. Migration `0100` rewrites stored overrides; the reader accepts the
old shape for one release so a dashboard opened mid-deploy cannot break.

### `CardVizPanel` composes what the builder composes

The dashboard's card editor renders the active descriptor's display controls
(`StudioPanel`) plus `SharedDisplaySections` — the same two units the builder's
Display tab uses — and emits a settings partial. `ChartSettingsSections`, whose
only remaining job was to hold those two together for this panel, is deleted.

### Migration

`0100_dashboard_viz_override_v2.sql` rewrites
`analytics.dashboard_cards.options->'vizOverride'`: nested display keys become
flat namespaced ones, `columnSettings` → `column_settings`,
`conditionalFormatting` → `conditional_formatting`, `viz_type` kept as-is. Cards
with no override are untouched.

## Testing

- **Renderer:** each option builder gets its config from a descriptor — existing
  renderer tests keep passing unchanged, which is the point; if one needs editing,
  the config plumbing is wrong.
- **Override:** `applyVizOverride` merges a settings partial; a card with an
  override renders differently from the same chart without one.
- **Round-trip:** a v1 chart still renders (descriptors read both shapes).
- **Migration:** applied on kind, with a stored override diffed before and after.
- **Live:** the builder, a dashboard with an overridden card, and its card editor.

## Success criteria

- `toRenderParams`, `ChartSettingsSections` and the v1 nested reads are gone; the
  only reader of the old shape is each descriptor's `fromParams` fallback.
- A dashboard card with a viz override renders and edits exactly as before.
- No renderer test needed changing to accommodate the new plumbing.

## Risks

- **Stored overrides are user data**, and unlike chart params they were never
  version-stamped. Mitigated by the reader accepting both shapes and by diffing
  the migration on kind before it lands.
- **The card editor is a second consumer of the display units.** A2 shipped a
  regression exactly here (#348) by changing one consumer and not the other —
  both paths get a test this time.
