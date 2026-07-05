# Trino tuning completeness — guided config properties (2026-07-05)

VI reference: `docs/vi/specs/2026-07-05-trino-tuning-completeness-design.md`.

**Goal:** Give the ~25 highest-value commonly-tuned Trino server properties a guided,
typed, documented UI instead of only the raw `config.properties` escape hatch, plus a
search mechanism to find any known property. Sub-project #2 of 4 (after the UX framework).
**Frontend only — no backend change.**

**Key architecture:** these are (almost) all cluster-wide `config.properties` keys, and
egret ALREADY renders the `configProperties` record into `config.properties` on BOTH roles
(`config_properties(spec, coord)` at `trino.rs`). So the guided fields simply read/write
KNOWN keys in the existing `configProperties` map — no new orchestrator fields, no
carriers, no payload shape change (the same `trinoConfig.configProperties` object).

**Non-goals:** new backend structured fields; security/TLS/access-control (#3);
observability (#4); properties that egret already owns structurally (`query.max-memory` =
`queryMaxMemory` field, log level, `internal-communication.shared-secret` = auto when auth
on) — excluded from the catalog to avoid double-ownership.

## The catalog

`config/knownProperties.ts` — `KnownProperty { key: string; section: SectionKey; label: string; type: "string"|"number"|"boolean"|"enum"|"size"|"duration"; default?: string; help: string; docsUrl?: string; enumOptions?: string[] }` + `export const knownProperties: KnownProperty[]`. Curated ~25 (extensible to the whole surface later). Types drive the input widget.

## Guided fields — write into `configProperties`

- `config/PropertyField.tsx` — `PropertyField({ prop, value, onChange })`: renders by
  `prop.type` — `size` → `QuantityInput`; `enum` → `Select` (from `enumOptions`); `boolean`
  → `Switch`; `number`/`duration`/`string` → `Input`; all wrapped with `FieldLabel`
  (help/docs come from the `prop` itself). Reads/writes a single `config.properties` value.
- `useEngineConfigForm` gains `getProp(key): string | undefined` + `setProp(key, value)`
  operating on the existing `configProps` (Kv[]) state (upsert on set, remove when blank).
  No payload change — `configProps` still serializes to `trinoConfig.configProperties`.
- The raw `config.properties` KeyValueEditor (General/Advanced) is FILTERED to hide keys
  present in `knownProperties` (those are owned by the guided UI) — so no duplication; it
  still handles arbitrary custom keys.

## UI — new Configuration nav sections

Add four domain sections to the vertical nav (after Reliability, before Advanced) +
fold two groups into existing sections:
- **Query & Memory** (new): query.max-total-memory, query.max-cpu-time,
  query.max-execution-time, query.max-run-time, query.max-planning-time,
  query.client.timeout, query.low-memory-killer.policy, query.max-history,
  query.max-hash-partition-count, query.min-hash-partition-count.
- **Spilling** (new): spill-enabled, spiller-spill-path, max-spill-per-node,
  query-max-spill-per-node.
- **Execution & Optimizer** (new): task.concurrency, task.max-worker-threads,
  join-distribution-type, optimizer.join-reordering-strategy,
  node-scheduler.include-coordinator, node-scheduler.max-splits-per-node,
  exchange.compression-codec.
- **SQL & Web UI** (new): sql.default-catalog, sql.default-schema,
  sql.forced-session-time-zone, web-ui.enabled, web-ui.authentication.type,
  web-ui.shared-secret, catalog.management.
- **Reliability** (existing): add query-retry-attempts + task-retry-attempts-per-task
  (fault-tolerant retry ceilings) as guided fields.
- **Advanced** (existing): add log.format (TEXT/JSON) as a guided field.
Each new section is a component rendering its `knownProperties`-filtered list via
`PropertyField`, wrapped in `ConfigSection`. `sectionCounts` gains counts for the new
section keys (a section counts a set config.properties key belonging to it), so the nav
badges work.

## Search — find any property

`config/PropertySearch.tsx` — a search `<Input>` at the top of the Configuration card that
filters `knownProperties` by key/label/help; matches show as a list (key + section label +
one-line help); clicking a result switches the vertical nav to that property's section (and
optionally scrolls/highlights it). Covers discoverability + the growing long tail.

## Test
- Unit: the catalog → `PropertyField` widget selection per type; `getProp`/`setProp` upsert
  + remove-on-blank over `configProps`; the raw editor filters out known keys; the built
  `trinoConfig.configProperties` contains a set known key + omits blank ones.
- **Playwright**: the new sections render grouped fields with help; setting `spill-enabled`
  (Switch) + `task.concurrency` (number) + `join-distribution-type=BROADCAST` (Select) →
  Save → the PUT `configProperties` carries `{spill-enabled:"true", task.concurrency:"…",
  join-distribution-type:"BROADCAST"}`; the property search finds "spill" and jumps to
  Spilling. 0 console errors.
- **Live (kind)**: provision with a few set → `kubectl get configmap demo2-config -o yaml`
  shows the keys in `config-coordinator.properties` (and worker where applicable).

## Compat
- Frontend-only; no migration, no API/route/payload-shape change (same
  `trinoConfig.configProperties` map). Existing raw config.properties still works for custom
  keys. Reuses the #1 UX framework primitives.
