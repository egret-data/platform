# EngineDetail Configuration IA re-architecture (2026-07-04)

VI reference: `docs/vi/specs/2026-07-04-engine-config-ia-design.md`.

**Goal:** Reorganize the engine detail page so configuration is no longer one
overwhelming "Cluster" scroll, and split the 1885-line `EngineDetail.tsx` into
focused components. **Frontend only — no backend/API change** (same `updateEngine`
PUT with the same `trinoConfig` payload).

**Problem:** Today the top level is just `Overview | Monitor`; Overview holds a
`Configuration` card whose `Cluster` sub-tab (~384 lines of JSX) crams eleven
unrelated groups (query max memory, log level, raw config properties, env vars,
fault-tolerant execution, event listener, node/log properties, resource groups,
session properties, exposure & auth). It is an exhausting scroll and the file is
too large to reason about.

**Non-goals:** backend changes; new config fields (1.5.3/1.5.4/1.5.5 come later);
Monitor tab changes; changing the save semantics (still one PUT of the whole config).

## Target IA

Top-level tabs: **Overview · Configuration · Security & Exposure · Monitor**.

- **Overview** — Status/pods table, Catalogs, Danger zone (Configuration card removed
  from here).
- **Configuration** — a sub-tab strip: **General · Coordinator · Worker · Workload ·
  Reliability · Advanced**.
  - **General**: query max memory, log level, raw `config.properties` (KV), env vars.
  - **Coordinator**: replicas, resources (req/limit), heap (fixed/percent), probes,
    additional JVM config, heap-headroom.
  - **Worker**: the same role fields + graceful shutdown.
  - **Workload**: resource groups + session properties (reuse `EngineResourceGroups.tsx`).
  - **Reliability**: fault-tolerant execution (toggle + retry policy + exchange storage
    conn + prefix).
  - **Advanced**: event-listener, additional node properties, additional log properties
    (raw escape hatches).
- **Security & Exposure** — password users, JWT, ingress (moved out of Cluster).
- **Monitor** — unchanged (Phase 1.6b).

## Component architecture

Extract the config editor out of the monolith:

- **`useEngineConfigForm(engine)`** (new hook, `frontend/src/routes/control-plane/useEngineConfigForm.ts`)
  — owns ALL config form state, prefill-from-spec (including the re-sync on the status
  poll), the merged `trinoConfig` payload builder, and the `updateEngine` save mutation
  (dirty flag + toast). Returns `{ state, setters, dirty, save, saving }`. EngineDetail
  and every sub-tab read/write through this one hook instance (created in EngineDetail,
  passed down by props) so the config is a single shared source of truth across the
  Configuration and Security tabs.
- **Sub-tab components** (each a focused file, props = the relevant slice of the hook):
  - `EngineConfigGeneral.tsx`
  - `EngineRoleConfig.tsx` — shared by Coordinator and Worker, parametrized by
    `role: "coordinator" | "worker"` (worker additionally renders graceful shutdown).
  - `EngineConfigReliability.tsx`
  - `EngineConfigAdvanced.tsx`
  - `EngineSecurity.tsx` — the Exposure & auth card.
  - Workload reuses the existing `EngineResourceGroups.tsx` (its `ResourceGroupsSection`
    + `SessionPropertiesSection`).
- **`EngineDetail.tsx`** becomes a thin orchestrator: the top-level `Tabs`, the shared
  `useEngineConfigForm` instance, Overview (status/catalogs/danger), and the routing of
  state slices into the sub-tabs. Target: well under half its current size.

Shared KV helpers (`KeyValueEditor`, `recordToKv`, `kvToRecord`) already live in
`EngineResourceGroups.tsx`; keep them there (or move to a small `configShared.ts` if a
cleaner import graph results) and import where needed.

## Save

One shared config state; a **Save changes** button rendered on BOTH the **Configuration**
tab and the **Security & Exposure** tab. Both call the same `save()` from the hook, which
PUTs the full merged `trinoConfig` (auth/ingress included) exactly as today. The button is
disabled when `!dirty` or while `saving`. (No sticky global bar — a per-tab button keeps
it simple and visible where the user is editing.)

## Behavior preserved

- Prefill from `engine.spec.trinoConfig` for every field, including the re-sync that runs
  on the pod-status poll (must not clobber in-progress edits — keep the existing guard).
- The exact same PUT payload shape (field-for-field) — verified by diffing a before/after
  save of an unchanged form.
- Saving still rolls the engine (backend behavior unchanged).

## Test

- `tsc` 0, `eslint` clean, `vitest` green, `vite build` ok.
- **Playwright**: the 4 top-level tabs render; the 6 Configuration sub-tabs render; a
  field (e.g. worker replicas) prefills from the current engine; changing it + Save issues
  the `updateEngine` PUT (200) and the engine rolls; the Security tab shows the existing
  password users / ingress config and its Save works too.
- Sanity: the produced `trinoConfig` payload for an unmodified engine is byte-identical to
  the pre-refactor payload (no accidental field drops).

## Compat
- No migration, no API change, no route change. Pure frontend restructure.
