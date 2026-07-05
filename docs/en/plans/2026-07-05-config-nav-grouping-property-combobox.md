# Config Nav Grouping + Property-Key Combobox Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** De-clutter the Configuration nav (14 flat items → 5 collapsible groups) and make the raw config.properties key field a Trino-property-name combobox.

**Architecture:** A `groups` array drives a collapsible grouped vertical nav; `KeyValueEditor` gains an optional `keySuggestions` combobox for its key field; the config.properties editor suggests the long-tail catalog and drops guided keys. Reuses the #1 UX framework. **Frontend only — payload unchanged.**

**Tech Stack:** React 18, shadcn (Collapsible, Command/Popover or a simple dropdown), Tailwind v4, Vitest.

**Spec:** `docs/en/specs/2026-07-05-config-nav-grouping-property-combobox-design.md`.

---

## File Structure
- **Create** `frontend/src/routes/control-plane/config/trinoPropertyNames.ts`.
- **Modify** `EngineResourceGroups.tsx` (`KeyValueEditor` optional `keySuggestions`), `EngineConfigGeneral.tsx` (pass suggestions + drop guided), `EngineDetail.tsx` (grouped nav).

---

## Task 1: trinoPropertyNames catalog + KeyValueEditor keySuggestions combobox

**Files:** Create `config/trinoPropertyNames.ts`; Modify `EngineResourceGroups.tsx`

- [ ] **Step 1: `trinoPropertyNames.ts`** — a curated broad list (superset of the 31 guided; ~50+ to start, extensible). Include the guided keys too (they get filtered out where used):

```ts
export interface TrinoPropertyName { key: string; help?: string }
export const trinoPropertyNames: TrinoPropertyName[] = [
  { key: "query.max-memory", help: "Cluster user memory per query" },
  { key: "query.max-total-memory", help: "Cluster total memory per query" },
  { key: "query.max-memory-per-node", help: "Worker user memory per query" },
  { key: "query.max-cpu-time", help: "Max cluster CPU time per query" },
  { key: "query.max-execution-time", help: "Max execution time" },
  { key: "query.max-run-time", help: "Max total lifetime incl. queue" },
  { key: "query.max-planning-time", help: "Max planning time" },
  { key: "query.client.timeout", help: "Abandon idle-client queries" },
  { key: "query.low-memory-killer.policy", help: "OOM kill policy" },
  { key: "query.max-history", help: "Completed queries kept for UI" },
  { key: "query.max-length", help: "Max SQL text length" },
  { key: "query.max-stage-count", help: "Max stages per query" },
  { key: "query.execution-policy", help: "phased | all-at-once" },
  { key: "query.max-hash-partition-count", help: "Max distributed partitions" },
  { key: "query.min-hash-partition-count", help: "Min distributed partitions" },
  { key: "memory.heap-headroom-per-node", help: "Untracked-alloc headroom" },
  { key: "spill-enabled", help: "Spill to disk" },
  { key: "spiller-spill-path", help: "Spill directories" },
  { key: "spiller-threads", help: "Spiller threads" },
  { key: "max-spill-per-node", help: "Max spill per node" },
  { key: "query-max-spill-per-node", help: "Max spill per node per query" },
  { key: "spill-compression-codec", help: "NONE | LZ4 | ZSTD" },
  { key: "spill-encryption-enabled", help: "Encrypt spilled data" },
  { key: "task.concurrency", help: "Local operator concurrency" },
  { key: "task.max-worker-threads", help: "Split worker threads" },
  { key: "task.writer-count", help: "Writer parallelism" },
  { key: "task.max-drivers-per-task", help: "Max drivers per task" },
  { key: "task.info-update-interval", help: "Task info refresh interval" },
  { key: "join-distribution-type", help: "AUTOMATIC | PARTITIONED | BROADCAST" },
  { key: "optimizer.join-reordering-strategy", help: "CBO join reordering" },
  { key: "optimizer.max-reordered-joins", help: "Max joins reordered by CBO" },
  { key: "optimizer.optimize-metadata-queries", help: "Metadata-only optimization" },
  { key: "redistribute-writes", help: "Hash-redistribute before write" },
  { key: "scale-writers", help: "Dynamic writer scale-out" },
  { key: "use-preferred-write-partitioning", help: "Preferred write partitioning" },
  { key: "node-scheduler.include-coordinator", help: "Schedule work on coordinator" },
  { key: "node-scheduler.max-splits-per-node", help: "Max splits per node" },
  { key: "node-scheduler.policy", help: "uniform | topology" },
  { key: "exchange.compression-codec", help: "Inter-node exchange compression" },
  { key: "exchange.client-threads", help: "Exchange client threads" },
  { key: "exchange.max-buffer-size", help: "Exchange buffer size" },
  { key: "exchange.data-integrity-verification", help: "NONE | ABORT | RETRY" },
  { key: "sql.default-catalog", help: "Default client catalog" },
  { key: "sql.default-schema", help: "Default client schema" },
  { key: "sql.forced-session-time-zone", help: "Force cluster time zone" },
  { key: "sql.path", help: "Default function path" },
  { key: "web-ui.enabled", help: "Expose the Web UI" },
  { key: "web-ui.authentication.type", help: "Web UI auth mechanism" },
  { key: "web-ui.shared-secret", help: "Web-UI cookie secret" },
  { key: "web-ui.session-timeout", help: "Web UI session timeout" },
  { key: "log.format", help: "TEXT | JSON" },
  { key: "log.max-size", help: "server.log rotation size" },
  { key: "log.max-total-size", help: "server.log total size cap" },
  { key: "http-server.log.enabled", help: "HTTP request log" },
  { key: "retry-policy", help: "NONE | TASK | QUERY" },
  { key: "query-retry-attempts", help: "Max query retries" },
  { key: "task-retry-attempts-per-task", help: "Max task retries" },
  { key: "fault-tolerant-execution-task-memory", help: "FTE task memory estimate" },
  { key: "catalog.management", help: "static | dynamic" },
  { key: "regex-library", help: "JONI | RE2J" },
];
```

