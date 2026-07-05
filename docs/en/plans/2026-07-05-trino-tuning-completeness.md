# Trino Tuning Completeness — Guided Config Properties Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give ~25 high-value Trino `config.properties` a guided/typed/documented UI (+ a property search) by reading/writing the existing `configProperties` map — no backend change.

**Architecture:** A `knownProperties` catalog drives a `PropertyField` widget-per-type; new Configuration nav sections render grouped known properties; the hook gains `getProp`/`setProp` over the existing `configProps` state; the raw config.properties editor hides known keys. Reuses the #1 UX framework primitives (`ConfigSection`, `FieldLabel`, `QuantityInput`, vertical nav, `sectionCounts`). **Frontend only — the `trinoConfig.configProperties` payload is unchanged in shape.**

**Tech Stack:** React 18, shadcn (Select/Switch/Input), Vitest.

**Spec:** `docs/en/specs/2026-07-05-trino-tuning-completeness-design.md`.

---

## File Structure
- **Create** `config/knownProperties.ts`, `config/PropertyField.tsx`, `config/QueryMemorySection.tsx`, `config/SpillingSection.tsx`, `config/ExecutionOptimizerSection.tsx`, `config/SqlWebUiSection.tsx`, `config/PropertySearch.tsx`.
- **Modify** `useEngineConfigForm.ts` (getProp/setProp + sectionCounts for new sections), `EngineDetail.tsx` (nav items + search), `EngineConfigGeneral.tsx`/`EngineConfigAdvanced.tsx` (raw editor filter + log.format), `EngineConfigReliability.tsx` (FTE retry attempts).

---

## Task 1: knownProperties catalog + hook getProp/setProp + raw-editor filter

**Files:** Create `config/knownProperties.ts`; Modify `useEngineConfigForm.ts`, `EngineConfigGeneral.tsx` (or wherever the raw config.properties KeyValueEditor lives)

- [ ] **Step 1: `knownProperties.ts`**

