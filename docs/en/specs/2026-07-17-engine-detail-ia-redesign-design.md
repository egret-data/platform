# Engine Detail — IA Redesign (Trino + StarRocks) — Design

**Date:** 2026-07-17 · **Status:** Design — awaiting approval · **Branch:** `feat/engine-detail-ia-redesign` (off `feat/starrocks-deep-config` / PR #266 — depends on its StarRocks config sections)
**Skill:** ui-ux-pro-max (Data-Dense Dashboard). **Scope chosen:** Full IA rework, FE-only. Keeps egret's OKLCH/Metabase tokens + shadcn — polish, not restyle.

> Vietnamese reference: `docs/vi/specs/2026-07-17-engine-detail-ia-redesign-design.md`.

## Problem (review findings)

The engine detail page (`EngineDetail.tsx`), now shared by Trino + StarRocks, reviewed against the Data-Dense Dashboard rules:

1. **Overview is thin.** Just a Status card (pods table, often empty) + Danger zone. A provisioning engine shows almost nothing; there's no at-a-glance summary — you must open Catalogs/Workspaces/Configuration to learn anything. The dashboard pattern wants key indicators up front.
2. **Sparse identity header** — icon + name + namespace + 2 badges; omits endpoint `host:port`, warehouse, image, auth/delegation mode.
3. **No KPI strip** — egret standardized on `SummaryStrip` for CP pages; the engine detail doesn't use it.
4. **Weak empty/loading states** — "No pods yet." with no reason/next step; no skeletons.
5. **Action placement drifts** — Restart lives in the Status card; Save in each config tab; no page-level anchor.
6. **6 flat tabs**, with Configuration + Security both being "settings" that (for both engines) already share ONE form instance + one re-provision Save — yet they're split, and StarRocks' config nav lacks Trino's non-default section badges.
7. **Monitor is Trino-only** — `EngineMonitor` is a rich Trino *query* monitor (cluster info, running queries, stage visualizer, kill) hitting Trino's native API. For StarRocks it renders the same Trino-shaped view against endpoints StarRocks doesn't have → empty/broken.

## Goal

Rework the engine detail IA so both engines read as a coherent ops dashboard:
- **Overview** = an at-a-glance dashboard (KPI strip + enriched header + real states).
- **Settings** = merge Configuration + Security into ONE tab (single left-nav + single Save), since they already share the form instance and both re-provision.
- **Monitor** = engine-aware: keep Trino's query monitor; give StarRocks a **pod/cluster health** view built from the k8s status egret already polls (no new backend). Query-level StarRocks monitoring is a flagged future feature (needs a StarRocks query/session API — out of scope).
- Consistent page-level actions, states, density, and Trino↔StarRocks parity.

**Tabs:** `Overview · Catalogs · Workspaces · Settings · Monitor` (5, down from 6).

## Non-goals

- **No new backend** — the StarRocks Monitor health view uses the existing status poll (pods/phase/replicas/HPA); no StarRocks query API.
- **No restyle** — keep OKLCH/Metabase tokens, shadcn, the existing card language. This is IA + information density + states, not a new visual style.
- **No change to Catalogs/Workspaces tab logic** — only density/state/hover polish.
- **No change to the config/security form logic** — the merge is presentational (same hooks, same Save).

## Background — current state (verified)

- `EngineDetail.tsx` `EngineTabs`: 6 `TabsTrigger`s. Configuration + Security are already gated the same for both engines and both consume ONE form hook instance (`useEngineConfigForm` for Trino, `useStarRocksConfigForm` for StarRocks) with a shared `SaveBar`. Overview = `StatusCard` (pods table + Restart) + `DangerCard`.
- `SummaryStrip` (`components/explore/SummaryStrip`) is the CP-standard KPI row (Access-UX epic).
- `EngineMonitor.tsx` (501 lines) = Trino query monitor via `getCluster`/`listQueries`/`getQuery`; rendered ungated at line 263 for every engine.
- The engine object carries: `type`, `namespace`, `status{phase,pods[],message}`, `spec` (imageVersion, trinoConfig / starrocksConfig incl. delegation/replicas), `warehouseId`, and (via helpers) coordinator host/port. Catalogs + workspaces are separate queries already used by their tabs.

## Architecture — components

### A. Enriched header (`EngineHeaderCard`)
Extend the identity card: keep icon/name/namespace/type+phase badges; add a **metadata row** — image · endpoint `host:port` (mono, `tabular-nums`) · warehouse name · auth/delegation mode (JWT/Impersonate/None; Trino auth for Trino). Move **page-level actions** here: `Restart` (engine-kind-aware, from #265) + an overflow `⋯` menu (Delete → confirm; future actions). Actions stay put across tabs.

### B. Overview = ops dashboard
- **`EngineSummaryStrip`** (reuse `SummaryStrip`): engine-adaptive items — Phase (dot+label) · Pods `ready/total` · replicas (`FE n · CN n` for StarRocks / `Coord n · Worker n` for Trino) · Catalogs count · Workspaces count · Auth mode. `tabular-nums`.
- **`StatusCard`** (keep): pods table with row-hover + Logs/Exec; **real states** — provisioning/empty → skeleton + "Waiting for the operator to create pods…"; error phase → the status `message`.
- **`DangerCard`** moves under an overflow or stays at the bottom (de-emphasized).

### C. Settings tab (merge Configuration + Security)
One tab whose content is the existing config **left-nav** with **Security & Exposure appended as the final nav item(s)** — flattening the former two tabs into one left-nav, one content pane, one `SaveBar`. Trino: `Image · Server · Coordinator · Worker · Workload · Pod · Autoscaling · Security & Exposure`. StarRocks: `Image · Compute · Resources · FE config · CN config · Autoscaling · Pod & Containers · Storage · Security & Delegation`. Reuses both hooks + the shared Save verbatim; adds **non-default section badges to the StarRocks nav** (parity with Trino). Deep-link preserved via `?section=`.

### D. Monitor = engine-aware
- **Trino:** unchanged (`EngineMonitor` query monitor).
- **StarRocks:** a **health** view built from the existing `status` poll + spec — pod cards (phase, ready, restarts, age), FE/CN replica summary, HPA target (if `cnAutoscaling`), and a link to per-pod Logs/Exec. A muted note: "Query-level monitoring for StarRocks is coming soon." No new backend.
- `EngineDetail` branches Monitor content by `engine.type`.

### E. Density + states pass (both engines, all tabs)
Tighter card padding on ops surfaces (density 8), `tabular-nums` on pod/metric/number columns, row-hover (150–300ms) + `cursor-pointer` on interactive rows, consistent 8px rhythm, loading skeletons on the async tabs (status, catalogs, workspaces, monitor). Respect `prefers-reduced-motion`. Contrast ≥4.5:1 (both themes).

## Data flow

No new data. The header/SummaryStrip read the already-fetched `engine` + `status` + (Overview) the catalogs/workspaces counts (small existing queries). Settings reuses the config/security hooks + Save (PUT `/engines/{id}` → re-provision, from #265). StarRocks Monitor reads `engine.status`.

## Testing strategy

- **Unit (Vitest):** `EngineDetail` renders 5 tabs; the SummaryStrip shows engine-adaptive KPIs (FE/CN vs Coord/Worker); Settings renders the merged left-nav incl. Security as the last item with one SaveBar; the StarRocks Monitor shows the health view (not the Trino query monitor); provisioning empty-state copy renders; Restart/Delete live in the header. Trino path unchanged (query monitor still renders under Monitor).
- **No new backend tests** (FE-only).
- **Live (browser):** a Trino engine + a StarRocks engine — verify the header metadata, the Overview KPI strip, the merged Settings nav (config + security, one Save), the StarRocks Monitor health view, and that Trino's query monitor still works. Screenshots at 1440 / 1024 / 768.

## Risks

1. **Merging Config+Security into one left-nav** must not break the shared-Save/dirty/validity wiring (both engines) or the `?section=` deep-links — presentational refactor only; keep the hooks + SaveBar instances intact.
2. **StarRocks Monitor health view** must degrade gracefully when `status.pods` is empty (provisioning) — reuse the Overview empty-state.
3. **Depends on #266** (deep-config StarRocks sections) — branch off it; if #266 changes, rebase.

## Work breakdown (for the plan)

1. **Enriched header** — metadata row + page-level Restart/⋯(Delete); move actions out of StatusCard/DangerCard.
2. **Overview SummaryStrip** — engine-adaptive KPI strip + Overview layout.
3. **Empty/loading states + density pass** — skeletons, provisioning copy, tabular-nums, row-hover (both engines, shared).
4. **Settings tab merge** — fold Security into the config left-nav (both engines), one SaveBar, `?section=` intact; StarRocks nav badges.
5. **Monitor engine-aware** — StarRocks health view from status; Trino unchanged; branch by type.
6. **Live smoke + screenshots** (Trino + StarRocks).

## Self-review

- **Scope focused:** IA (5 tabs, Settings merge, engine-aware Monitor) + Overview dashboard + states/density; no restyle, no new backend, form logic untouched. ✔
- **No placeholders:** concrete components (`SummaryStrip`, `StatusCard`, `EngineMonitor`), the merge mechanism (fold Security into the config nav), and the Monitor split are named. The one honest limitation (StarRocks query monitor = future) is explicit. ✔
- **Consistency:** both engines share the exact shell; only labels/sections/monitor-content differ. ✔
- **Dependency called out:** off #266. ✔
