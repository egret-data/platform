# StarRocks Engine Config Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Follow superpowers:test-driven-development per task.

**Goal:** Give a provisioned StarRocks engine the same post-create management shell as Trino — a Configuration tab and a Security tab that save-and-re-provision, plus a working Restart — filled with StarRocks-appropriate fields.

**Architecture:** `StarRocksProvisioner::update` already re-applies the CR; the blockers are the handler guards (`update_engine`/`restart_engine` 400 on non-Trino) and the Trino-only `build_provision_req`. Factor create's spec+req building into shared helpers (`build_starrocks_spec` / `build_starrocks_provision_req`), reach them from `update`, re-run the delegation bootstrap + re-materialize assigned connections on a delegation change, and de-guard the frontend tabs.

**Tech Stack:** Rust (`egret-engine-mgmt`, `egret-orchestrator`, `egret-bin`), React/TS frontend, `StarRocksCluster` CR.

**Spec:** `docs/en/specs/2026-07-16-starrocks-config-parity-design.md`. **Branch:** `feat/starrocks-config-parity` (off `main`).

**Reference:** `crates/egret-engine-mgmt/src/engines.rs` (`create_starrocks_engine` @1893, `build_provision_req` @514, `update_engine` @2588, `restart_engine` @2716); `crates/egret-engine-mgmt/src/assignment.rs` (`assign` @40, `delegated_conn_options` @138); `crates/egret-bin/src/main.rs` (`EngineProvisioningPort` impl @487, `restart` stub @524); `frontend/src/routes/control-plane/EngineDetail.tsx` (`!isStarRocks` guards @222/256), `useEngineConfigForm.ts` (shape to mirror), `ControlPlaneEngines.tsx` (`DELEGATION_HELP`, StarRocks field UI to reuse).

---

## Task 1: Factor `build_starrocks_provision_req` out of create

**Files:** Modify `crates/egret-engine-mgmt/src/engines.rs`.

Extract the `EngineSpec` (EngineProvisionReq) assembly from `create_starrocks_engine` (@2081–2194: Ranger resolution + delegation cert/secret + `StarRocksDelegation`/`StarRocksDelegationBootstrap` + the `EngineSpec { … starrocks: Some(StarRocksConfig { … }) }`) into a reusable async helper, so create and (Task 2) update assemble the provision request identically.

- [ ] **Step 1: Write a failing test** in `engines.rs`'s `#[cfg(test)] mod tests` that asserts the helper builds a StarRocks `EngineSpec` from a stored engine + warehouse. Because the helper touches `CpState` (secrets/storage/authz), gate it like the other DB-touching tests (`if std::env::var("DATABASE_URL").is_err() { return; }`) and seed via the existing test harness used by the create tests (grep `create_starrocks_engine` tests / `sample` helpers in this file). Assert: `req.kind == EngineKind::StarRocks`, `req.starrocks.unwrap().fe_image == "<stored>"`, `storage_endpoint`/`storage_path` are populated from the warehouse's storage connection, and `delegation.is_some()` iff the stored `starrocksConfig.delegation` is set.

- [ ] **Step 2: Run → FAIL** (`cargo test -p egret-engine-mgmt --lib build_starrocks_provision_req` — helper undefined).

- [ ] **Step 3: Implement the helper.** Signature:
  ```rust
  /// Assemble the StarRocks provisioning request from a STORED engine row (its
  /// `spec.starrocksConfig`) + the warehouse's storage. Shared by create and
  /// update so both feed the provisioner an identical `EngineSpec`. Resolves
  /// storage creds, the optional Ranger binding, and (when the stored spec has a
  /// delegation block) re-mints/reuses the TLS cert + delegator secret
  /// idempotently, returning the `StarRocksDelegation` for the CR + the
  /// `StarRocksDelegationBootstrap` the post-Ready wire step consumes.
  async fn build_starrocks_provision_req(
      st: &CpState,
      engine: &Engine,
      wh: &egret_catalog_api::Warehouse,
  ) -> Result<(EngineProvisionReq, Option<StarRocksDelegationBootstrap>), CpError> { … }
  ```
  Move, verbatim (adjusting variable sources to read from `engine.spec["starrocksConfig"]` instead of the create body): storage resolution (currently @1903–1923), Ranger resolution (@2081–2089), the delegation cert/secret minting (@2111–2171, keeping the same `ensure_tls_cert`/`ensure_delegator_secret` calls — already idempotent by `engine.tls_cert_id`/`delegator_secret_id`), and the `EngineSpec` construction (@2173–2194). Read `fe_image`/`cn_image`/`fe_replicas`/`cn_replicas`/`fe_resources`/`cn_resources`/`delegation.mode`/`delegation.tlsEnabled` from `engine.spec["starrocksConfig"]` (add small typed getters or `serde_json` accessors). Compute `tls_enabled`/`delegation_mode` from that stored block (not a body).

