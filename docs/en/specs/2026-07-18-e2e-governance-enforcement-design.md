# E2E: governance enforcement on kind — Design

**Date:** 2026-07-18 · **Status:** Design — approved · **Branch:** `feat/e2e-governance-enforcement` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-18-e2e-governance-enforcement-design.md`.

## Problem

Project 2 (full automated e2e) slices 1 (data-plane, PR #273) + 2 (auth+control-plane admin, PR #274) are done ([[egret-e2e-dataplane-kind]]). The next slice is **governance enforcement** — the crown-jewel data-plane assertion: a policy (column-mask / row-filter / deny) actually **changes a governed query's output**. Ranger enforcement has been proven by hand (P3.2 tag masking #232, StarRocks #259) but never as an automated e2e.

This is the heaviest slice: it needs a Trino engine bound to Ranger, a governed table with data, policies, and a query — inheriting slice-1's engine-warmup flakiness plus the Ranger policy-poll delay. It is deliberately chosen (facet B, real enforcement) over the deterministic admin-UI CRUD (facet A).

## Goal

One automated Playwright lifecycle spec, run via a Ranger-enabled `just e2e-kind` variant against the in-cluster kind deploy, that provisions a Ranger-bound Trino engine, seeds a governed Iceberg table, creates policies via the egret policy admin API, and asserts a query as the admin returns **masked column values + row-filtered rows** — proving Ranger enforcement end-to-end. Run model unchanged: kind canonical, opt-in `EGRET_E2E_KIND=1`, not in GitHub CI.

## Out of scope

- The deterministic governance **admin-UI CRUD** (facet A: policy/classification management forms) — a possible later slice.
- Two-user enforcement (admin-raw vs member-masked) — this slice targets the **admin's own username** with policies (single-user), avoiding a second session; the two-user variant is a later refinement.
- StarRocks Ranger enforcement, Cedar BI ABAC, tag-based masking — Trino resource-based mask/row-filter/deny only.
- Provisioning-as-test, notebooks, BI/SQL-Lab — separate slices.

## Context — grounded surface (verified)

- **egret IS the Ranger admin.** With `config.ranger.enabled=true`, egret mounts `/service` (`ranger_router`, `crates/egret-server/src/lib.rs:174`) — the Ranger Admin REST surface the Trino plugin polls (`/service/plugins/policies/download/{service}`). **No separate Ranger server is deployed.** `config.ranger.enabled` defaults to **false** (`egret-config`), and the current kind deploy has it off → engines provision **allow-all** (`ranger disabled; ... allow-all` in the logs). `auth_token=""` opens `/service` anonymously for the stock plugin.
- **Ranger-bound engine provisioning.** When `EngineMgmtState.ranger_enabled=true` (from `config.ranger.enabled`, `main.rs:1481`) and no admin-chosen service, engine creation **auto-binds** a fresh Ranger profile/service (`engines.rs:1922`), and (PR #272) Trino is provisioned with `access-control.name=ranger` + a `ranger-trino-security.xml` pointing the plugin at egret's in-cluster `/service`.
- **Policy admin API** (`crates/egret-policy/src/admin`, mounted at `/api/control-plane`, reachable via the ingress `/api`): `GET/POST /policies`, plus service-def / principals / resource-lookup. Policies created here are served to the plugin via `/service`. **`/service` is NOT reachable through the browser ingress** (nginx routes non-`/api` paths to the SPA), so the e2e MUST use `/api/control-plane/policies`, not `/service` directly.
- **Query runs AS the caller** (impersonation, `X-Trino-User` = the logged-in user's username; `execution/routes.rs:124`). So enforcement is per-user — targeting the admin username with mask/row-filter policies makes the admin's own query show the enforcement.
- **The manual recipe** (`docs/en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md`) is the proven reference: default-deny needs global `execute` on `queryid=*` + `impersonate` on `trinouser=*`; then `select` on the table; then a column-`MASK` policy and a `rowFilterInfo.filterExpr` policy. Trino catalog name is `iceberg`; the plugin polls every ~5s.
- **Reuse** slice-1 helpers (`provisionStorageAndWarehouse`, `provisionTrinoEngine`, `waitForEngineReady`, `enableEngineJwt`, `assignEngineToWorkspace`, a query-submit helper, `csrf`) and the `just e2e-kind` runner shape.

## Architecture

One long lifecycle spec + a Ranger-enabled deploy variant + small helpers. Provisioning + policy setup via the egret API (`page.request`); the enforcement assertion is a query through the SQL Lab backend.

### A. Deploy — egret with Ranger on

- **Chart** (`deploy/helm/egret`): add a `ranger.enabled` value (default false) → `configmap-env.yaml` emits `EGRET_RANGER__ENABLED` + `EGRET_RANGER__AUTH_TOKEN=""` when set. (Anonymous `/service` for the stock plugin.)
- **Runner**: a `deploy-kind-governance` recipe (or extend `deploy-kind-oidc`) = `helm upgrade --install ... --set oidc.enabled=true --set ingress.enabled=true --set ranger.enabled=true`, then `wait-ready.sh`. The e2e spec runs under `EGRET_E2E_KIND=1` like the others.

### B. Spec — `frontend/tests/e2e/governance-kind.spec.ts`

Gate: skip unless `EGRET_E2E_KIND=1`; `test.setTimeout(900_000)`; `try/finally` cleanup; login once (shared page, serial — the login rate limit lesson from slice 2). Flow (as admin, via `page.request` unless noted):

1. **Provision** storage → warehouse → Trino engine (Ranger auto-bound because the deploy is ranger-on) → `waitForEngineReady` → `enableEngineJwt` → `waitForEngineReady` → assign workspace. (Slice-1 helpers.)
2. **Seed a governed table** via the SQL Lab query backend (`POST /api/ws/{ws}/queries`, retrying through coordinator warmup): `CREATE SCHEMA` → `CREATE TABLE report(id, item, country, amount)` → `INSERT` a few rows (some `country='US'`, some not). Poll each to `finished`.
   - This requires the default-deny globals to already permit the admin to run DDL/DML → create those policies first (step 3), or seed before enabling Ranger enforcement (TBD in the plan; simplest: create the global execute/impersonate + a broad admin allow policy, wait for the poll, then seed).
3. **Create policies** via `/api/control-plane/policies` (on the auto-bound engine's profile): global `execute` (`queryid=*`) + `impersonate` (`trinouser=*`); table `select` for the admin username; a **column-MASK** on `item` for the admin; a **row-filter** `country='US'` for the admin. Resolve the admin username via `/api/me`. Wait ~8–10s for the plugin poll.
4. **Assert enforcement**: `SELECT id, item, country FROM iceberg.report ORDER BY id` as admin → the result shows only `country='US'` rows (row-filter) AND the `item` column is masked (not the raw values). Compare against the seeded data.
5. **Cleanup** (`finally`): delete the policies, drop the table, delete the engine.

### C. Helpers

Add to `helpers.ts` as needed: a `runQuery(request, wsSlug, dbName, sql)` that submits + polls + returns the result JSON (generalizing slice-1's `runSqlInLab` API path, reused for DDL/DML + the assertion), and `createRangerPolicy(request, csrf, ...)` thin wrappers if the policy DTO is verbose.

### D. Docs

Add a "Governance enforcement e2e" subsection to `frontend/tests/e2e/README.md`: needs the ranger-on deploy (`deploy-kind-governance`), what it proves (Ranger mask + row-filter on a live query), and that the Trino Ranger plugin poll adds ~5–10s.

## Testing / acceptance

Not "wrote a test" — **run it for real** via the ranger-on deploy and confirm the spec PASSES (admin query shows masked `item` + only `US` rows). Unit gates (`npm run lint`/`typecheck`/`test`/`build`) stay green; the spec is opt-in.

## Risks

1. **Heaviest slice** — engine warmup (~40s) + Ranger plugin poll (~5–10s) + policy propagation; expect several runs to converge (like slice 1). Generous timeouts + retries.
2. **Write path (INSERT) through the catalog** — seeding data via the engine exercises the Iceberg REST write path (empty-location guard, reserved properties); may surface issues the read-only slice-1 didn't.
3. **Default-deny ordering** — with Ranger on, the admin needs execute/impersonate/select policies BEFORE any query (even DDL) succeeds. Get the policy-then-seed ordering right (create globals + admin-allow first, poll, then seed).
4. **Policy admin API DTO** — the egret policy DTO differs from the raw Ranger `/service` JSON in the manual recipe; pin the exact `POST /api/control-plane/policies` body (access / mask / row-filter shapes) during writing-plans.
5. **Chart change** — this slice touches the Helm chart (ranger config), unlike the pure-test slices.

## Project 2 remaining after this slice

governance admin-UI CRUD (facet A) · provisioning-as-test · notebooks (Daft) · BI/SQL-Lab consumer UI · two-user + StarRocks + Cedar enforcement variants. Each its own spec → plan.