- [ ] **Step 2: `KeyValueEditor` combobox** — STUDY `KeyValueEditor` in `EngineResourceGroups.tsx` (it renders `rows.map(row => <Input key .../> <Input value .../> <trash>)`). Add an optional prop `keySuggestions?: { key: string; help?: string }[]`. When set, replace the KEY `<Input>` with a small combobox: an `<Input>` plus, when it has focus AND the typed text matches ≥1 suggestion (case-insensitive substring on `key`) that isn't already an exact match, an absolute `z-10` dropdown listing up to 8 matches (`key` mono + `help` muted); `onMouseDown`-select fills that row's key (call the existing per-row key `onChange`); free text is accepted normally (typing anything, blur commits). When `keySuggestions` is absent, render the plain `<Input>` exactly as today (env/annotation/etc. editors unchanged). Extract the combobox into a small local `SuggestKeyInput` component in the same file.
- [ ] **Step 3** `tsc`/`build`/`vitest`. Commit `feat(ui): trinoPropertyNames catalog + KeyValueEditor key combobox`.

---

## Task 2: Wire combobox into config.properties editor + drop guided keys

**Files:** Modify `EngineConfigGeneral.tsx`

- [ ] **Step 1** The config.properties `KeyValueEditor` currently gets `rows={configProps.filter(kv => !knownPropertyKeys.has(kv.key))}` + an `onChange` that merges `[...known-entries, ...rows]`. Change:
  - Pass `keySuggestions={trinoPropertyNames.filter(p => !knownPropertyKeys.has(p.key))}` (import `trinoPropertyNames` from `./config/trinoPropertyNames`, `knownPropertyKeys` is already imported) — so the combobox suggests only long-tail keys.
  - In `onChange(rows)`, DROP any row whose key is a guided key before merging: `const custom = rows.filter(kv => !knownPropertyKeys.has(kv.key)); setConfigProps([...configProps.filter(kv => knownPropertyKeys.has(kv.key)), ...custom]);` — so a guided key typed into the raw editor never persists (fixes the ghost `query.max-run-time × 3`).
- [ ] **Step 2** `tsc`/`build`; the raw config.properties editor shows a key combobox + never keeps a guided key. Commit `feat(ui): config.properties key combobox + drop guided keys from raw`.

---

## Task 3: Grouped collapsible Configuration nav

**Files:** Modify `EngineDetail.tsx`

- [ ] **Step 1** In `ConfigurationTab`, define the groups:

```tsx
const NAV_GROUPS: { label: string; sections: { key: string; label: string }[] }[] = [
  { label: "Basics", sections: [{ key: "general", label: "General" }] },
  { label: "Roles", sections: [{ key: "coordinator", label: "Coordinator" }, { key: "worker", label: "Worker" }] },
  { label: "Workload & Tuning", sections: [
    { key: "workload", label: "Workload" }, { key: "reliability", label: "Reliability" },
    { key: "queryMemory", label: "Query & Memory" }, { key: "spilling", label: "Spilling" },
    { key: "execution", label: "Execution & Optimizer" }, { key: "sqlWebUi", label: "SQL & Web UI" } ] },
  { label: "Pods & Scheduling", sections: [
    { key: "pod", label: "Pod" }, { key: "placement", label: "Placement" },
    { key: "containersMounts", label: "Containers & Mounts" }, { key: "autoscaling", label: "Autoscaling" } ] },
  { label: "Advanced", sections: [{ key: "advanced", label: "Advanced" }] },
];
```

  (Use the SAME section keys the current flat nav uses — verify each against the existing nav so every one of the 14 is present exactly once + renders the same component.)

- [ ] **Step 2** Replace the flat `<nav>` list with a grouped render: for each group, a header button (chevron + label + a summed badge = the sum of `cfg.sectionCounts[s.key]` over its sections, shown when > 0) that toggles the group's open state; when open, the group's section buttons (each: label + its own `sectionCounts` badge, active-state highlight, `onClick={() => setSection(s.key)}`). Open state: `const [openGroups, setOpenGroups] = useState<Record<string, boolean>>(() => …)` initialised so the group containing the initial `section` is open (plus "Basics"). Reuse the app's `Collapsible`/`CollapsibleTrigger`/`CollapsibleContent` primitive (used by probes) OR a simple button + conditional list — match the existing pattern.
- [ ] **Step 3** Keep the right content pane + `SaveBar` + `PropertySearch` (jump still calls `setSection`; ensure jumping to a section also opens its group — when `setSection(key)` is called from search, open the group that contains `key`). Wrap `setSection` in a helper `selectSection(key)` that sets the section AND opens its group.
- [ ] **Step 4** `tsc`/`eslint`/`build`/`vitest`; the nav shows 5 group headers, collapsing/expanding works, all 14 sections reachable, badges on items + summed on collapsed groups. Commit `feat(ui): grouped collapsible Configuration nav`.

---

## Task 4: Gate + Playwright + PR

- [ ] **Step 1** `cd frontend`: `npx tsc --noEmit` (0), `npm run lint` (0 errors), `npx vitest run` (green), `npm run build`.
- [ ] **Step 2: Playwright**: Configuration shows 5 group headers (Basics/Roles/Workload & Tuning/Pods & Scheduling/Advanced); clicking a header collapses/expands its items; a collapsed group with set config shows a summed badge; PropertySearch jumping to "Spilling" opens the Workload & Tuning group + selects Spilling. On General → config.properties "Add property": focus the key field, type "exchange" → a suggestion dropdown lists `exchange.*` keys, pick one → the key fills; type a fully custom key → accepted; there is no persistent `query.max-run-time` ghost row (guided key dropped). Save → the `updateEngine` PUT payload is unchanged in shape. 0 console errors.
- [ ] **Step 3** Commit; open PR; run the full frontend gate before an admin squash-merge.

---

## Self-Review notes
- **Spec coverage:** trinoPropertyNames + KeyValueEditor combobox (T1) ✓; config.properties suggestions + drop-guided ghost fix (T2) ✓; grouped collapsible nav + summed badges + open-active-group + search-jump-opens-group (T3) ✓; verify (T4) ✓.
- **Type consistency:** `TrinoPropertyName{key,help?}` ↔ `keySuggestions` prop; nav group `section` keys match the existing flat-nav keys (T3 Step 1 verifies) + `cfg.sectionCounts` keys.
- **Payload unchanged:** combobox only changes how a key is entered; drop-guided removes rows that were duplicates anyway; nav is visual-only.
- **Backward-compatible:** `KeyValueEditor` without `keySuggestions` behaves exactly as before (env/annotations unaffected).
- **Deferred (YAGNI):** per-key value typing in the raw editor (guided fields already cover typed values); persisting group open-state across reloads.