```ts
export type PropType = "string" | "number" | "boolean" | "enum" | "size" | "duration";
export interface KnownProperty { key: string; section: string; label: string; type: PropType; default?: string; help: string; docsUrl?: string; enumOptions?: string[] }
export const knownProperties: KnownProperty[] = [
  // queryMemory
  { key: "query.max-total-memory", section: "queryMemory", label: "Query max total memory", type: "size", help: "Cluster total memory (incl. revocable) a query may use.", docsUrl: "https://trino.io/docs/current/admin/properties-resource-management.html#query-max-total-memory" },
  { key: "query.max-cpu-time", section: "queryMemory", label: "Query max CPU time", type: "duration", help: "Max cluster CPU time a single query may use (runaway guard).", docsUrl: "https://trino.io/docs/current/admin/properties-query-management.html" },
  { key: "query.max-execution-time", section: "queryMemory", label: "Query max execution time", type: "duration", default: "100d", help: "Max active execution time before a query is killed.", docsUrl: "https://trino.io/docs/current/admin/properties-query-management.html#query-max-execution-time" },
  { key: "query.max-run-time", section: "queryMemory", label: "Query max run time", type: "duration", default: "100d", help: "Max total lifetime incl. queue time.", docsUrl: "https://trino.io/docs/current/admin/properties-query-management.html#query-max-run-time" },
  { key: "query.max-planning-time", section: "queryMemory", label: "Query max planning time", type: "duration", default: "10m", help: "Max planning time before failing a query.", docsUrl: "https://trino.io/docs/current/admin/properties-query-management.html" },
  { key: "query.client.timeout", section: "queryMemory", label: "Client timeout", type: "duration", default: "5m", help: "Abandon a query if the client stops contacting the server.", docsUrl: "https://trino.io/docs/current/admin/properties-query-management.html" },
  { key: "query.low-memory-killer.policy", section: "queryMemory", label: "Low-memory killer policy", type: "enum", default: "total-reservation-on-blocked-nodes", enumOptions: ["none","total-reservation","total-reservation-on-blocked-nodes"], help: "Which query to kill when the cluster runs low on memory." },
  { key: "query.max-history", section: "queryMemory", label: "Query history size", type: "number", default: "100", help: "Completed queries kept for the Web UI / API." },
  { key: "query.max-hash-partition-count", section: "queryMemory", label: "Max hash partition count", type: "number", default: "100", help: "Max partitions for distributed hash operations." },
  { key: "query.min-hash-partition-count", section: "queryMemory", label: "Min hash partition count", type: "number", default: "4", help: "Min partitions for distributed hash operations." },
  // spilling
  { key: "spill-enabled", section: "spilling", label: "Spill to disk", type: "boolean", default: "false", help: "Spill operator memory to disk when memory limits are hit.", docsUrl: "https://trino.io/docs/current/admin/properties-spilling.html" },
  { key: "spiller-spill-path", section: "spilling", label: "Spill path(s)", type: "string", help: "Directory/directories for spilled data (required when spill is enabled; needs a writable volume).", docsUrl: "https://trino.io/docs/current/admin/properties-spilling.html" },
  { key: "max-spill-per-node", section: "spilling", label: "Max spill per node", type: "size", default: "100GB", help: "Max spill space per node across all queries." },
  { key: "query-max-spill-per-node", section: "spilling", label: "Query max spill per node", type: "size", default: "100GB", help: "Max spill space per node for a single query." },
  // execution
  { key: "task.concurrency", section: "execution", label: "Task concurrency", type: "number", help: "Local concurrency for parallel operators (power of 2).", docsUrl: "https://trino.io/docs/current/admin/properties-task.html#task-concurrency" },
  { key: "task.max-worker-threads", section: "execution", label: "Max worker threads", type: "number", help: "Threads used to process splits (default CPU*2).", docsUrl: "https://trino.io/docs/current/admin/properties-task.html" },
  { key: "join-distribution-type", section: "execution", label: "Join distribution type", type: "enum", default: "AUTOMATIC", enumOptions: ["AUTOMATIC","PARTITIONED","BROADCAST"], help: "How joins are distributed across the cluster.", docsUrl: "https://trino.io/docs/current/admin/properties-general.html#join-distribution-type" },
  { key: "optimizer.join-reordering-strategy", section: "execution", label: "Join reordering strategy", type: "enum", default: "AUTOMATIC", enumOptions: ["AUTOMATIC","ELIMINATE_CROSS_JOINS","NONE"], help: "Cost-based join reordering.", docsUrl: "https://trino.io/docs/current/admin/properties-optimizer.html" },
  { key: "node-scheduler.include-coordinator", section: "execution", label: "Schedule work on coordinator", type: "boolean", default: "true", help: "Run splits on the coordinator too (keep off for large clusters).", docsUrl: "https://trino.io/docs/current/admin/properties-node-scheduler.html" },
  { key: "node-scheduler.max-splits-per-node", section: "execution", label: "Max splits per node", type: "number", default: "256", help: "Max splits scheduled on a node at once." },
  { key: "exchange.compression-codec", section: "execution", label: "Exchange compression", type: "enum", default: "NONE", enumOptions: ["NONE","LZ4","ZSTD"], help: "Compress inter-node exchange data." },
  // sqlWebUi
  { key: "sql.default-catalog", section: "sqlWebUi", label: "Default catalog", type: "string", help: "Default catalog for clients that don't set one." },
  { key: "sql.default-schema", section: "sqlWebUi", label: "Default schema", type: "string", help: "Default schema for clients that don't set one." },
  { key: "sql.forced-session-time-zone", section: "sqlWebUi", label: "Forced session time zone", type: "string", help: "Override the session time zone cluster-wide (e.g. UTC)." },
  { key: "web-ui.enabled", section: "sqlWebUi", label: "Web UI enabled", type: "boolean", default: "true", help: "Expose the Trino Web UI.", docsUrl: "https://trino.io/docs/current/admin/properties-web-interface.html" },
  { key: "web-ui.authentication.type", section: "sqlWebUi", label: "Web UI auth type", type: "enum", default: "FORM", enumOptions: ["FORM","FIXED","CERTIFICATE","KERBEROS","JWT","OAUTH2"], help: "Web UI authentication mechanism." },
  { key: "web-ui.shared-secret", section: "sqlWebUi", label: "Web UI shared secret", type: "string", help: "Stable secret for the Web-UI auth cookie (survives restarts)." },
  { key: "catalog.management", section: "sqlWebUi", label: "Catalog management", type: "enum", default: "static", enumOptions: ["static","dynamic"], help: "Allow CREATE/DROP CATALOG at runtime (dynamic)." },
  // reliability (folded into existing Reliability section)
  { key: "query-retry-attempts", section: "reliability", label: "Query retry attempts", type: "number", default: "4", help: "Max query retries under retry-policy=QUERY.", docsUrl: "https://trino.io/docs/current/admin/fault-tolerant-execution.html" },
  { key: "task-retry-attempts-per-task", section: "reliability", label: "Task retry attempts", type: "number", default: "4", help: "Max task retries under retry-policy=TASK." },
  // advanced (folded into existing Advanced section)
  { key: "log.format", section: "advanced", label: "Log format", type: "enum", default: "TEXT", enumOptions: ["TEXT","JSON"], help: "Server log format (JSON for centralized aggregation)." },
];
export const knownPropertyKeys = new Set(knownProperties.map(p => p.key));
```

