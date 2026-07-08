# E2d — Move the Policies admin router into the seam + type rename (as-built)

> **Status:** Implemented (stacked on E2a–c, branch `feat/e2a-policy-contract-crate`) · **Date:** 2026-07-08 · **Part:** E2d of [E2 decomposition](./2026-07-08-e2-policy-seam-decomposition.md) · **Precedent:** E1c (engine domain leaves control-plane).

The final structural part: the admin Policies console leaves `egret-controlplane` for the seam, and the seam's public types get their correct `policy` names. Two commits.

## E2d(1/2) — type rename (commit `2c9d290`)
`AuthzStore → PolicyStore`, `AuthzError → PolicyStoreError` (distinct from the contract crate's `PolicyError`). Workspace-wide identifier rename, no logic/SQL change. `Ranger*` names kept — `ranger_router`/`RangerState`/`RangerError` accurately name the Apache-Ranger-compat protocol surface, not a mismatch.

## E2d(2/2) — module move + reader-Port
### egret-policy gains the admin module
- `git mv controlplane/src/policies/{mod.rs,ranger_json.rs} → egret-policy/src/admin/`.
- New `admin/reader.rs`: **`EngineBindingReader`** Port (+ `EngineBinding {id, name, status, authz_service_id}`, `EngineReadError`) — the narrow cross-seam read into the engine seam (which engines bind to which profile + their catalog names). Design §3.2: read another seam only through a Port, never its tables.
- New `admin/error.rs`: **`PolicyHttpError`** mirroring the `CpError` arms these routes produced (status + `{"error": msg}` body identical).
- **`PolicyAdminState`** `{ store: PolicyStore (in-seam), engines: Arc<dyn EngineBindingReader>, users, groups, sessions, engine_egret_url }` + `policy_admin_router()`.
- Handlers rewired: `st.authz → st.store`; `st.engine_mgmt.list_engines()/list_engine_catalogs() → st.engines.list_engine_bindings()/engine_catalog_names()`; `st.users/st.groups` read directly (core hub, allowed). `CpError → PolicyHttpError`, `require_permission` now from `egret-auth`.
- egret-policy now depends on **egret-auth** (Auth extractor + `require_permission` + identity repos). No cycle (auth doesn't depend on policy).

### Consumers
- **egret-controlplane** shrinks: `policies` module gone; `CpState` drops `authz`/`engine_mgmt`/`engine_egret_url` (keeps `users`/`groups`/`roles` for the `access` console); `error.rs` drops the now-dead `From<PolicyStoreError>` + `From<EngineMgmtError>`; **`egret-policy` + `egret-engine-mgmt` dropped from its deps**.
- **egret-bin** implements `EngineBindingReader` over `EngineMgmtStore` and builds `PolicyAdminState` (composition root — the only place the two seams meet).
- **egret-server** gains `AppState.policy_admin` and mounts `policy_admin_router()` at **`/api/control-plane`** with the CSRF layer — same prefix as before, so **FE URLs are unchanged** (`frontend/src/api/policies.ts` untouched).

### Tests
- 11 policy HTTP tests moved `controlplane/tests/routes_api.rs → egret-policy/tests/admin_routes.rs`, rebuilt against `policy_admin_router` + a **mock `EngineBindingReader`**. The engine→profile auto-bind round-trip stays covered by egret-engine-mgmt's own test (avoids an engine-mgmt dev-dep on egret-policy); the policy side (`list_profiles` renders bound engines) is covered here via the mock.
- Updated the `catalog_drift.rs` permission-enforcement scanner: `policy:*` keys now live in `../egret-policy/src/admin/mod.rs` (removed `policies/mod.rs`, added the seam file) — same pattern it already uses for the `engine:*` (engine-mgmt) and `workspace:*` (egret-workspace) seams.

## Verification
fmt + clippy(workspace, all-targets) clean; egret-policy (unit + folded ranger + new `admin_routes` 10 HTTP tests) green; controlplane (incl. the fixed drift scan) + bin + server green — the only red is the pre-existing E1b `last_admin_guard` isolation flake (now also touching the `policy` schema; not a seam-code regression). No frontend change.

## Result
**E2 (Policy/Ranger seam) fully extracted** — owns its DTOs (E2a contract) + merged crate (E2b) + schema (E2c) + HTTP/admin surface behind Ports (E2d). Only E2e (anon serving-surface fix) remains.
