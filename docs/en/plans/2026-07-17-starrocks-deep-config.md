# StarRocks Deep Cluster Config Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Follow superpowers:test-driven-development per task.

**Goal:** Deepen the StarRocks Configuration tab to Trino-level detail — `fe.conf`/`cn.conf` overrides, CN autoscaling (HPA), and Pod & Containers/scheduling — all editable + save-and-reprovision.

**Architecture:** Reuse #265's update/restart path (`update_starrocks_engine`, `build_starrocks_spec`, `build_starrocks_provision_req`, `StarRocksProvisioner::update`) and the engine-agnostic FE `config/` toolkit. Extend the `StarRocksConfig` contract + the CR builder to carry/render the new fields; protected egret-owned `fe.conf` keys are guarded (400).

**Tech Stack:** Rust (`egret-engine-mgmt-api`, `egret-orchestrator`, `egret-engine-mgmt`), React/TS, `StarRocksCluster` CR.

**Spec:** `docs/en/specs/2026-07-17-starrocks-deep-config-design.md`. **Branch:** `feat/starrocks-deep-config` (off `main`).

**Reference anchors:** `crates/egret-engine-mgmt-api/src/lib.rs` (`StarRocksConfig`@407); `crates/egret-engine-mgmt/src/engines.rs` (`StarRocksConfigBody`@176, `build_starrocks_spec`, `build_starrocks_provision_req`, `update_starrocks_engine` — all from #265); `crates/egret-orchestrator/src/starrocks.rs` (`starrocks_fe_configmap`@40, `starrocks_cluster_cr`@211 with `fe_spec`@217 / `cn_spec`@329 / merge@343); `frontend/src/routes/control-plane/config/` (`KeyedPropertyList`, `PlacementSection`, `PodSection`, `ContainersMountsSection`, `RawJsonField`, `validityRegistry`); `frontend/src/routes/control-plane/{useStarRocksConfigForm.ts,StarRocksConfigTab.tsx}` (from #265).

---

## Task 1: Contract + body/spec plumbing (new fields, omit=preserve, protected-key 400)

**Files:** `crates/egret-engine-mgmt-api/src/lib.rs`, `crates/egret-engine-mgmt/src/engines.rs`.

- [ ] **Step 1: Write failing tests.** In `engines.rs mod tests`: `build_starrocks_spec_roundtrips_deep_fields` (a body with `feConfig`/`cnConfig`/`cnAutoscaling`/`pod` → stored spec carries them; a later update omitting them PRESERVES, per #265 rule); `feconfig_protected_key_rejected` (a `feConfig` with key `run_mode` or `aws_s3_endpoint` → `validate_sr_*` returns 400). Run → FAIL.

- [ ] **Step 2: Extend the contract** (`egret-engine-mgmt-api/src/lib.rs`, near `StarRocksConfig`@407). Add:
  ```rust
  #[derive(Clone, Debug, Default, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct CnAutoscaling {
      pub min_replicas: Option<u32>,
      pub max_replicas: u32,
      pub target_cpu_percent: Option<u32>,
  }
  #[derive(Clone, Debug, Default, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct StarRocksPod {
      #[serde(default)] pub service_account: Option<String>,
      #[serde(default)] pub node_selector: std::collections::BTreeMap<String, String>,
      #[serde(default)] pub annotations: std::collections::BTreeMap<String, String>,
      #[serde(default)] pub pod_labels: std::collections::BTreeMap<String, String>,
      #[serde(default)] pub env: Vec<serde_json::Value>,        // [{name,value}|{name,valueFrom}]
      #[serde(default)] pub tolerations: Option<serde_json::Value>,
      #[serde(default)] pub affinity: Option<serde_json::Value>,
      #[serde(default)] pub security_context: Option<serde_json::Value>,
      #[serde(default)] pub init_containers: Option<serde_json::Value>,
      #[serde(default)] pub sidecars: Option<serde_json::Value>,
      #[serde(default)] pub image_pull_secrets: Vec<String>,
  }
  ```
  Add to `StarRocksConfig` (all with `#[serde(default)]`): `fe_config: BTreeMap<String,String>`, `cn_config: BTreeMap<String,String>`, `cn_autoscaling: Option<CnAutoscaling>`, `pod: Option<StarRocksPod>`. Update the manual `Debug` (if any) + every `StarRocksConfig { … }` literal (grep — tests + `build_starrocks_provision_req`) to add the new fields (use `..Default::default()` where a struct-update is cleaner).

- [ ] **Step 3: Extend `StarRocksConfigBody`** (`engines.rs`@176): add `fe_config: Option<BTreeMap<String,String>>`, `cn_config: Option<BTreeMap<String,String>>`, `cn_autoscaling: Option<serde_json::Value>`, `pod: Option<serde_json::Value>` (camelCase via the struct's existing `rename_all`). Keep them optional so existing bodies parse.

- [ ] **Step 4: Thread through `build_starrocks_spec`** (store into `starrocksConfig`, omit=preserve using the same `existing` fallback the #265 fields use) and **`build_starrocks_provision_req`** (read them back into `StarRocksConfig`). Add a `const SR_PROTECTED_FE_KEYS: &[&str] = &["run_mode","cloud_native_meta_port","cloud_native_storage_type","aws_s3_endpoint","aws_s3_path","aws_s3_access_key","aws_s3_secret_key","aws_s3_use_aws_sdk_default_behavior","aws_s3_enable_path_style_access","enable_load_volume_from_conf","access_control","ssl_keystore_location","ssl_keystore_password","ssl_key_password","ssl_force_secure_transport","http_port","rpc_port","query_port","edit_log_port"];` and, in `validate_sr_delegation_mode`'s sibling validation (or a new `validate_sr_config`), reject a `feConfig` that contains any protected key: `Err(CpError::BadRequest(format!("fe.conf key '{k}' is managed by egret and cannot be overridden")))`. Call it from create + `update_starrocks_engine`.

- [ ] **Step 5: Run → PASS.** `cargo fmt`; `cargo clippy -p egret-engine-mgmt -p egret-engine-mgmt-api --all-targets -- -D warnings` clean; `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-engine-mgmt -p egret-engine-mgmt-api` (green except the 2 known pre-existing assignment.rs failures).

- [ ] **Step 6: Commit.** `feat(engine): deep StarRocks config contract + body/spec plumbing + protected-key guard`.

---

## Task 2: FE config merge (`starrocks_fe_configmap`)

**Files:** `crates/egret-orchestrator/src/starrocks.rs`.

- [ ] **Step 1: Failing tests** (`mod tests`): `fe_configmap_applies_override` (`sr.fe_config = {"sys_log_level":"WARN"}` → the rendered `fe.conf` has `sys_log_level = WARN`, NOT `INFO`, and exactly once — no duplicate line); `fe_configmap_appends_new_key` (`{"query_timeout":"600"}` → appended); `fe_configmap_ignores_protected` (defense-in-depth: even if a protected key sneaks in, the egret value wins — belt with the Task-1 API guard). Run → FAIL.

- [ ] **Step 2: Refactor `starrocks_fe_configmap`** (@40) to build the base as an ordered list of lines, then apply overrides by key: for each `(k,v)` in `sr.fe_config`, if a base line `^\s*{k}\s*=` exists, REPLACE it in place (preserving order); else append `k = v`. Keep comment/blank lines. Skip any key in `SR_PROTECTED_FE_KEYS` (egret value wins — the API already 400s, this is belt-and-suspenders). Re-serialize to the `fe.conf` string. (A small helper `merge_conf(base_lines: Vec<String>, overrides: &BTreeMap<String,String>, protected: &[&str]) -> String` keeps it testable.)

- [ ] **Step 3: Run → PASS.** fmt + clippy clean; `cargo test -p egret-orchestrator starrocks`.

- [ ] **Step 4: Commit.** `feat(orchestrator): merge admin fe.conf overrides (protected keys guarded)`.

---

## Task 3: CN config ConfigMap + wire `starRocksCnSpec.configMapInfo`

**Files:** `crates/egret-orchestrator/src/starrocks.rs`; the provisioner apply path (grep where `starrocks_fe_configmap` is applied — likely `StarRocksProvisioner::provision`/`update`).

- [ ] **Step 1: Failing tests:** `cn_configmap_some_when_nonempty` (`sr.cn_config` non-empty → `starrocks_cn_configmap` returns `Some({name}-cn-cm)` with `data["cn.conf"]` containing the keys; empty → `None`); `cr_wires_cn_configmapinfo` (`starrocks_cluster_cr` sets `starRocksCnSpec.configMapInfo = {configMapName:"{name}-cn-cm", resolveKey:"cn.conf"}` when `cn_config` non-empty, absent otherwise). Run → FAIL.

- [ ] **Step 2: Add `pub fn starrocks_cn_configmap(spec: &EngineSpec) -> Option<Value>`** mirroring `starrocks_fe_configmap`'s ConfigMap shape (`metadata.name = {name}-cn-cm`, `data = {"cn.conf": <lines from sr.cn_config>}`), returning `None` when `cn_config` is empty. In `starrocks_cluster_cr` (`cn_spec`@329), when `cn_config` non-empty set `cn_spec["configMapInfo"] = json!({ "configMapName": format!("{}-cn-cm", spec.name), "resolveKey": "cn.conf" })`.

- [ ] **Step 3: Apply the CN ConfigMap in the provisioner.** Where `provision`/`update` apply the FE ConfigMap (grep `starrocks_fe_configmap(` in `StarRocksProvisioner`), also apply `starrocks_cn_configmap(spec)` when `Some` (server-side apply, same as FE). Ensure `update` re-applies it too (so edits land).

- [ ] **Step 4: Run → PASS.** fmt + clippy; `cargo test -p egret-orchestrator starrocks`.

- [ ] **Step 5: Commit.** `feat(orchestrator): CN cn.conf ConfigMap wired onto starRocksCnSpec`.

---

## Task 4: CN autoscaling (`autoScalingPolicy`)

**Files:** `crates/egret-orchestrator/src/starrocks.rs`.

- [ ] **Step 1: Failing test:** `cr_renders_cn_autoscaling` (`sr.cn_autoscaling = Some({min:1,max:3,cpu:70})` → `starRocksCnSpec.autoScalingPolicy = {version:"v2", minReplicas:1, maxReplicas:3, hpaPolicy:{metrics:[<cpu 70% utilization MetricSpec>]}}`; absent → no `autoScalingPolicy` key). Run → FAIL.

- [ ] **Step 2: Render it** in `starrocks_cluster_cr` on `cn_spec` (@329) when `sr.cn_autoscaling` is set:
  ```rust
  if let Some(a) = &sr.cn_autoscaling {
      let mut pol = json!({ "version": "v2", "maxReplicas": a.max_replicas });
      if let Some(m) = a.min_replicas { pol["minReplicas"] = json!(m); }
      if let Some(cpu) = a.target_cpu_percent {
          pol["hpaPolicy"] = json!({ "metrics": [{
              "type": "Resource",
              "resource": { "name": "cpu", "target": { "type": "Utilization", "averageUtilization": cpu } }
          }]});
      }
      cn_spec["autoScalingPolicy"] = pol;
  }
  ```
  (Match the operator's `AutoScalingPolicy`/`HPAPolicy` shape — `autoscalingv2beta2.MetricSpec`. Confirm the `version` enum value the operator expects — `v2` per `auto_scale.go`.)

- [ ] **Step 3: Run → PASS.** fmt + clippy; `cargo test -p egret-orchestrator starrocks`.

- [ ] **Step 4: Commit.** `feat(orchestrator): render CN HPA autoScalingPolicy on the StarRocks CR`.

---

## Task 5: Pod & Containers / scheduling on FE + CN

**Files:** `crates/egret-orchestrator/src/starrocks.rs`.

- [ ] **Step 1: Failing tests:** `cr_renders_pod_fields_on_both_roles` (`sr.pod` with serviceAccount + nodeSelector + a toleration → both `starRocksFeSpec` and `starRocksCnSpec` carry them); `pod_initcontainers_preserve_ssl_builder` (a TLS engine + `sr.pod.init_containers = [<admin container>]` → FE `initContainers` contains BOTH the `build-keystore` container AND the admin one). Run → FAIL.

- [ ] **Step 2: Add a helper `apply_pod_fields(spec: &mut Value, pod: &StarRocksPod)`** that sets `serviceAccount`, `nodeSelector`, `annotations` (merge with any existing, e.g. the restart stamp), `podLabels`, `feEnvVars`/`env` (use the field name the operator expects — check `LoadSpec`; likely `feEnvVars`/`beEnvVars` are FE/BE-specific, but the generic pod env is on the container — confirm and use `initContainers`/`sidecars` verbatim as raw arrays), `tolerations`, `affinity`, `securityContext`, `imagePullSecrets`, and APPENDS `init_containers`/`sidecars` (don't overwrite). Call it for BOTH `fe_spec` and `cn_spec`. On FE, the SSL `initContainers` (@309) already sets `fe_spec["initContainers"]` — make `apply_pod_fields` APPEND the admin init-containers to the existing array (build the SSL entry first, then extend), never replace. Same care for `storageVolumes` if the pod block ever carries them (it doesn't in this MVP — only init/sidecar/env/scheduling).

- [ ] **Step 3: Run → PASS.** fmt + clippy; `cargo test -p egret-orchestrator starrocks` (all prior SSL/restart/config tests still green).

- [ ] **Step 4: Commit.** `feat(orchestrator): render pod/scheduling fields on FE+CN (SSL-init-safe)`.

---

## Task 6: Frontend — FE/CN config + Autoscaling sections

**Files:** `frontend/src/routes/control-plane/useStarRocksConfigForm.ts`, `StarRocksConfigTab.tsx`, tests.

- [ ] **Step 1: Failing test** (`__tests__/StarRocksConfigTab.test.tsx`): add an FE-config key `sys_log_level=WARN` via the section, Save → `updateEngine` called with `starrocks.feConfig` `{sys_log_level:"WARN"}`; set autoscaling max=3 → body has `cnAutoscaling:{maxReplicas:3,...}`. Run → FAIL.

- [ ] **Step 2: Extend `useStarRocksConfigForm`.** Add state + seed from `engine.spec.starrocksConfig`: `feConfig`/`cnConfig` (`Record<string,string>`), `cnAutoscaling` (`{enabled,minReplicas,maxReplicas,targetCpuPercent}`), and the `pod` block (deferred UI to Task 7, but seed/carry it here so both sections share the hook). Include them in `save()`'s `starrocks` body (omit when empty/disabled, mirroring the resources rule). Extend the `resolveConfig`-based dirty snapshot to include them.

- [ ] **Step 3: Add nav sections to `StarRocksConfigTab`.** New left-nav items **FE config**, **CN config**, **Autoscaling** (Pod & Containers added in Task 7). FE/CN config: reuse `KeyedPropertyList` (read its props first — key/value rows + add/remove) bound to `feConfig`/`cnConfig`. Autoscaling: a small form (Enable switch + min/max replicas + target CPU% inputs). Reuse the `SaveBar`.

- [ ] **Step 4: Run → PASS.** `cd frontend`: `npx vitest run src/routes/control-plane`; `npm run typecheck` (only the known pre-existing error); `npx eslint` touched files clean.

- [ ] **Step 5: Commit.** `feat(fe): StarRocks FE/CN config + Autoscaling sections`.

---

## Task 7: Frontend — Pod & Containers section

**Files:** `frontend/src/routes/control-plane/StarRocksConfigTab.tsx`, `useStarRocksConfigForm.ts` (wire the pod setters), tests.

- [ ] **Step 1: Failing test:** in the Pod & Containers section, set a serviceAccount + a nodeSelector row + type invalid JSON in the affinity raw field → Save is DISABLED (ValidityProvider) until the JSON is valid/blank; with valid input, Save → body `starrocks.pod` carries `serviceAccount` + `nodeSelector`. Run → FAIL.

- [ ] **Step 2: Add the Pod & Containers section** reusing `PodSection` / `PlacementSection` / `ContainersMountsSection` + wrap in `ValidityProvider` (read each component's props first — they take specific state shapes for serviceAccount/securityContext/nodeSelector/affinity/tolerations/env/init/sidecar; adapt the StarRocks `pod` state to those props, or lift the minimal subset the MVP needs). The `SaveBar`'s disabled prop wires to the ValidityProvider's `onAnyInvalidChange` (mirror `ConfigurationTab`'s `rawInvalid`). Map the section's state back into the hook's `pod` block for `save()`.

- [ ] **Step 3: Run → PASS.** vitest + typecheck + eslint as Task 6.

- [ ] **Step 4: Commit.** `feat(fe): StarRocks Pod & Containers config section`.

---

## Task 8: Full CI + live smoke

- [ ] **Step 1: Full local CI.** `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings` (vendored `mysql_async` warning excepted); `DATABASE_URL=…egret_test cargo test --workspace` (2 pre-existing assignment.rs failures unrelated); frontend `lint` + `typecheck` (known error only) + `vitest` + `build`.

- [ ] **Step 2: Provision a real 3.4 StarRocks**, open its Configuration tab — FE config / CN config / Autoscaling / Pod & Containers sections render.

- [ ] **Step 3: FE config** — set `sys_log_level = WARN` → Save → `kubectl -n egret-engines get cm <name>-fe-cm -o jsonpath='{.data.fe\.conf}'` shows `sys_log_level = WARN` (once), pods roll.

- [ ] **Step 4: CN autoscaling** — enable min 1 / max 3 / 70% CPU → Save → the CR's `starRocksCnSpec.autoScalingPolicy` is set and `kubectl -n egret-engines get hpa` shows the operator-created HPA.

- [ ] **Step 5: Pod field** — set a nodeSelector → Save → `kubectl -n egret-engines get pod <name>-cn-0 -o jsonpath='{.spec.nodeSelector}'` reflects it.

- [ ] **Step 6: Protected key** — attempt an FE-config `run_mode=shared_nothing` → the API returns 400 (guard holds).

- [ ] **Step 7: Report** exactly what ran; fold any live fixes back.

---

## Self-Review

- **Spec coverage:** contract/plumbing/protected-key → T1; fe.conf merge → T2; cn.conf → T3; autoscaling → T4; pod/scheduling → T5; FE config/CN/autoscaling UI → T6; Pod & Containers UI → T7; live → T8. Covered.
- **Type consistency:** `CnAutoscaling`/`StarRocksPod` (T1) are the contract; `SR_PROTECTED_FE_KEYS` (T1) reused by T2's belt-and-suspenders; `apply_pod_fields` (T5) is the single FE+CN renderer; the FE `feConfig`/`cnConfig`/`cnAutoscaling`/`pod` state (T6/T7) matches the camelCase body T1/T3 read. `build_starrocks_spec`/`build_starrocks_provision_req` (from #265) are the store/read seam.
- **No placeholders:** concrete types, the `starrocks_fe_configmap` merge point, the operator `autoScalingPolicy`/LoadSpec fields, and the reused FE components are named. The one live-only unknown (exact operator env field name / CN configMap mount) is flagged in T5/T3 with where to confirm it.
- **Ambiguity:** pod fields cluster-wide (both roles); protected keys 400; autoscaling = HPA v2; merge = parse-and-replace.