- [ ] **Step 2: hook getProp/setProp** — in `useEngineConfigForm`, the raw config.properties lives in `configProps` (a `Kv[]` of `{key,value}` — verify the exact shape). Add:

```ts
  const getProp = (key: string) => configProps.find(kv => kv.key === key)?.value ?? "";
  const setProp = (key: string, value: string) => setConfigProps(prev => {
    const rest = prev.filter(kv => kv.key !== key);
    return value.trim() === "" ? rest : [...rest, { key, value }];
  });
```

  Expose `getProp`, `setProp`, and the raw `configProps`/`setConfigProps` in the hook return.

- [ ] **Step 3: filter the raw config.properties editor** — where the raw `config.properties` KeyValueEditor is rendered (General or Advanced), pass it only the NON-known keys and merge back on change:
  `const customProps = configProps.filter(kv => !knownPropertyKeys.has(kv.key));`
  on change: `setConfigProps([...configProps.filter(kv => knownPropertyKeys.has(kv.key)), ...newCustom])`. So the raw editor no longer shows/duplicates known keys.
- [ ] **Step 4** `tsc`/`build`/`vitest`. Commit `feat(ui): knownProperties catalog + getProp/setProp + raw-editor filter`.

---

## Task 2: PropertyField widget-by-type (TDD)

**Files:** Create `config/PropertyField.tsx`; Test `config/PropertyField.test.tsx`

- [ ] **Step 1: Failing test** — render `PropertyField` for each type + assert the widget:

