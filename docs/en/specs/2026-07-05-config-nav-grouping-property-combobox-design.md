# Config nav grouping + property-key combobox (2026-07-05)

VI reference: `docs/vi/specs/2026-07-05-config-nav-grouping-property-combobox-design.md`.

**Goal:** Two UX refinements on the shipped config UI, from user feedback: (1) the
Configuration vertical nav has 14 flat items and feels cluttered — organize them into a
few collapsible groups; (2) the raw `config.properties` "Add property" key field is
free-text — make it a combobox that suggests known Trino property names (typeahead) while
still allowing a custom key. **Frontend only — no backend/payload change.**

**Non-goals:** new config fields; changing what any field does; security/observability
(#3/#4). Reuses the #1 UX framework primitives.

## 1. Grouped, collapsible Configuration nav

Replace the flat 14-item vertical nav (in `EngineDetail.tsx`'s `ConfigurationTab`) with a
grouped list — 5 collapsible groups, each a header (chevron, click to toggle) with its
section items nested:
- **Basics**: General
- **Roles**: Coordinator, Worker
- **Workload & Tuning**: Workload, Reliability, Query & Memory, Spilling, Execution & Optimizer, SQL & Web UI
- **Pods & Scheduling**: Pod, Placement, Containers & Mounts, Autoscaling
- **Advanced**: Advanced

Behavior: a `groups` array of `{ label, sections: SectionKey[] }` drives the render; per-
group open/closed state in a `Record<string,boolean>`; on load, expand the group that
contains the active `section` (and Basics), collapse the rest. Selecting a section still
sets the active `section`. Non-default badges stay on the section items; a collapsed
group header shows a small summed badge (sum of its sections' counts) so set config isn't
hidden. Reuse the existing `Collapsible` primitive. No section is removed — this only
organizes them.

## 2. Property-key combobox in the raw config.properties editor

- **`config/trinoPropertyNames.ts`** — a curated broad catalog
  `export const trinoPropertyNames: { key: string; help?: string }[]` of ~100+ common
  Trino `config.properties` names (from the docs), a superset of the 31 guided keys.
- **`KeyValueEditor`** (in `EngineResourceGroups.tsx`) gains an optional
  `keySuggestions?: { key: string; help?: string }[]` prop. When provided, the key
  `<Input>` becomes a small combobox: as the user types, a dropdown lists up to ~8
  matching suggestions (key + one-line help); clicking one fills the key; free text
  (a key not in the list) is still accepted on Enter/blur. When the prop is absent the
  editor behaves exactly as today (env, annotations, etc. are unaffected).
- The config.properties editor in `EngineConfigGeneral.tsx` passes
  `keySuggestions = trinoPropertyNames.filter(p => !knownPropertyKeys.has(p.key))` — it
  suggests the LONG-TAIL properties (the 31 guided ones live in their sections). Its
  `onChange` also DROPS any row whose key is in `knownPropertyKeys`
  (`newRows.filter(kv => !knownPropertyKeys.has(kv.key))`) so a guided key typed into the
  raw editor never persists as a duplicate/ghost row (fixes the observed
  "query.max-run-time × 3" confusion — that key is guided in Query & Memory).

## Test
- Unit: `keySuggestions` filtering (guided keys excluded from raw suggestions); the raw
  config.properties `onChange` drops known keys; the nav `groups` mapping covers all 14
  sections exactly once.
- **Playwright**: Configuration shows ~5 group headers; collapsing/expanding a group
  hides/shows its items; a collapsed group with set config shows a summed badge; on the
  General config.properties editor, "Add property" → typing "spill" (a long-tail key like
  `spiller-threads`, since `spill-enabled` is guided/excluded) suggests matches and picking
  one fills the key; a truly custom key still types through; a guided key (e.g.
  `query.max-run-time`) is not suggested and, if force-typed, does not persist as a raw row.
  0 console errors. Save still issues the `updateEngine` PUT with an unchanged payload.

## Compat
- Frontend-only; no migration/API/payload change. The nav grouping is purely visual; the
  combobox is additive (KeyValueEditor unchanged when `keySuggestions` is absent).
