# StarRocks Engine Config Parity — Design

**Date:** 2026-07-16 · **Status:** Design — awaiting approval · **Branch:** `feat/starrocks-config-parity` (off `main`)
**Program:** StarRocks-parity-with-Trino follow-on. The three delegation/provisioning slices (#259/#261/#263) shipped *creating* a StarRocks engine; this brings the post-create **management surface** (Configuration + Security tabs, edit + restart) to parity with Trino's engine-detail page.

> Vietnamese reference: `docs/vi/specs/2026-07-16-starrocks-config-parity-design.md`.

## Problem

After a StarRocks engine is created, its detail page (`EngineDetail.tsx`) shows only Overview / Catalogs / Workspaces / Monitor. The **Configuration** and **Security & Exposure** tabs are hidden (`!isStarRocks` guards), and Restart is disabled, because:
- The backend `update_engine` (`engines.rs:2604`) and `restart_engine` (`:2729`) **fail-fast with 400** for non-Trino engines — `build_provision_req` only assembles a Trino `EngineSpec`, and StarRocks restart was a no-op stub.
- The Trino config form (`useEngineConfigForm`) is entirely Trino-shaped (coordinator/worker, JVM heap, Trino auth, ingress).

So a StarRocks engine is create-then-delete-only: no image bump, no rescale, no delegation change, no restart without recreating the cluster (and losing its catalogs/assignments).

## Goal

Give a provisioned StarRocks engine the **same management shell** as Trino — a Configuration tab and a Security tab that **save and re-provision**, plus a working Restart — filled with **StarRocks-appropriate** fields (the engines' config surfaces barely overlap, so this is UX-shell parity, not a literal field-for-field copy).

**Editable (Configuration):** FE image, CN image, FE replicas, CN replicas, CPU min/max, memory. **Read-only:** shared-data storage (endpoint/path) — set at create; changing it would orphan the storage volume.

**Editable (Security):** per-user delegation mode (None / JWT / Impersonate), TLS on/off, Ranger binding.

**Restart:** roll the FE + CN pods.

Proven live: edit replicas/resources/image → the cluster re-provisions; toggle delegation → the bootstrap re-runs and assigned workspaces pick up the new mode; restart → FE/CN roll.

## Non-goals

- **No new Trino-shaped sections for StarRocks** (JVM heap, resource groups, autoscaling, ingress, password-file auth) — those are Trino concepts. StarRocks gets only the sections listed above.
- **No storage migration** — shared-data storage stays create-time (read-only in the form).
- **The delegation *mechanism* is unchanged** — this reuses the slice-3 `ensure_tls_cert` / `ensure_delegator_secret` / `StarRocksDelegation` builders + `spawn_starrocks_catalog_wire` bootstrap; it only makes them reachable from `update`, idempotently.
- **RisingWave out.**

## Background — current state (verified)

- **`StarRocksProvisioner::update` already exists** (`egret-orchestrator/src/starrocks.rs:372`) and re-applies the `StarRocksCluster` CR via server-side apply. The provisioner layer is ready; the *handler* is the blocker.
- **`EngineProvisioningPort`** (`egret-engine-mgmt-api`) has `provision`/`status`/`update`/`delete`/`restart`; the dispatch impl (`egret-bin/src/main.rs:487`) routes by `kind` via `for_kind` — but `restart` for `"starrocks"` returns `Ok(())` as a stub (`main.rs:525`), and the operator owns the FE/CN StatefulSets (patching them directly is reverted).
- **`create_starrocks_engine`** (`engines.rs`) builds the StarRocks `spec` + `StarRocksDelegation` from the body (image/replicas/resources/delegation), mints the TLS cert + delegator secret, persists, provisions, and `spawn_starrocks_catalog_wire`s the bootstrap. Update must reuse this spec-building path (factor a shared `build_starrocks_spec` helper).
- **Assignment materialization** (`egret-engine-mgmt/src/assignment.rs`) snapshots `impersonate`/`tls_cert` into each assigned workspace's `databases` row at assign-time via `EngineAssignmentService::assign`. Changing delegation on the engine must re-run that materialization for assigned workspaces (per the approved decision).
- **`EngineDetail.tsx`** already renders the Trino Configuration/Security tabs from a shared `useEngineConfigForm(engine)`; the `!isStarRocks` guards (lines 222–227, 256–266) and the "not yet supported" Overview alert are what hide the StarRocks surface.

## Architecture — components

### A. Backend: `update_engine` StarRocks branch (`egret-engine-mgmt/src/engines.rs`)

1. **Factor `build_starrocks_spec(body, existing) -> (Value, Option<StarRocksDelegation>)`** out of `create_starrocks_engine` so create and update produce byte-identical specs. Storage fields are read from the *existing* spec (not the body) — they're immutable post-create.
2. **`update_engine`:** replace the non-Trino 400 with a `match engine.type`. For `"starrocks"`: rebuild the spec via `build_starrocks_spec`, persist (`engine_mgmt.update_engine`), then `build_provision_req` (below) → `prov.update`.
3. **Delegation-block reuse (idempotent):** `build_starrocks_spec` reuses `ensure_tls_cert(st, engine)` (returns the existing cert if `engine.tls_cert_id` is set, mints on first enable) and `ensure_delegator_secret(st, engine)` (existing delegator secret or mint). Enabling delegation on a previously-open engine mints; disabling drops the block from the spec.
4. **Re-run the bootstrap on change:** if the delegation block changed (mode or tls), call the same post-Ready `spawn_starrocks_catalog_wire` so the FE re-applies the JWT integration + impersonation-grant SQL.
5. **Re-materialize assigned connections (approved decision):** after a delegation/TLS change, for each workspace the engine is assigned to, re-run the connection materialization so the `databases` row's `impersonate`/`tls_cert` matches the new mode. Factor the assign-time materialization body into a reusable `rematerialize_delegated_conn(engine, ws)` used by both `assign` and `update_engine`.

### B. Backend: `build_provision_req` StarRocks branch (`engines.rs:511`)

Today it assembles a Trino `EngineSpec`. Add a `match engine.type` StarRocks arm that assembles the StarRocks `EngineSpec` (kind, images, replicas, resources, storage, delegation) — the same assembly `create` feeds the provisioner. Keep the Trino arm unchanged.

### C. Backend: `restart_engine` StarRocks branch (`engines.rs:2729` + `egret-bin/src/main.rs:524`)

Replace the handler's non-Trino 400 and the port impl's `Ok(())` stub. A StarRocks restart rolls the FE + CN pods by **re-applying the CR with a bumped pod-template annotation** (`egret.io/restartedAt`) — the operator propagates it to the StatefulSets and rolls them, without egret touching operator-owned objects directly. Implement in `StarRocksProvisioner` (a `restart`-flavored re-apply, or an annotation the CR-builder stamps) and route `for_kind("starrocks").restart` to it (add `restart` to the `EngineProvisioner` trait, or handle it in the port impl by calling `update` with the annotation).

### D. Frontend: `useStarRocksConfigForm` (`frontend/src/routes/control-plane/useStarRocksConfigForm.ts`)

A new StarRocks-shaped form hook (parallel to `useEngineConfigForm`, not an extension — the fields barely overlap): `feImage`, `cnImage`, `feReplicas`, `cnReplicas`, `cpuMin`, `cpuMax`, `memory`, `delegationMode`, `tlsEnabled`, plus Ranger binding. Seeds from `engine.spec.starrocksConfig`, tracks `dirty`, and `save()` PUTs via the existing `updateEngine` with a `starrocks` body.

### E. Frontend: de-guard `EngineDetail.tsx` + StarRocks tabs

- Remove the `!isStarRocks` guards; render StarRocks Configuration + Security tabs when `isStarRocks`, Trino ones otherwise. Drop the Overview "not yet supported" alert; enable Restart for StarRocks.
- **StarRocks Configuration tab** — left-nav sections Image / Compute / Resources (+ read-only Storage), mirroring the Trino tab's shell; a `SaveBar` that re-provisions.
- **StarRocks Security tab** — Delegation (None/JWT/Impersonate) + TLS switch + Ranger binding; a `SaveBar`. Reuses the Add-dialog's `DELEGATION_HELP` copy.

## Data flow

- **Edit config:** Configuration tab → `save()` → `PUT /engines/{id}` (starrocks body) → `update_engine` rebuilds spec + `prov.update` re-applies CR → operator rolls the changed role.
- **Change delegation:** Security tab → `PUT` → `update_engine` re-mints (idempotent) + re-runs bootstrap SQL + **re-materializes each assigned workspace's connection** → queries use the new mode immediately.
- **Restart:** Restart button → `POST /engines/{id}/restart` → StarRocks branch re-applies CR with a fresh `restartedAt` → FE/CN roll.

## Testing strategy

- **Unit (CI):** `build_starrocks_spec` produces the expected spec from a body (+ preserves storage from existing); `build_provision_req` StarRocks arm; the delegation-change detector; `rematerialize_delegated_conn` sets the right `impersonate`/`tls_cert`. Frontend: `useStarRocksConfigForm` seeds + builds the PUT body; the StarRocks Configuration/Security tabs render + submit; `EngineDetail` shows the tabs for a StarRocks engine.
- **Live (controller-driven):** on a real StarRocks engine — (1) bump FE replicas 1→2 + change memory → CR re-applies, CN/FE roll to the new shape; (2) toggle delegation None→Impersonate → bootstrap re-runs, an assigned workspace's query switches identity; (3) Restart → FE/CN pods roll. Report exactly what ran.

## Live-only risks (validated at smoke)

1. **CR re-apply semantics** — does the operator roll the FE/CN StatefulSets on a spec change (replicas/resources/image) cleanly, or need a specific field? (server-side apply is already used for create.)
2. **Restart annotation** — whether a bumped pod-template annotation on the CR actually rolls the StatefulSets via the operator (vs. needing a different trigger).
3. **Re-materialization timing** — a delegation change while queries are in flight; the re-materialized row must not break an open connection (acceptable: next query picks up the new mode).

## Work breakdown (for the plan)

1. **`build_starrocks_spec` refactor** — extract create's spec-building; create still green.
2. **`update_engine` StarRocks** — spec rebuild + `build_provision_req` arm + `prov.update`; image/replicas/resources editable; tests.
3. **Delegation change on update** — idempotent re-mint + bootstrap re-run + `rematerialize_delegated_conn` for assigned workspaces; tests.
4. **`restart_engine` StarRocks** — CR re-apply with `restartedAt` annotation; port-impl route; tests.
5. **`useStarRocksConfigForm` + StarRocks Configuration tab** — de-guard EngineDetail; tests.
6. **StarRocks Security tab + Restart enable** — tests.
7. **Live smoke** — edit / delegation-toggle / restart on a real 3.4 StarRocks.

## Self-review

- **Scope focused:** post-create management (edit + restart) only; mechanism reused, storage read-only, no Trino-only sections. ✔
- **No placeholders:** concrete handlers (`update_engine`/`restart_engine`/`build_provision_req`), the existing `StarRocksProvisioner::update`, the reused delegation builders, and the assignment re-materialization seam are named. ✔
- **Consistency:** mirrors Trino's EngineDetail shell + SaveBar; `build_starrocks_spec` is the single create/update spec source; `rematerialize_delegated_conn` is the single assign/update materialization source. ✔
- **Ambiguity:** "parity" = same tabs/shell with StarRocks fields, not a literal Trino field copy; delegation change auto-re-materializes assigned connections (approved). ✔
