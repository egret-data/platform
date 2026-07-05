# Properties-first config UI (2026-07-05)

VI reference: `docs/vi/specs/2026-07-05-properties-first-config-design.md`.

**Goal:** Re-center the engine Configuration UI on a single, searchable, key/value
**Properties** list backed by the FULL Trino property catalog — because the user finds the
many bespoke structured-form sections harder to read than the simple "Config properties"
editor. Everything that is a Trino `config.properties`/`node.properties`/`log.properties`
value moves into one Properties list; only genuinely-structured k8s/engine config keeps a
form. The Configuration nav shrinks from 14 sections to ~5. **Frontend only — no payload
change** (values still serialize into the existing `configProperties` /
`additionalNodeProperties` / `additionalLogProperties` maps).

**Non-goals:** backend changes; forcing structured config (auth users, HPA/KEDA, pod
spec, ingress, resource-groups, catalogs) into flat KV — those stay as forms (a form beats
raw JSON for a password-user list or an HPA); security/observability completeness (#3/#4).

## The full Trino property catalog

`config/trinoCatalog.ts` — extracted from the Trino docs (`data-portal/trino/docs/src/
main/sphinx/admin/properties-*.md`, `fault-tolerant-execution.md`, etc.):
`export interface TrinoProp { key: string; group: string; file: "config"|"node"|"log";
type: "string"|"number"|"boolean"|"enum"|"size"|"duration"; default?: string; help: string;
docsUrl?: string; enumOptions?: string[] }` + `export const trinoCatalog: TrinoProp[]`
(~200-400 entries — the whole operator-facing surface). `file` says which `.properties`
map the value belongs to (almost all `config`; a few `node`; per-logger levels `log`).
The old `knownProperties.ts` (31) is superseded by / folded into this catalog.

## The unified **Properties** section

`config/PropertiesSection.tsx` — the primary config surface:
- A **table of the currently-set properties** across the three maps (`configProperties`,
  `additionalNodeProperties`, `additionalLogProperties`): key + a **typed value editor**
  (`PropertyField` by the catalog `type` when the key is in the catalog; a plain input for
  a custom/unknown key) + remove. A group chip (from the catalog `group`) per row.
- An **"Add property"** control: a combobox over the WHOLE `trinoCatalog` (search by
  key/label/help, grouped), showing type/default/help; picking one adds a row routed to its
  `file` map with a typed value editor; a "custom key…" affordance still allows an arbitrary
  key → `config.properties` (free text).
- A **search box** filters the visible rows + the add-combobox.
- Reads/writes via the hook's `configProps`/`nodeProps`/`logProps` state (a small
  `getProp(file,key)`/`setProp(file,key,val)` over the right map — extend the existing
  `getProp`/`setProp`). Payload maps unchanged.
This replaces the four guided tuning sections (Query & Memory / Spilling / Execution &
Optimizer / SQL & Web UI) AND the raw config/node/log KeyValueEditors (General/Advanced).

## Nav restructure (14 → 5 Configuration sections)

- **Properties** — the unified list above (absorbs the 4 tuning sections + raw config/node/
  log properties + `queryMaxMemory`/`logLevel`/`log.format`, which are all properties now).
- **Resources** — per-role Coordinator/Worker (a role toggle): replicas, cpu/mem
  request+limit, heap, probes, additional JVM config, query-max-memory-per-node,
  heap-headroom, graceful-shutdown; plus cluster `env` vars. (merges the current
  Coordinator + Worker sections.)
- **Workload** — resource groups + session properties + fault-tolerant execution
  (retry-policy + exchange storage conn). (merges Workload + Reliability.)
- **Scheduling** — Pod + Placement + Containers & Mounts (merges the 3 pod sections; keep
  their internal collapsible sub-groups).
- **Scaling** — Autoscaling (HPA/KEDA).
Security & Exposure stays a top-level tab; Catalogs stays in Overview. With ~5 sections the
grouped-collapsible nav can flatten to a simple list (drop the group headers), or keep 2
light groups — implementer's call for the cleanest look. Non-default badges stay.

## Component disposition
- Reuse: `PropertyField`, `FieldLabel`, `QuantityInput`, `ConfigSection`, the role/pod/
  autoscaling/security/resource-group editors (moved under the merged sections, unchanged
  internally).
- Remove: `QueryMemorySection`/`SpillingSection`/`ExecutionOptimizerSection`/
  `SqlWebUiSection` (their props now live in the Properties list via the catalog);
  `EngineConfigGeneral`/`EngineConfigAdvanced` shrink (their raw KV editors move into
  Properties; env moves to Resources; the remaining pieces fold in).
- `PropertySearch` (jump-to-section) is superseded by the Properties list's own search;
  remove or repurpose.
- `sectionCounts` updates to the new 5 section keys.

## Test
- Unit: catalog integrity (every entry has key/group/file/type/help); `getProp`/`setProp`
  route to the right map by `file`; the Properties list surfaces set keys from all three
  maps; adding a catalog property routes to its map with a typed editor; the built payload
  keeps the three maps' shapes.
- **Playwright**: Configuration shows ~5 nav sections; Properties shows current props +
  "Add property" opens a searchable combobox over the full catalog (hundreds); picking
  `spill-enabled` adds a boolean row, `join-distribution-type` an enum row, a size prop a
  QuantityInput; a custom key types through; Save → the PUT `configProperties` (+ node/log
  maps) carry them unchanged in shape. 0 console errors.
- **Live (kind)**: a couple set → configmap `config-coordinator.properties` shows them.

## Compat
- Frontend-only; no migration/API/payload-shape change (same three properties maps).
- This supersedes the #2 guided-tuning-sections UI (built on the same catalog data, so the
  data + `PropertyField` carry over; only the section wrappers are replaced).
