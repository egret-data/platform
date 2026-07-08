# E2e — Fix the anonymous Ranger serving-surface (as-built)

> **Status:** Implemented (stacked on E2a–d, branch `feat/e2a-policy-contract-crate`) · **Date:** 2026-07-08 · **Part:** E2e (final) of [E2 decomposition](./2026-07-08-e2-policy-seam-decomposition.md).

## Problem
`egret-policy`'s `ranger/auth.rs` gate is all-or-nothing: when `ranger.auth_token` is empty (the default), the **entire** `/service/*` surface is open — including the Ranger-Admin CRUD **write** routes (`POST /plugins/services`, `DELETE /plugins/services/{id}`, `POST/PUT/DELETE /plugins/policies`). Those writes have **zero callers** (verified: no Rust or frontend code hits `/service/plugins/*`; egret's policy writes go through the authenticated control-plane admin API, and stock engine plugins only GET the download endpoint). So the default exposed unauthenticated policy mutation for no benefit.

## Fix (locked decision: split the gate + drop unused writes)
Keep the read-only surface open-capable (the legitimate stock-plugin need — plugins authenticate out-of-band over a trusted network and poll `/plugins/policies/download/{service}`); **remove the write routes entirely**.

- `ranger/api/crud.rs` is now **read-only**: `GET /plugins/definitions`, `/plugins/definitions/name/{name}`, `/plugins/services`, `/plugins/policies/{id}`. Removed the `create_service`/`delete_service`/`create_policy`/`update_policy`/`delete_policy` handlers + routes (and the now-unused `post`/`PolicyStoreError` imports).
- The `require_bearer` gate is unchanged in mechanism: empty token → open (now exposes **reads only**); set a token → bearer required on everything. Doc comment updated to state the surface is read-only.
- Re-addable behind auth if external Ranger-Admin write compat is ever needed.

## Tests
`tests/http_ranger.rs` rewritten: services/policies are seeded **through the store** (as the admin API does) instead of the removed HTTP write routes; the download / `304` / version-bump / anonymous-download / unknown-404 / bearer-required coverage is unchanged. 4/4 green.

## Verification
fmt + clippy(egret-policy, all-targets) clean; egret-policy suites green (incl. the rewritten `http_ranger`). No frontend change; no other crate references the removed routes.

## Result — E2 (Policy/Ranger seam) COMPLETE
Per-seam DoD met: contract crate `egret-policy-api` (E2a) · merged seam crate `egret-policy` (E2b) · owned `policy` schema (E2c) · admin router out of controlplane behind reader-Ports + type rename (E2d) · anonymous-surface hardened (E2e). Deferred (consistent with E0/E1): OpenAPI emit + FE codegen (E0·P4), `policy.changed` event (first consumer), the two authorizers' unification (Effort B). Known: the `last_admin_guard` test-isolation flake (E1b, now also touching the `policy` schema) — a test-infra follow-up, not seam code.
