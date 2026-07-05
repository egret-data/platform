# Properties-First Config UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Re-center the Configuration UI on one searchable Properties list backed by the full Trino property catalog, and shrink the nav from 14 sections to 5.

**Architecture:** A `trinoCatalog` (extracted from the Trino docs) drives a single `PropertiesSection` (list of set config/node/log properties + an add-from-catalog combobox with typed value editors); the hook's `getProp`/`setProp` route to the right `.properties` map by a `file` tag; the nav merges into 5 sections (Properties / Resources / Workload / Scheduling / Scaling) reusing the existing role/pod/workload/scaling editors. **Frontend only — the three properties maps' payload shape is unchanged.**

**Tech Stack:** React 18, shadcn, Tailwind v4, Vitest.

**Spec:** `docs/en/specs/2026-07-05-properties-first-config-design.md`.

---

## File Structure
- **Create** `config/trinoCatalog.ts`, `config/PropertiesSection.tsx`, `config/ResourcesSection.tsx`, `config/SchedulingSection.tsx`, `config/WorkloadSection.tsx`.
- **Modify** `useEngineConfigForm.ts` (getProp/setProp by file + sectionCounts), `EngineDetail.tsx` (5-section nav), and remove the now-dead tuning sections.

---

## Task 1: Full Trino property catalog (docs extraction)

**Files:** Create `config/trinoCatalog.ts`

- [ ] **Step 1** Extract the operator-facing property surface from the Trino docs at
  `/Users/1pro-l4r8/Documents/github/data-portal/trino/docs/src/main/sphinx/admin/` —
  every `properties-*.md` (general, resource-management, query-management, spilling,
  exchange, task, optimizer, node-scheduler, web-interface, logging, http-server,
  writer-scaling, write-partitioning, sql-environment, catalog, http-client) +
  `fault-tolerant-execution.md`. Produce:

```ts
export type PropType = "string" | "number" | "boolean" | "enum" | "size" | "duration";
export interface TrinoProp { key: string; group: string; file: "config" | "node" | "log"; type: PropType; default?: string; help: string; docsUrl?: string; enumOptions?: string[] }
export const trinoCatalog: TrinoProp[] = [ /* ~200-400 entries */ ];
export const trinoCatalogByKey: Record<string, TrinoProp> = Object.fromEntries(trinoCatalog.map(p => [p.key, p]));
```

  Extraction rules: `group` = the doc/section it came from (e.g. "Memory", "Query
  management", "Spilling", "Task", "Optimizer", "Node scheduler", "Exchange", "Web UI",
  "Logging", "SQL environment", "Fault-tolerant", "General"). `file` = `config` for
  everything EXCEPT `node.*` identity keys (→ `node`) and per-logger `log.*` levels (→
  `log`); `log.format`/`log.max-size` etc. are `config`. `type`: a value ending in a data
  size / named `*memory*`/`*buffer*`/`*spill*-size*` → `size`; a duration / `*time*`/
  `*timeout*`/`*period*`/`*delay*` → `duration`; `true`/`false` default → `boolean`; a
  fixed value set → `enum` (fill `enumOptions`); an integer default → `number`; else
  `string`. `help` = a one-line paraphrase of the doc description; `docsUrl` = the doc page
  anchor. Fold in the existing `knownProperties.ts` entries (reuse their help/enum where
  they overlap). Aim for the well-documented ~200-400 properties; it's fine to omit truly
  obscure internal knobs — `log()`-style note not needed (this is a data file).

- [ ] **Step 2** `tsc`/`build`. A quick sanity test `config/trinoCatalog.test.ts`: assert
  `trinoCatalog.length > 150`, every entry has non-empty `key`/`group`/`help` + a valid
  `type`/`file`, and keys are unique. Commit `feat(ui): full Trino property catalog`.

---

## Task 2: Hook getProp/setProp by file

**Files:** Modify `useEngineConfigForm.ts`

- [ ] **Step 1** Change `getProp`/`setProp` to take a `file: "config" | "node" | "log"`
  first arg and operate on the matching state (`configProps`+`setConfigProps` /
  `nodeProps`+`setNodeProps` / `logProps`+`setLogProps`):