- [ ] **Step 4: Rewire `create_starrocks_engine` to call the helper.** After the engine row + primary catalog + authz bind exist (the reloaded `engine` @2067–2079), replace @2081–2194 with:
  ```rust
  let wh_ref = &wh;
  let (req, delegation_bootstrap) = build_starrocks_provision_req(&st, &engine, wh_ref).await
      .map_err(|e| { /* same rollback: delete_engine + revoke_client */ e })?;
  ```
  (Keep the existing rollback-on-error pattern; the helper returns `CpError`, so wrap with the same `delete_engine`+`revoke_client` cleanup the surrounding code uses.) Leave the storage-path derivation that create needs BEFORE the engine row (the `storage_path`/`sc` used only for the pre-row storage checks) — but the authoritative storage for the req now comes from inside the helper, so remove the now-duplicated post-row storage plumbing.

- [ ] **Step 5: Run → PASS.** `cargo test -p egret-engine-mgmt` (all create/starrocks tests still green — the refactor is behavior-preserving). `cargo fmt -p egret-engine-mgmt`; `cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings` clean.

- [ ] **Step 6: Commit.** `refactor(engine): extract build_starrocks_provision_req shared by create/update`.

---

## Task 2: `update_engine` + `build_provision_req` StarRocks branches (image/replicas/resources)

**Files:** Modify `crates/egret-engine-mgmt/src/engines.rs`.

- [ ] **Step 1: Write failing tests.** In `mod tests` (DB-gated), a test `update_starrocks_engine_rescales`: create a StarRocks engine, PUT a body with `feReplicas: 2` + `memory` change, assert the stored `spec.starrocksConfig.feReplicas == 2` and the resources changed; and `update_starrocks_engine_preserves_storage`: assert `spec` after update still has no storage leak and the provision req's `storage_path` is unchanged. Also `build_provision_req_dispatches_starrocks`: a stored StarRocks engine → `build_provision_req` returns a StarRocks `EngineSpec` (kind StarRocks), not a Trino one.

- [ ] **Step 2: Run → FAIL** (update 400s for starrocks; `build_provision_req` builds Trino).

- [ ] **Step 3a: Add a `build_starrocks_spec(sr_body, existing) -> serde_json::Value` helper** that produces the stored `spec` JSON (mirroring create @1991–2002), reading each field from the body, else falling back to the `existing` stored spec (so a PUT that omits a field preserves it). Refactor create's inline spec (@1991–2002) to call it with `existing = None`.

- [ ] **Step 3b: `update_engine` StarRocks branch.** Replace the fail-fast (@2604–2608) with:
  ```rust
  if engine.r#type == "starrocks" {
      return update_starrocks_engine(st, engine, b).await;
  }
  if engine.r#type != "trino" {
      return Err(CpError::BadRequest("update not supported for this engine kind".into()));
  }
  ```
  Add `async fn update_starrocks_engine(st, engine, b)`:
  1. `let sr_body = b.starrocks.clone().unwrap_or_default();`
  2. validate `delegation_mode` the same way create does (@1944–1951).
  3. `let spec = build_starrocks_spec(&sr_body, Some(&engine.spec));`
  4. `st.engine_mgmt.update_engine(engine.id, &spec, &engine.status).await?;` then reload `engine`.
  5. resolve the warehouse: `let wh = st.catalog.get_warehouse(engine.warehouse_id).await?…` (grep how create obtains `wh` — the create route resolves it before dispatch; reuse that resolver).
  6. `let (req, bootstrap) = build_starrocks_provision_req(&st, &engine, &wh).await?;`
  7. `prov.update(&req).await` (map err → `BadRequest`).
  8. (Task 3 adds the delegation-change bootstrap + re-materialize here.)
  9. `Ok(Json(engine.into()))`.

- [ ] **Step 3c: `build_provision_req` StarRocks arm.** At the top of `build_provision_req` (@514), before the Trino assembly:
  ```rust
  if engine.r#type == "starrocks" {
      let wh = /* resolve warehouse from engine.warehouse_id */;
      let (req, _bootstrap) = build_starrocks_provision_req(st, engine, &wh).await?;
      return Ok(req);
  }
  ```
  (This makes any generic `build_provision_req` caller correct for StarRocks; `update_starrocks_engine` may call `build_starrocks_provision_req` directly to also get the bootstrap.)

