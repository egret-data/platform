# E2E: engine provisioning via the UI on kind — Design

**Date:** 2026-07-18 · **Status:** Design — approved · **Branch:** `feat/e2e-provisioning-ui` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-18-e2e-provisioning-ui-design.md`.

## Problem

Project 2 slices 1–3 provision engines through the **egret API** (fast helpers). None exercise the **control-plane engine-provisioning UI** — the "Add engine" form, the live status badge, and the delete flow that a platform admin actually clicks. Slice 4 ("provisioning-as-test") covers that browser path.

This is a hybrid: a deterministic create form + an async wait for the engine to reach `running` (real k8s provisioning). It avoids slice-1's flakiest bits (no JWT toggle, no query warmup) — just provision-to-running — so it's more tractable than the full data-plane.

## Goal

One Playwright spec that drives the control-plane engine-provisioning UI against the in-cluster kind deploy: fill the "Add engine" form for a Trino cluster, watch the engine's status badge reach **running**, then delete it via the UI — proving the provisioning UI works end-to-end. Run model unchanged: kind canonical, opt-in `EGRET_E2E_KIND=1`, not in GitHub CI.

## Out of scope

- StarRocks provisioning (a later variant — adds the operator + CR).
- Storage / warehouse provisioning via the UI (seeded via the API here; their UI could be a later slice).
- Deep engine config editor, JWT enable, workspace assignment, queries (covered by slices 1/3).
- RisingWave (not provisionable yet).

## Context — grounded surface (verified)

- **`ControlPlaneEngines.tsx`** (`/control-plane/engines`, gated `RequirePermission perm="engine:read"`): a Tabs bar per engine type (Trino / StarRocks / RisingWave) sets `activeType`; an **"Add engine"** button (disabled unless the active type is provisionable) opens a **Dialog** create form with `name`, a **warehouse** `Select`, and resource inputs (`coordinatorReplicas`, `workerReplicas`, `cpuMin`, `cpuMax`, `memory` — defaults `cpuMin:"1"`, `cpuMax:"2"`, `memory:"4Gi"`). Submit calls `createEngine` (`POST /engines`). Each list row shows `engine.name`, `engine.namespace`, a type Badge, and a live **`<EngineStatusBadge>`** (per-row status poll) rendering the phase (`running` / `provisioning` / …); a summary strip shows running / not-running counts.
- **Prerequisite:** the create form picks a **warehouse** — one must exist. Seed storage + a warehouse via the API (reuse slice-1 `provisionStorageAndWarehouse`) with the reserved bucket `e2e`.
- **Provision-to-running needs no S3/JWT** — the engine just needs its pods Ready; slice-1 proved a 1Gi/1-worker Trino reaches `running` in ~1–2 min. Use small resources in the form to avoid OOM / long waits.
- **Login is rate-limited** (slice-2 lesson) → log in once (shared page, serial).
- Reuse slice-1 helpers (`csrf`, `provisionStorageAndWarehouse`, `deleteEngine` for `finally` cleanup) + the `just e2e-kind`-style runner (any OIDC deploy; ranger state is irrelevant to reaching `running`).

## Architecture

One spec + API setup for the warehouse; the engine lifecycle is driven through the browser UI.

### A. `frontend/tests/e2e/provisioning-ui-kind.spec.ts`

Gate: skip unless `EGRET_E2E_KIND=1`; `test.setTimeout(600_000)`; login once (shared page, serial); `try/finally` cleanup (delete the engine via the API as a safety net if the UI delete didn't run).

Flow:
1. **Setup (API):** `provisionStorageAndWarehouse` (bucket `e2e`) → a provisioned warehouse the form can pick.
2. **Open the create form:** `goto /control-plane/engines`; ensure the Trino tab is active; click **"Add engine"** → the dialog opens.
3. **Fill + submit:** set `name` = `e2e-ui-${RUN_ID}`; pick the seeded warehouse in the warehouse `Select`; set **small** resources (`workerReplicas`=1, `memory`=`1Gi`); submit. The dialog closes and the engine appears in the list.
4. **Wait for running:** poll the engine row's status badge until it reads **running** (reload/poll, cap ~4 min — real k8s provisioning + Trino startup). Assert the row shows `running`.
5. **Delete via UI:** open the engine row's actions → Delete → confirm; assert the row disappears from the list.
6. **Cleanup (`finally`):** best-effort `deleteEngine` via the API (in case the UI delete step failed), so no engine leaks.

### B. Runner

Runs under the existing opt-in flow — `EGRET_E2E_KIND=1` against a running deploy (e.g. the one `just e2e-kind` brings up). Documented in the README; no new deploy variant needed (uses `EGRET_E2E_BUCKET=e2e`).

### C. Docs

Add an "Engine provisioning via the UI" subsection to `frontend/tests/e2e/README.md`: what it drives (Add-engine form → status `running` → delete), the reserved-bucket note, and that it waits ~1–2 min for the engine to come up.

## Testing / acceptance

Not "wrote a test" — **run it for real** and confirm the spec PASSES (the UI-created Trino reaches `running`, then deletes). Unit gates (`lint`/`typecheck`/`test`/`build`) stay green; the spec is opt-in.

## Risks

1. **Async provisioning wait** — the engine takes ~1–2 min to reach `running`; poll the status badge with a generous cap; small resources (1Gi/1 worker) keep it fast + avoid OOM.
2. **Form selectors** — the warehouse `Select` (a Radix combobox) + resource inputs + submit button; centralize and finalize against the live DOM (like prior slices). The warehouse Select must not be confused with the top-nav workspace switcher (slice-1 lesson).
3. **Status-badge polling** — `<EngineStatusBadge>` polls per row; the spec may need to reload or wait for the badge text to flip to `running`.
4. **Reserved bucket** — needs `EGRET_E2E_BUCKET=e2e` (the `warehouse` bucket carries a lakekeeper orphan); documented.

## Project 2 remaining after this slice

governance admin-UI CRUD (facet A) · notebooks (Daft) · BI/SQL-Lab consumer UI · StarRocks + storage/warehouse-UI provisioning variants. Each its own spec → plan.