```tsx
import { render, screen } from "@testing-library/react";
import { PropertyField } from "./PropertyField";
test("enum renders a combobox with options", () => {
  render(<PropertyField prop={{ key:"x", section:"s", label:"X", type:"enum", enumOptions:["A","B"], help:"h" }} value="A" onChange={()=>{}} />);
  expect(screen.getByRole("combobox")).toBeTruthy();
});
test("boolean renders a switch", () => {
  render(<PropertyField prop={{ key:"y", section:"s", label:"Y", type:"boolean", help:"h" }} value="true" onChange={()=>{}} />);
  expect(screen.getByRole("switch")).toBeTruthy();
});
```

  (Match the app's testing-library setup — check an existing component test for imports/render helper.)

- [ ] **Step 2** Run → FAIL.
- [ ] **Step 3: Implement** `PropertyField({ prop, value, onChange }: { prop: KnownProperty; value: string; onChange: (v: string) => void })`:
  - `size` → `<QuantityInput fieldKey=prop.key label=prop.label value onChange />` (also register `prop.help/docsUrl` — pass via a `fieldHelp`-like inline or extend `FieldLabel` to accept an inline `help`/`docsUrl`; simplest: give `FieldLabel` optional `help`/`docsUrl` props used when no `fieldHelp[fieldKey]` entry exists, and pass `prop.help`/`prop.docsUrl`).
  - `enum` → a `<FieldLabel>` + shadcn `<Select value onValueChange={onChange}>` over `prop.enumOptions` (plus an empty "(default)" option).
  - `boolean` → `<FieldLabel>` + `<Switch checked={value==="true"} onCheckedChange={v=>onChange(v?"true":"")} />`.
  - `number`/`duration`/`string` → `<FieldLabel>` + `<Input value onChange placeholder=prop.default />`.
  In all cases `FieldLabel` shows `prop.help` tooltip + `prop.docsUrl` link.
- [ ] **Step 4** Extend `FieldLabel` to accept optional `help`/`docsUrl` props (fallback to the `fieldHelp` map when absent) so catalog-driven help works without adding every key to `fieldHelp.ts`.
- [ ] **Step 5** Run test → pass. `tsc`/`build`. Commit `feat(ui): PropertyField widget-by-type`.

---

## Task 3: New sections + nav wiring + sectionCounts

**Files:** Create `config/QueryMemorySection.tsx`, `SpillingSection.tsx`, `ExecutionOptimizerSection.tsx`, `SqlWebUiSection.tsx`; Modify `EngineDetail.tsx`, `useEngineConfigForm.ts`

- [ ] **Step 1** Each section component: `Section({ getProp, setProp })` maps `knownProperties.filter(p => p.section === "<key>")` to `<PropertyField prop value={getProp(prop.key)} onChange={v=>setProp(prop.key, v)} />`, wrapped in a `ConfigSection` (title + description + a group docs link). One component per section: `queryMemory`, `spilling`, `execution`, `sqlWebUi`.
- [ ] **Step 2** In `EngineDetail.tsx`, insert 4 nav items after `reliability`, before `advanced`: `Query & Memory`/queryMemory, `Spilling`/spilling, `Execution & Optimizer`/execution, `SQL & Web UI`/sqlWebUi — each rendering its section with `getProp`/`setProp` from `cfg`.
- [ ] **Step 3** Extend `sectionCounts` (Task-4-of-#1 function) to count the new sections: for each new section key, count how many of its `knownProperties` keys are present in `builtTrinoConfig.configProperties`. Badge the nav items.
- [ ] **Step 4** `tsc`/`eslint`/`build`/`vitest`. Commit `feat(ui): Query&Memory/Spilling/Execution/SQL&WebUI tuning sections`.

---

## Task 4: Fold FTE retry attempts into Reliability + log.format into Advanced

**Files:** Modify `EngineConfigReliability.tsx`, `EngineConfigAdvanced.tsx`

- [ ] **Step 1** In `EngineConfigReliability.tsx`, after the fault-tolerant fields, render the two `reliability`-section known properties (`query-retry-attempts`, `task-retry-attempts-per-task`) via `<PropertyField prop value={getProp(key)} onChange={v=>setProp(key,v)} />` (pass `getProp`/`setProp` as props from EngineDetail).
- [ ] **Step 2** In `EngineConfigAdvanced.tsx`, add the `advanced`-section known property (`log.format`) via `PropertyField`. Ensure `sectionCounts` for `reliability`/`advanced` includes these keys.
- [ ] **Step 3** `tsc`/`build`/`vitest`. Commit `feat(ui): FTE retry attempts in Reliability + log.format in Advanced`.

---

## Task 5: Property search

**Files:** Create `config/PropertySearch.tsx`; Modify `EngineDetail.tsx`

- [ ] **Step 1** `PropertySearch({ onJump }: { onJump: (section: string) => void })`: a search `<Input>` (with a search icon); as the user types, filter `knownProperties` by `key`/`label`/`help` (case-insensitive) and show a dropdown/list of up to ~8 matches — each row: label, `key` (mono, muted), and the section label; clicking calls `onJump(prop.section)` + clears.
- [ ] **Step 2** In `EngineDetail.tsx`'s Configuration card header, render `<PropertySearch onJump={setSection} />` (sets the vertical-nav section). (Optional nicety: also stash the jumped-to key so the section can scroll/highlight it — SKIP for now, YAGNI.)
- [ ] **Step 3** `tsc`/`eslint`/`build`. Commit `feat(ui): property search across the tuning catalog`.

---

## Task 6: Gate + Playwright + live + PR

- [ ] **Step 1** `cd frontend`: `npx tsc --noEmit` (0), `npm run lint` (0 errors), `npx vitest run` (green), `npm run build`.
- [ ] **Step 2: Playwright**: Configuration nav shows the 4 new sections; open **Spilling** → toggle `spill-enabled` (Switch); open **Execution & Optimizer** → set `task.concurrency` (number) + `join-distribution-type = BROADCAST` (Select); Save → the `updateEngine` PUT body `trinoConfig.configProperties` contains `{"spill-enabled":"true","task.concurrency":"…","join-distribution-type":"BROADCAST"}`. The **property search** for "spill" lists the spilling props and jumping selects the Spilling section. A field's help icon shows the catalog help. Confirm the raw config.properties editor (General/Advanced) does NOT list these known keys. 0 console errors.
- [ ] **Step 3: Live (kind)**: with those props saved → provision → `kubectl get configmap demo2-config -o yaml` (ANSI-stripped) shows `spill-enabled=true`, `task.concurrency=…`, `join-distribution-type=BROADCAST` in `config-coordinator.properties`.
- [ ] **Step 4** Commit; open PR; run the full frontend gate before an admin squash-merge.

---

## Self-Review notes
- **Spec coverage:** catalog + getProp/setProp + raw-editor filter (T1) ✓; PropertyField widget-by-type + FieldLabel inline help (T2) ✓; 4 new sections + nav + counts (T3) ✓; Reliability/Advanced folds (T4) ✓; property search (T5) ✓; verify incl. live configmap (T6) ✓.
- **Type consistency:** `KnownProperty`/`PropType` ↔ `PropertyField` `prop`; `getProp`/`setProp` operate on `configProps` (`Kv[]`); `knownPropertyKeys` set drives the raw-editor filter + the counts.
- **Payload shape unchanged:** all values live in `trinoConfig.configProperties` (the existing map); no new fields; backend untouched.
- **Excluded** (already egret-owned): `query.max-memory`, log level, `internal-communication.shared-secret` — not in the catalog.
- **Deferred:** the whole-surface catalog beyond ~25 (extensible later); scroll-to-highlight on search jump; security/TLS (#3); observability (#4).
