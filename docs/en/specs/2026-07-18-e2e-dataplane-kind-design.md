# E2E: data-plane money-path on kind — Design

**Date:** 2026-07-18 · **Status:** Design — approved · **Branch:** `feat/e2e-dataplane-kind` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-18-e2e-dataplane-kind-design.md`.

## Problem

egret now runs **in-cluster on kind** (deploy Project 1, PRs #270/#271/#272 merged) and the data-plane money-path — OIDC → engine JWKS → JWT → TLS:8443 → **impersonation** (`SELECT current_user` = caller) → query — has been **proven manually** via Playwright MCP driving. It is **not yet an automated test**. The existing e2e harness (`frontend/tests/e2e/`) targets the **host dev stack** (Vite :5173 + host `serve`), not the in-cluster deploy, and covers only notebooks + embedded-catalog. This is the first slice of **Project 2** (full automated e2e), which is decomposed by area: auth / control-plane / provisioning / data-plane / governance / notebooks.

## Goal

An automated Playwright spec that drives the **full data-plane lifecycle against the in-cluster kind deploy** and asserts impersonation works end-to-end, plus a `just e2e-kind` runner. This locks in — as a repeatable regression gate — the exact chain we had to prove by hand.

Run model (decided): **kind is the canonical target, local opt-in.** Full CI (`ci.yml`) stays unit-only (GitHub CI is chronically infra-red); e2e is a separate local gate run against a developer's kind cluster.

## Out of scope

- The other 5 Project-2 areas (auth, control-plane, provisioning-as-test, governance, notebooks) — each is its own later slice.
- kind-in-GitHub-CI (heavy; conflicts with the always-infra-red CI reality).
- StarRocks data-plane e2e (Trino first; StarRocks is a follow-on).
- No app/backend code change — this slice is test harness + runner + docs only.

## Context — what already exists (verified)

- **Harness:** `frontend/tests/e2e/` — Playwright, `baseURL` env-driven (`EGRET_E2E_BASE_URL`, default `http://localhost:5173`), `workers:1`, not parallel, timeout 90s. Specs gate on env flags (`EGRET_E2E_CATALOG=1`).
- **Login helpers** (`helpers.ts`): `loginViaOidc` (Keycloak SSO) and `loginLocalAdmin` (break-glass local admin, handles the toggle). The kind deploy seeds `admin@egret.local` / `Admin12345!` via `egret init`, so `loginLocalAdmin` works against kind even with OIDC on.
- **API-in-test pattern:** `embedded-catalog.spec.ts` already provisions via `page.request` (shares the logged-in session cookie) with a local `csrf(context)` double-submit helper. This slice reuses and generalizes that pattern.
- **Provisioning endpoints** (grounded in `crates/egret-engine-mgmt/src/engines.rs`): `POST /engines`, `PUT /engines/{id}` (the JWT block is resolved on create+update by `resolve_auth`), `DELETE /engines/{id}`, `POST /engines/{id}/restart`, `PUT /engines/{id}/workspaces` (set full assignment set) or `POST /api/admin/workspaces/{id}/engines`. Storage + warehouse (catalog) live in the control-plane routers (exact DTOs pinned during writing-plans).
- **Deploy recipes** (`justfile`): `deploy-kind`, `k8s-up`, `kind-undeploy`, `ingress-install`, `coredns-rewrite`. Deploy lives at `http://egret.localtest.me`.
- **Hard-won lessons (from the manual proof, to design around):** single kind node runs ~93% memory → size the test engine at 1Gi / workers=1 and clean up after; the JWT toggle+save on the UI has a same-call race (do it as sequential API calls, not a UI toggle); the SQL Lab Monaco editor rejects direct click → set query via `monaco.editor.getModels()[0].setValue(...)`; a fresh non-Ranger Trino now writes+mounts `access-control.name=allow-all` itself (PR #272), so impersonation works out of the box with ranger off.

## Architecture

Hybrid: **provision via the egret API, assert via the browser.** One long-running spec drives the whole lifecycle; a `just e2e-kind` recipe brings the deploy to Ready and runs it.

### A. Spec — `frontend/tests/e2e/dataplane-kind.spec.ts`

Gate: skip unless `EGRET_E2E_KIND=1`. `test.setTimeout(600_000)` (engine provisioning takes minutes). One test, full lifecycle:

1. **Login** `loginLocalAdmin(page)`; capture `page.request` + `csrf(context)`.
2. **Provision via API** (idempotent, resource names carry a per-run suffix from `EGRET_E2E_RUN_ID` or a timestamp so repeated runs don't collide):
   - Storage connection → in-cluster MinIO (`http://minio:9000`, bucket `warehouse`, key `egret` / `egretsecret`).
   - Warehouse (catalog) on that connection.
   - `POST /engines` Trino: **workers=1, memory 1Gi**, wired to that catalog, **Ranger off** (→ allow-all per PR #272 → impersonation permitted).
3. **Poll to Ready:** `waitForEngineReady(request, id)` — `GET /engines/{id}` until the coordinator reports Ready (~5s poll, ~4 min cap).
4. **Enable JWT:** `PUT /engines/{id}` with the jwt block → poll to Ready again (coordinator re-provisions). Sequential API calls avoid the UI toggle+save race.
5. **Assign workspace:** `PUT /engines/{id}/workspaces` assigning the seeded workspace (`default` from `egret init`, or `demo`).
6. **Assert via browser:** open `/explore/{ws}/sql-lab`, set Monaco to `SELECT current_user` via `setValue`, pick the engine in the Database dropdown, Run, and assert the results panel contains the caller (`admin`) — **not** `egret_delegator` — proving impersonation.
7. **Cleanup (`afterAll`):** `DELETE /engines/{id}` + delete warehouse/storage to free node memory. Idempotent; swallow already-deleted errors.

**Shared helpers** added to `helpers.ts`: `csrf(context)`, `provisionTrinoEngine(request, opts)`, `waitForEngineReady(request, id)`, `runSqlInLab(page, ws, sql)`, `deleteEngine(request, id)`. `embedded-catalog.spec.ts` is updated to import `csrf` from `helpers.ts` (DRY) rather than defining it locally.

### B. Runner — `just e2e-kind`

```
# Opt-in data-plane e2e against the in-cluster kind deploy (egret.localtest.me).
# One-time prereq: just k8s-up && just ingress-install && just coredns-rewrite
e2e-kind: deploy-kind-oidc
    ./deploy/k8s/wait-ready.sh
    cd frontend && EGRET_E2E_KIND=1 \
      EGRET_E2E_BASE_URL=http://egret.localtest.me \
      npx playwright test dataplane-kind
```

- **`deploy-kind-oidc`** — a sibling of `deploy-kind` that runs `helm upgrade --install ... --set oidc.enabled=true --set ingress.enabled=true` (the money-path needs JWT/OIDC = Milestone B). The plain `deploy-kind` (OIDC off) is unchanged.
- **`deploy/k8s/wait-ready.sh`** (new) — `kubectl wait --for=condition=Ready` on the egret + keycloak pods, then poll `curl -sf http://egret.localtest.me/api/auth/config` until 200 (OIDC discovery converges past the Keycloak-not-ready 503 race). This replaces manual pod-watching.
- **Idempotent by design:** `helm upgrade --install` re-runs harmlessly; the spec cleans up its engine; per-run resource names prevent collisions → `just e2e-kind` is repeatable on the same cluster.
- **Does NOT create the cluster** — `e2e-kind` requires a pre-existing kind `egret` cluster + ingress + coredns-rewrite (avoids a ~14-min recreate and losing state); it fails early with clear guidance if the cluster is missing.

### C. Docs

- `frontend/tests/e2e/README.md` — add a "Data-plane e2e on kind" section: one-time prereq, `just e2e-kind`, env overrides, what it proves.
- This spec (EN canonical + VI reference).

## Testing / acceptance

Not "wrote a test" — **run it for real.** After implementation, run `just e2e-kind` against the existing kind cluster and confirm the spec PASSES (`SELECT current_user` = `admin`), capturing the Playwright report. Unit gates (`cargo fmt/clippy/test`, frontend `lint`/`typecheck`) stay green; the new spec is opt-in and does not run in `npm test`.

## Risks

1. **Node memory ~93%** (highest risk) — engine at 1Gi + mandatory `afterAll` cleanup; README documents deleting stale engines if the node is still wedged.
2. **Slow provisioning / timeout** — sane poll caps (~4 min per Ready wait), `test.setTimeout(600s)`.
3. **Monaco / dropdown selectors drift with UI changes** — centralize in `runSqlInLab` so there is one place to fix; prefer durable role/text selectors.
4. **OIDC discovery race** — `wait-ready.sh` polls `/api/auth/config` for 200 before the test starts.

## Project 2 remaining (separate slices)

After this slice: auth/login · control-plane admin · provisioning-as-test (the full UI provisioning click-through) · governance (policies/masking/row-filter/Cedar) · notebooks (already partly covered). Each gets its own spec → plan → implementation. kind-in-GitHub-CI and StarRocks data-plane e2e are deferred.