- [ ] **Step 4: Run → PASS.** `cargo test -p egret-engine-mgmt`; fmt + clippy clean.

- [ ] **Step 5: Commit.** `feat(engine): editable StarRocks config via update (image/replicas/resources)`.

---

## Task 3: Delegation change on update — re-bootstrap + re-materialize assigned connections

**Files:** Modify `crates/egret-engine-mgmt/src/engines.rs`, `crates/egret-engine-mgmt/src/assignment.rs`.

- [ ] **Step 1: Write failing tests.** In `assignment.rs` `mod tests` (DB-gated): `rematerialize_updates_conn_opts` — assign a StarRocks engine (mode=impersonate) to a workspace, then call the new `EngineAssignmentService::rematerialize_for_engine(engine_id)` after flipping the stored engine to mode=jwt, and assert the workspace's `databases` row `options` changed (impersonate dropped / tls per jwt). In `engines.rs`: `update_starrocks_delegation_change_rematerializes` — after a PUT that changes delegation, the assigned workspace's connection reflects the new mode.

- [ ] **Step 2: Run → FAIL** (`rematerialize_for_engine` undefined).

- [ ] **Step 3a: Factor the materialization body in `assignment.rs`.** Extract the per-workspace connection upsert from `assign` (@63–110: dedupe name, `delegated_conn_options(&engine)`, insert/update the `databases` row) into:
  ```rust
  async fn materialize_conn(&self, engine: &Engine, ws: Uuid) -> Result<(), EngineMgmtError> { … }
  ```
  Have `assign` call it. Then add:
  ```rust
  /// Re-run connection materialization for every workspace this engine is
  /// assigned to — called after a delegation/TLS change so each workspace's
  /// stored `databases` options (impersonate/tls_cert) match the new mode.
  pub async fn rematerialize_for_engine(&self, engine_id: Uuid) -> Result<(), EngineMgmtError> {
      let engine = self.engines.get_engine(engine_id).await?…;
      for ws in self.engines.list_engine_workspace_ids(engine_id).await? {
          self.materialize_conn(&engine, ws).await?;
      }
      Ok(())
  }
  ```
  (Grep for the existing "list workspaces assigned to engine" query — `list_engine_workspaces`/`WorkspacesCard`'s backing call — and reuse/add a `list_engine_workspace_ids`.)

- [ ] **Step 3b: Detect the change + wire it in `update_starrocks_engine`.** Before persisting, compute `let deleg_changed = old_delegation != new_delegation` (compare the `mode`+`tlsEnabled` of `engine.spec["starrocksConfig"]["delegation"]` vs the rebuilt spec's). After `prov.update` succeeds, if `deleg_changed`:
  1. re-run the post-Ready bootstrap the same way create does — call the existing `spawn_starrocks_catalog_wire(...)` with the `bootstrap` returned from `build_starrocks_provision_req` (grep its create-site call for the exact args).
  2. `st.assignment.rematerialize_for_engine(engine.id).await` (add `assignment: EngineAssignmentService` to `CpState` if not already reachable — grep `EngineAssignmentService` construction in `egret-bin`).

- [ ] **Step 4: Run → PASS.** `cargo test -p egret-engine-mgmt`; fmt + clippy clean.

- [ ] **Step 5: Commit.** `feat(engine): delegation change on StarRocks update re-bootstraps + re-materializes connections`.

---

## Task 4: `restart_engine` StarRocks branch (roll FE/CN)

**Files:** Modify `crates/egret-engine-mgmt/src/engines.rs` (`restart_engine` @2716), `crates/egret-bin/src/main.rs` (port impl `restart` @524), `crates/egret-orchestrator/src/starrocks.rs` (CR annotation).

- [ ] **Step 1: Write a failing test** in `egret-orchestrator/src/starrocks.rs` `mod tests`: `restart_stamps_pod_annotation` — `starrocks_cluster_cr` with a `restarted_at` input carries `spec.starRocksFeSpec.podLabels`/annotations (whichever the operator honors — check `starrocks-kubernetes-operator/` CRD for the pod-annotation field; likely `annotations` under the fe/cn spec) with the stamp, and both FE and CN carry it.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3a: CR builder accepts an optional restart stamp.** Add a param/field so `starrocks_cluster_cr` can stamp `spec.starRocksFeSpec.annotations["egret.io/restartedAt"]` + the CN spec the same. When absent, unchanged. (Confirm the operator field name against the CRD in `starrocks-kubernetes-operator/deploy/starrocks.com_starrocksclusters.yaml` — use `podLabels`/`annotations` as supported.)

- [ ] **Step 3b: `StarRocksProvisioner` restart.** Add a method that re-applies the CR with a fresh stamp (reuse `provision`/`update`'s apply path with the stamp set). Since the timestamp can't come from `Date.now()` in the orchestrator, thread it from the caller (egret-bin passes a timestamp string).

- [ ] **Step 3c: Route it.** Add `restart` to the `EngineProvisioner` trait (default `Ok(())` for Trino? no — Trino already rolls Deployments in the port impl) OR keep dispatch in the port impl: in `main.rs` `restart` (@524), replace the `if kind == "starrocks" { return Ok(()); }` stub with a call that re-applies the StarRocks CR with a stamp (`self.starrocks.restart(namespace, name, &stamp).await`). Generate `stamp` in egret-bin (allowed there).

- [ ] **Step 3d: Un-gate the handler.** In `restart_engine` (@2729), replace the non-Trino 400 with allowing `"starrocks"` (keep 400 for other kinds). It already calls `prov.restart(&engine.r#type, …)`.

- [ ] **Step 4: Run → PASS.** `cargo test -p egret-orchestrator starrocks`; `cargo test -p egret-engine-mgmt`; fmt + clippy clean.

- [ ] **Step 5: Commit.** `feat(engine): restart StarRocks by rolling FE/CN via a CR annotation`.

---

## Task 5: Frontend — `useStarRocksConfigForm` + StarRocks Configuration tab

**Files:** Create `frontend/src/routes/control-plane/useStarRocksConfigForm.ts`; modify `frontend/src/routes/control-plane/EngineDetail.tsx`; create `frontend/src/routes/control-plane/StarRocksConfigTab.tsx`.

- [ ] **Step 1: Write a failing test** `frontend/src/routes/control-plane/__tests__/StarRocksConfigTab.test.tsx`: render the tab for a StarRocks engine (spec `{ starrocksConfig: { feImage, cnImage, feReplicas: 1, cnReplicas: 1 } }`), change FE replicas to 2, click Save, assert `updateEngine` is called with `{ kind: "starrocks", starrocks: { feReplicas: 2, … } }`. Mock `updateEngine` from `@/api/controlPlane`.

- [ ] **Step 2: Run → FAIL** (`npx vitest run src/routes/control-plane/__tests__/StarRocksConfigTab.test.tsx`).

- [ ] **Step 3a: `useStarRocksConfigForm(engine)` hook.** Seeds state from `engine.spec.starrocksConfig` (feImage/cnImage/feReplicas/cnReplicas/cpuMin/cpuMax/memory/delegation.mode/delegation.tlsEnabled). Exposes setters, `dirty` (compare to seed), `saving`/`saved`/`error`, and:
  ```ts
  async function save() {
    await updateEngine(engine.id, {
      name: engine.name, warehouseId: engine.warehouseId ?? "",
      kind: "starrocks",
      starrocks: { feImage, cnImage, feReplicas, cnReplicas,
        feResources: resourcesOrUndefined(cpuMin, cpuMax, memory),
        ...(delegationMode !== "none" ? { delegationMode, tlsEnabled } : {}) },
      ...TRINO_FIELD_DEFAULTS,  // required-but-ignored body fields (mirror ControlPlaneEngines)
    });
  }
  ```
  (Reuse `updateEngine` from `controlPlane.ts:823`; mirror `ControlPlaneEngines.tsx`'s `TRINO_FIELD_DEFAULTS`/resource shape.)

- [ ] **Step 3b: `StarRocksConfigTab.tsx`** — left-nav sections Image (FE/CN image inputs) / Compute (FE/CN replicas) / Resources (CPU min/max, memory) / Storage (read-only: endpoint/path from spec) + a `SaveBar` (reuse `EngineDetail`'s `SaveBar` by exporting it, or a small local copy). On save success, invalidate `["cp", "engine", id]`.

- [ ] **Step 3c: De-guard EngineDetail (config).** In `EngineTabs`, render a `<TabsTrigger value="configuration">` for StarRocks too; in the content, branch: `isStarRocks ? <StarRocksConfigTab engine={engine}/> : <ConfigurationTab cfg={cfg} …/>`. Instantiate the StarRocks form hook alongside (or lazily inside the tab).

- [ ] **Step 4: Run → PASS.** `npx vitest run src/routes/control-plane`; `npm run typecheck` (only the pre-existing `useEngineConfigForm.dirty.test.tsx` error allowed); eslint clean on touched files.

- [ ] **Step 5: Commit.** `feat(fe): StarRocks Configuration tab (editable image/compute/resources)`.

---

## Task 6: Frontend — StarRocks Security tab + enable Restart

**Files:** Modify `frontend/src/routes/control-plane/EngineDetail.tsx`; create `frontend/src/routes/control-plane/StarRocksSecurityTab.tsx`.

- [ ] **Step 1: Write a failing test** `__tests__/StarRocksSecurityTab.test.tsx`: render for a StarRocks engine, change delegation None→Impersonate, toggle TLS, Save → `updateEngine` called with `{ starrocks: { delegationMode: "impersonate", tlsEnabled: true } }`. Plus an `EngineDetail` test: a StarRocks engine renders the Configuration + Security tabs and the Restart button is enabled.

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3a: `StarRocksSecurityTab.tsx`** — Delegation Select (None/JWT/Impersonate, reuse `ControlPlaneEngines.tsx`'s `DELEGATION_HELP` copy — export it or duplicate the map), a TLS Switch (with the ≥3.4.1 hint), and the Ranger binding control (mirror how the create/authz bind is surfaced; if Ranger binding isn't in the update body yet, scope this tab to Delegation + TLS and leave a note). Shares the same `useStarRocksConfigForm` instance as the Config tab (lift it to `EngineTabs`, like Trino's shared `cfg`), so one Save covers both. `SaveBar` at the bottom.

- [ ] **Step 3b: De-guard EngineDetail (security + restart).** Render `<TabsTrigger value="security">` for StarRocks → `<StarRocksSecurityTab .../>`. Remove the Overview "not yet supported" alert for StarRocks. Set `restartDisabled={false}` for StarRocks in `StatusCard` (the backend now supports it after Task 4).

- [ ] **Step 4: Run → PASS.** `npx vitest run src/routes/control-plane`; typecheck + eslint clean.

- [ ] **Step 5: Commit.** `feat(fe): StarRocks Security tab + enable Restart`.

---

## Task 7: Full CI + live smoke

- [ ] **Step 1: Full local CI.** `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` (vendored `mysql_async` warning excepted); `DATABASE_URL=…egret_test cargo test --workspace` (the 2 pre-existing `assignment.rs` failures + the `engine-job open_secret` ones are unrelated); frontend `npm run lint` + `typecheck` (pre-existing `useEngineConfigForm.dirty` error only) + `vitest` + `build`.

- [ ] **Step 2: Provision a real StarRocks** (3.4 image, delegation on) and open its detail page — Configuration + Security tabs render, Restart enabled.

- [ ] **Step 3: Edit config** — bump FE replicas 1→2 + change memory → Save → confirm the CR re-applies and the operator scales/rolls (`kubectl -n egret-engines get pod | grep <name>`; the new replica appears / pods roll).

- [ ] **Step 4: Change delegation** — flip None→Impersonate (or Impersonate→JWT) on an assigned engine → Save → confirm the bootstrap re-runs (FE `SHOW SECURITY INTEGRATIONS` / the impersonation grant) and the assigned workspace's `databases` row options changed.

- [ ] **Step 5: Restart** — click Restart → confirm FE + CN pods roll (`kubectl` shows new pod ages).

- [ ] **Step 6: Report** exactly what ran; fold any live fixes back into the code + spec.

---

## Self-Review

- **Spec coverage:** Configuration edit (image/replicas/resources) → T2; Security edit (delegation/TLS) + re-materialize → T3; Restart → T4; FE Configuration tab → T5; FE Security tab + Restart enable → T6; live proof → T7. `build_starrocks_provision_req`/`build_starrocks_spec` refactor (spec §A) → T1. Storage read-only (spec non-goal) → preserved in T2's `build_starrocks_spec` fallback. Covered.
- **Type consistency:** `build_starrocks_provision_req(st, engine, wh) -> (EngineProvisionReq, Option<StarRocksDelegationBootstrap>)` defined in T1, consumed in T2/T3; `build_starrocks_spec(sr_body, existing)` defined T2, reused by create; `rematerialize_for_engine`/`materialize_conn` defined T3; `useStarRocksConfigForm` defined T5, reused by T6 (shared instance). FE `updateEngine` + `CreateEngineInput.starrocks` already exist.
- **No placeholders:** every step names the exact function + line anchor; the one genuinely live-only unknown (the operator's pod-annotation field name for restart) is flagged in T4 Step 3a with where to confirm it (the CRD file).
- **Ambiguity:** parity = same tabs/shell with StarRocks fields; delegation change auto-re-materializes (approved); storage read-only.
