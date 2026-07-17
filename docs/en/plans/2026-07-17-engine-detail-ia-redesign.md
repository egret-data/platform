# Engine Detail IA Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Follow superpowers:test-driven-development per task.

**Goal:** Rework the engine detail page (Trino + StarRocks) into a coherent ops dashboard — 5 tabs (Overview · Catalogs · Workspaces · Settings · Monitor), an at-a-glance Overview, a merged Settings tab (Configuration + Security), an engine-aware Monitor, plus states/density polish. FE-only, egret's OKLCH/Metabase tokens kept.

**Architecture:** All in `frontend/src/routes/control-plane/EngineDetail.tsx` + small new sub-components. Reuse `SummaryStrip`, the existing per-engine config/security components (`ConfigurationTab`/`SecurityTab`/`StarRocksConfigTab`/`StarRocksSecurityTab`), and the shared form hooks + `SaveBar`. No backend, no form-logic change.

**Tech Stack:** React 18 + TS + Vite + Vitest + Tailwind v4 + shadcn.

**Spec:** `docs/en/specs/2026-07-17-engine-detail-ia-redesign-design.md`. **Branch:** `feat/engine-detail-ia-redesign` (off `main`, which now has #266).

**Reference anchors** (`frontend/src/routes/control-plane/EngineDetail.tsx`): `EngineTabs`@186 (6 TabsTriggers @225–230, TabsContents @233–263), `EngineMonitor` render @263, `StatusCard`@344, `NAV_ITEMS`@525 (Trino config sections), `ConfigurationTab`@535, `SecurityTab`@685, `DangerCard`@1212. StarRocks config/security live in `StarRocksConfigTab.tsx` + `StarRocksSecurityTab.tsx` (branched in the Configuration/Security TabsContent by `isStarRocks`). KPI: `components/explore/SummaryStrip`. Engine host/port: the engine object + its helpers (grep `coordinator` / `endpoint` in `controlPlane.ts`).

---

## Task 1: Enriched header + page-level actions

**Files:** Modify `EngineDetail.tsx` (the header Card in the default export + `EngineTabs`); move Restart out of `StatusCard`, Delete out of `DangerCard`.

- [ ] **Step 1: Failing test** (`__tests__/EngineDetail.test.tsx`): the header renders a metadata row with the image version + endpoint host:port + auth/delegation mode; the **Restart** button and a **Delete** action (via an overflow menu) are in the header (not only inside the Status/Danger cards). Mock the engine query. Run → FAIL.

- [ ] **Step 2: Build `EngineHeaderMeta`** — a subcomponent under the name/namespace showing: `imageVersion` · `host:port` (mono, `tabular-nums`) · warehouse name (if resolvable from the engine) · auth/delegation mode (Trino: from `spec.trinoConfig.auth`; StarRocks: from `spec.starrocksConfig.delegation.mode`, else "None"). Derive host/port from the engine (grep how EngineMonitor/`getCluster` or the connection resolves it; if only namespace+name is available, render `{name}-fe:9030` / `{name}-coordinator:8080` by kind — reuse any existing helper).

- [ ] **Step 3: Page-level actions in the header** — add a `Restart` button (kind-aware, reusing the existing `restartEngine` mutation currently in `StatusCard`) + an overflow `⋯` (shadcn `DropdownMenu`) with **Delete** (opens the existing `ConfirmDeleteDialog` from `DangerCard`). Lift the restart + delete mutations to the header scope (or a small `EngineActions` component). Keep `restartDisabled=false` for StarRocks (backend supports it since #265).

- [ ] **Step 4: Slim `StatusCard`/`DangerCard`** — remove the in-card Restart button (now in header); `DangerCard` either removed (Delete moved to the header ⋯) or kept as a de-emphasized Overview footer — pick one and keep the delete flow working. Prefer removing the Restart from StatusCard + moving Delete to the header ⋯, dropping DangerCard.

- [ ] **Step 5: Run → PASS.** `cd frontend`: `npx vitest run src/routes/control-plane/__tests__/EngineDetail.test.tsx`; `npm run typecheck` (known pre-existing error only); `npx eslint EngineDetail.tsx` clean.

- [ ] **Step 6: Commit.** `feat(fe): enriched engine header + page-level Restart/Delete`.

---

## Task 2: Overview SummaryStrip (engine-adaptive)

**Files:** Modify `EngineDetail.tsx` (the `overview` TabsContent); reuse `components/explore/SummaryStrip`.

- [ ] **Step 1: Failing test**: for a StarRocks engine the Overview shows a SummaryStrip with items Phase, Pods (`ready/total`), `FE n · CN n`, Catalogs count, Workspaces count, Auth mode; for a Trino engine the replica item reads `Coordinator n · Worker n`. Run → FAIL.

- [ ] **Step 2: Build `EngineSummaryStrip({ engine })`** using `SummaryStrip` (read its props first — `items: {label, value}[]`). Items:
  - **Phase** — `status.phase` (with a status dot; reuse `phaseVariant`).
  - **Pods** — `${ready}/${total}` from `status.pods` (`ready` = pods with `ready===true`).
  - **Replicas** — StarRocks: `FE ${feReplicas} · CN ${cnReplicas}` (from `spec.starrocksConfig`); Trino: `Coord ${coordinatorReplicas} · Worker ${workerReplicas}` (from `spec`).
  - **Catalogs** / **Workspaces** — counts from the existing `listEngineCatalogs` / `list engine workspaces` queries (the Catalogs/Workspaces tabs already fetch these — lift the queries or add lightweight count queries; enable them on Overview).
  - **Auth** — StarRocks delegation mode / Trino auth (JWT/password/none).
  Use `tabular-nums` for numeric values.

- [ ] **Step 3: Place it** at the top of the `overview` TabsContent, above `StatusCard`.

- [ ] **Step 4: Run → PASS.** vitest + typecheck + eslint as Task 1.

- [ ] **Step 5: Commit.** `feat(fe): engine Overview KPI SummaryStrip (engine-adaptive)`.

---

## Task 3: States + density pass (shared, both engines)

**Files:** Modify `EngineDetail.tsx` (StatusCard, catalogs/workspaces cards) + reuse `ListSkeleton`.

- [ ] **Step 1: Failing test**: a provisioning engine with empty `status.pods` shows "Waiting for the operator to create pods…" (not just "No pods yet."); a loading engine shows a skeleton. Run → FAIL.

- [ ] **Step 2: Real states in `StatusCard`** — when `status.pods` is empty: if phase is provisioning/pending → skeleton + "Waiting for the operator to create pods…"; if phase is failed/error → the `status.message`. Add row-hover (`hover:bg-muted/50`, 150–300ms transition) + `tabular-nums` on the pods table; `cursor-pointer` where rows are interactive.

- [ ] **Step 3: Loading skeletons** — ensure Catalogs/Workspaces/Overview use `ListSkeleton` while their queries load (grep existing skeleton usage; add where missing). Respect `prefers-reduced-motion` (Tailwind `motion-reduce:` or the existing convention).

- [ ] **Step 4: Density** — tighten card padding on the ops surfaces to the density-8 rhythm (8px scale) without breaking the shared card component; keep contrast ≥4.5:1 in both themes.

- [ ] **Step 5: Run → PASS.** vitest + typecheck + eslint.

- [ ] **Step 6: Commit.** `feat(fe): engine detail real states + density polish`.

---

## Task 4: Settings tab — merge Configuration + Security

**Files:** Modify `EngineDetail.tsx` (`EngineTabs` tab list + content; the config left-nav in `ConfigurationTab` + the StarRocks tab).

- [ ] **Step 1: Failing test**: `EngineTabs` renders exactly 5 tabs (Overview/Catalogs/Workspaces/**Settings**/Monitor — no separate Configuration/Security). The Settings tab's left-nav ends with a **Security & Exposure** item; selecting it shows the security controls; there is ONE `SaveBar` covering both config + security edits. For StarRocks the config nav items carry non-default badges. Run → FAIL.

- [ ] **Step 2: Replace the Configuration + Security TabsTriggers** (@228–229) with a single `<TabsTrigger value="settings">Settings</TabsTrigger>`; replace the two TabsContents (@246–261) with one `settings` TabsContent.

- [ ] **Step 3: Fold Security into the config left-nav.** In `ConfigurationTab` (Trino) add a `{ key: "security", label: "Security & Exposure" }` entry to `NAV_ITEMS`; when `section === "security"` render the `SecurityTab` body (extract `SecurityTab`'s inner content into a section component so it renders inside the config pane, sharing the same `cfg` + one `SaveBar`). Do the same in `StarRocksConfigTab` — append a `Security & Delegation` nav item rendering `StarRocksSecurityTab`'s body, sharing `srCfg`. Both already use ONE hook instance per engine, so one `SaveBar` naturally covers all sections. Preserve the `?section=` deep-link (Security is just another section value).

- [ ] **Step 4: StarRocks nav non-default badges** — mirror Trino's `sectionCounts` badge on the StarRocks nav (count non-default fields per section: FE config keys, CN config keys, autoscaling on, pod fields set). Reuse the Trino badge pattern.

- [ ] **Step 5: Run → PASS.** `npx vitest run src/routes/control-plane`; typecheck (known error only); eslint. Verify the shared-Save/dirty/validity still works (the `disabled={autoscalingInvalid || podRawInvalid}` gate from #266 must still apply).

- [ ] **Step 6: Commit.** `feat(fe): merge Configuration + Security into a Settings tab`.

---

## Task 5: Engine-aware Monitor

**Files:** Modify `EngineDetail.tsx` (`monitor` TabsContent); new `StarRocksMonitor.tsx` (or an inline health view).

- [ ] **Step 1: Failing test**: for a StarRocks engine the Monitor tab renders a **health view** (pod cards with phase/ready/restarts + FE/CN replica summary + a "Query-level monitoring for StarRocks is coming soon" note) and does NOT call the Trino `getCluster`/`listQueries`. For a Trino engine the Monitor still renders the existing `EngineMonitor` query monitor. Run → FAIL.

- [ ] **Step 2: Branch the Monitor content** by `engine.type`: `engine.type === "trino" ? <EngineMonitor engineId={id}/> : <StarRocksMonitor engine={engine}/>`.

- [ ] **Step 3: `StarRocksMonitor({ engine })`** — a pod/cluster health view from `engine.status` (already polled): pod cards (name, phase badge, ready, restarts, age), FE/CN replica summary, HPA target if `spec.starrocksConfig.cnAutoscaling` is set, per-pod Logs/Exec links (reuse the StatusCard's `PodLogsDialog`/`PodTerminalDialog`), a provisioning empty-state (reuse Task 3's copy), and a muted footer note about query-level monitoring being a future feature. NO new API calls.

- [ ] **Step 4: Run → PASS.** vitest + typecheck + eslint.

- [ ] **Step 5: Commit.** `feat(fe): engine-aware Monitor (StarRocks health view)`.

---

## Task 6: Full FE CI + live smoke (Trino + StarRocks)

- [ ] **Step 1: FE CI.** `cd frontend`: `npm run lint` (0 errors; the 3 pre-existing non-epic warnings allowed), `npm run typecheck` (known `useEngineConfigForm.dirty.test.tsx` error only), `npx vitest run src/routes/control-plane` (all green), `npm run build` (blocked only by the known pre-existing error).

- [ ] **Step 2: Live (browser, both engines).** Provision (or reuse) a StarRocks engine and — if available — a Trino engine. Verify:
  - Header metadata row (image/endpoint/auth) + Restart + ⋯Delete.
  - Overview KPI SummaryStrip (engine-adaptive labels) + provisioning empty-state.
  - Settings tab: one left-nav with config sections + Security as the last item, one Save; StarRocks nav badges.
  - Monitor: StarRocks health view; Trino query monitor still works.
  - Screenshots at 1440 / 1024 / 768; check dark mode contrast.

- [ ] **Step 3: Report** exactly what was verified live vs. only unit-tested (e.g. if no Trino engine is available, say the Trino monitor path is unit-tested only).

---

## Self-Review

- **Spec coverage:** header → T1; Overview KPI → T2; states/density → T3; Settings merge → T4; engine-aware Monitor → T5; CI+live → T6. Covered.
- **Type consistency:** `EngineSummaryStrip`/`EngineHeaderMeta`/`StarRocksMonitor`/`EngineActions` are the new components; the config-nav `NAV_ITEMS` gains a `security` entry (Trino) + the StarRocks nav gains `Security & Delegation`; both reuse the existing `cfg`/`srCfg` hooks + `SaveBar`. No backend types touched.
- **No placeholders:** concrete anchors + reused components named; the one honest limitation (StarRocks query monitor = future) is in T5's note.
- **Ambiguity:** 5 tabs; Security folded as a nav section (not a sub-tab); Monitor branches by `engine.type`; density = 8px rhythm keeping the shared card component.