```ts
  const mapFor = (file: "config"|"node"|"log") =>
    file === "node" ? [nodeProps, setNodeProps] as const
    : file === "log" ? [logProps, setLogProps] as const
    : [configProps, setConfigProps] as const;
  const getProp = (file: "config"|"node"|"log", key: string) =>
    mapFor(file)[0].find(kv => kv.key === key)?.value ?? "";
  const setProp = (file: "config"|"node"|"log", key: string, value: string) => {
    const set = mapFor(file)[1];
    set(prev => { const rest = prev.filter(kv => kv.key !== key); return value.trim()==="" ? rest : [...rest, { key, value }]; });
  };
```

  Update the `EngineConfigForm` interface signatures. (Callers of the old single-arg
  `getProp`/`setProp` are the tuning sections being removed in Task 5 + the Reliability/
  Advanced folds — update them to pass `"config"`.)
- [ ] **Step 2** `tsc`/`build`/`vitest`. Commit `feat(ui): getProp/setProp routed by properties file`.

---

## Task 3: Unified PropertiesSection

**Files:** Create `config/PropertiesSection.tsx`; Modify `EngineDetail.tsx` (temporary mount)

- [ ] **Step 1** `PropertiesSection({ cfg })` (cfg = the hook return). Render inside a
  `ConfigSection` titled "Properties" (description: cluster-wide Trino config.properties /
  node.properties / log.properties):
  - Build the row list = `[...cfg.configProps.map(kv=>({file:"config",...kv})), ...cfg.nodeProps.map(...node), ...cfg.logProps.map(...log)]`.
  - A **search `<Input>`** filtering rows by key.
  - A **table**: each row = the key (mono; a group chip from `trinoCatalogByKey[key]?.group` when known) + a **value editor**: if `trinoCatalogByKey[key]` exists, a `<PropertyField prop={{...catalog, label: key}} value={cfg.getProp(file,key)} onChange={v=>cfg.setProp(file,key,v)} />`; else a plain `<Input>` for the custom key's value; + a remove (trash) button (`cfg.setProp(file,key,"")`).
  - An **"Add property"** control: a combobox over `trinoCatalog` (search key/label/help; show group + type + default; grouped or flat list, up to ~30 shown filtered) — selecting a prop calls `cfg.setProp(prop.file, prop.key, prop.default ?? "")` (adds the row, editable inline) and scrolls to it; plus a "+ custom key" affordance that adds an empty `config` row with a free-text key input. Reuse the `SuggestKeyInput`/combobox pattern from `KeyValueEditor` or build a small popover.
- [ ] **Step 2** Temporarily add a `Properties` nav item in `EngineDetail.tsx` rendering
  `<PropertiesSection cfg={cfg} />` (the full nav restructure is Task 5) to verify it works.
- [ ] **Step 3** `tsc`/`eslint`/`build`; the Properties list shows set props + add-from-catalog
  + typed editors + remove + search. Commit `feat(ui): unified PropertiesSection over the full catalog`.

---

## Task 4: Merged section components (Resources / Workload / Scheduling)

**Files:** Create `config/ResourcesSection.tsx`, `config/WorkloadSection.tsx`, `config/SchedulingSection.tsx`

- [ ] **Step 1: `ResourcesSection`** — a Coordinator/Worker role toggle (reuse the pattern
  from PlacementSection) rendering the existing `EngineRoleConfig` for the selected role,
  PLUS the cluster `env` KeyValueEditor (moved from General). Props: the coordinator/worker
  role slices + env slice from `cfg`.
- [ ] **Step 2: `WorkloadSection`** — renders the existing `ResourceGroupsSection` +
  `SessionPropertiesSection` (from EngineResourceGroups) + the existing `EngineConfigReliability`
  fault-tolerant content (retry-policy + exchange storage conn). Pass the slices from `cfg`.
- [ ] **Step 3: `SchedulingSection`** — renders `PodSection` + `PlacementSection` +
  `ContainersMountsSection` (the three existing pod components) stacked (each already a
  `ConfigSection`). Pass their slices from `cfg`.
- [ ] **Step 4** `tsc`/`build`; each merged section renders its parts. Commit `feat(ui): merged Resources/Workload/Scheduling sections`.

---

## Task 5: Nav restructure to 5 sections + remove dead components

**Files:** Modify `EngineDetail.tsx`, `useEngineConfigForm.ts`; Delete `QueryMemorySection.tsx`, `SpillingSection.tsx`, `ExecutionOptimizerSection.tsx`, `SqlWebUiSection.tsx`, `PropertySearch.tsx`

- [ ] **Step 1** Replace the grouped 14-item nav in `ConfigurationTab` with a flat 5-item
  nav: `Properties` → `<PropertiesSection cfg />`, `Resources` → `<ResourcesSection cfg />`,
  `Workload` → `<WorkloadSection cfg />`, `Scheduling` → `<SchedulingSection cfg />`,
  `Scaling` → `<EngineAutoscaling cfg />`. Drop the `NAV_GROUPS` collapsible grouping (5
  items is short enough for a flat list). Keep the `SaveBar` (with Review changes) + the
  `ValidityProvider` wrapper.
- [ ] **Step 2** Remove the mounts of the removed sections (Query & Memory / Spilling /
  Execution / SQL & Web UI / General / Advanced / Coordinator / Worker / Workload /
  Reliability / Pod / Placement / Containers & Mounts as top-level nav items) — their
  content now lives inside Properties / Resources / Workload / Scheduling. Delete the four
  tuning section files + `PropertySearch.tsx` (its search is superseded by the Properties
  list search). Shrink `EngineConfigGeneral`/`EngineConfigAdvanced` — the raw config/node/log
  KV editors move into Properties; the log.format/event-listener known bits go through the
  catalog; if nothing meaningful remains in those two files, delete them (move `env` to
  ResourcesSection).
- [ ] **Step 3** Update `sectionCounts` to the 5 keys: `properties` (count of set keys
  across config/node/log maps), `resources` (coordinator+worker+env), `workload`
  (resourceGroups+sessionProperties+faultTolerant), `scheduling` (pod+placement+
  containersMounts groups), `scaling` (autoscaling). Badge the 5 nav items.
- [ ] **Step 4** `tsc`/`eslint`/`build`/`vitest`; the nav shows exactly 5 sections, every
  former field reachable, save works. Commit `feat(ui): 5-section properties-first Configuration nav`.

---

## Task 6: Gate + Playwright + live + PR

- [ ] **Step 1** `cd frontend`: `npx tsc --noEmit` (0), `npm run lint` (0 errors), `npx vitest run` (green), `npm run build`.
- [ ] **Step 2: Playwright**: Configuration shows exactly 5 nav sections (Properties/
  Resources/Workload/Scheduling/Scaling); Properties lists current props + "Add property"
  opens a searchable combobox over the full catalog (150+); typing "spill" finds
  `spill-enabled` → adds a boolean row, "join-dist" → `join-distribution-type` enum row, a
  size prop → QuantityInput; a custom key types through; Resources shows the Coordinator/
  Worker toggle + fields; Scheduling shows Pod/Placement/Containers; Scaling shows
  Autoscaling. Save → the `updateEngine` PUT `trinoConfig.configProperties` (+ node/log
  maps) carry the set keys. 0 console errors.
- [ ] **Step 3: Live (kind)**: a couple set → `kubectl get configmap demo2-config` shows them
  in `config-coordinator.properties`.
- [ ] **Step 4** Commit; open PR; run the full frontend gate before an admin squash-merge.

---

## Self-Review notes
- **Spec coverage:** full catalog (T1) ✓; getProp/setProp by file (T2) ✓; unified Properties list w/ typed value + add-from-catalog + search (T3) ✓; merged Resources/Workload/Scheduling reusing existing editors (T4) ✓; 5-section nav + remove dead + sectionCounts (T5) ✓; verify incl. live (T6) ✓.
- **Type consistency:** `TrinoProp`/`PropType`/`trinoCatalogByKey` ↔ `PropertyField` prop; `getProp(file,key)`/`setProp(file,key,val)` used by PropertiesSection + the Reliability/Advanced folds updated to pass `"config"`.
- **Payload unchanged:** all three maps (config/node/log Properties) serialize as before; no field/backend change; the structured forms (role/workload/pod/scaling/security) reuse existing editors unchanged internally.
- **Supersedes #2:** the 4 tuning section wrappers are removed; their data (the catalog) grows into `trinoCatalog`; `knownProperties.ts` is folded in / kept only if still referenced.
- **Deferred:** per-role property overrides in the Properties list (cluster-wide only for now); security/observability (#3/#4).
