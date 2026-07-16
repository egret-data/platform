# StarRocks Provisioning — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Follow superpowers:test-driven-development per task.

**Goal:** Provision StarRocks FE+CN shared-data clusters from egret's control plane via the `kube-starrocks` operator + a `StarRocksCluster` CR, auto-wired to the embedded Iceberg catalog and (optionally) Ranger-enforced.

**Architecture:** A new `StarRocksProvisioner` (impl `EngineProvisioner`) templates a `StarRocksCluster` CR + FE/Ranger `ConfigMap`s and applies them through egret's existing CR-capable `KubeClient`. `EngineSpec` gains an engine `kind` + StarRocks config; the egret-bin provisioning-port adapter dispatches Trino vs StarRocks. The operator is pre-installed (not egret-managed).

**Tech Stack:** Rust (egret-orchestrator, egret-engine-mgmt(-api), egret-bin), kube-rs 0.99 DynamicObject apply, StarRocks Kubernetes Operator (`data-portal/starrocks-kubernetes-operator/`), StarRocks shared-data mode, kind, kubectl, MinIO.

**Spec:** `docs/en/specs/2026-07-16-starrocks-provisioning-design.md`.

**Reference to mirror:** `crates/egret-orchestrator/src/trino.rs` (templating fns + `#[cfg(test)]` JSON assertions with a fake `KubeClient`), and the Trino create path in `crates/egret-engine-mgmt/src/engines.rs`.

---

## File Structure

- **Create** `deploy/starrocks-operator/` — vendored `operator.yaml` + `starrocks.com_starrocksclusters.yaml` (copied from `data-portal/starrocks-kubernetes-operator/deploy/`) + install notes in `deploy/starrocks/README.md`.
- **Modify** `crates/egret-engine-mgmt-api/src/lib.rs` — `EngineKind` enum; `StarRocksConfig`; `EngineSpec { kind, starrocks }`; `EngineProvisioningPort::{status,delete,restart}` gain `kind: &str`.
- **Create** `crates/egret-orchestrator/src/starrocks.rs` — `StarRocksProvisioner` + `starrocks_cluster_cr` / `starrocks_fe_configmap` / `starrocks_ranger_configmap` + tests.
- **Modify** `crates/egret-orchestrator/src/lib.rs` — `pub mod starrocks; pub use starrocks::StarRocksProvisioner;`.
- **Modify** `crates/egret-bin/src/main.rs` — build both provisioners into the adapter; adapter dispatches by kind.
- **Modify** `crates/egret-engine-mgmt/src/engines.rs` — `create_engine("starrocks")` path + StarRocks config surface + pass `&engine.r#type` to status/delete/restart + auto-wire Iceberg.
- **Modify** `frontend/src/routes/control-plane/` engine create/detail — a StarRocks config form (largely engine-type-agnostic).

---

## Task 1: Vendor + install the StarRocks operator

**Files:** Create `deploy/starrocks-operator/operator.yaml`, `deploy/starrocks-operator/crd.yaml`; modify `deploy/starrocks/README.md`.

- [ ] **Step 1: Vendor the manifests.** Copy verbatim:
  - `data-portal/starrocks-kubernetes-operator/deploy/operator.yaml` → `deploy/starrocks-operator/operator.yaml`
  - `data-portal/starrocks-kubernetes-operator/deploy/starrocks.com_starrocksclusters.yaml` → `deploy/starrocks-operator/crd.yaml`

- [ ] **Step 2: Document install** in `deploy/starrocks/README.md` — a "Provisioning (Kubernetes operator)" section:
  ```bash
  kubectl apply -f deploy/starrocks-operator/crd.yaml
  kubectl apply -f deploy/starrocks-operator/operator.yaml   # installs into ns `starrocks`
  kubectl -n starrocks rollout status deploy/kube-starrocks-operator
  ```
  Note: the operator is pre-installed once (like the Stackable Trino operator); egret does not manage its lifecycle. Provisioned clusters land in the engines namespace egret already uses.

- [ ] **Step 3: Install into the dev kind cluster** (controller does this at smoke time; not a code step). Verify `kubectl get crd starrocksclusters.starrocks.com` succeeds.

- [ ] **Step 4: Commit.** `git add deploy/starrocks-operator deploy/starrocks/README.md && git commit -m "chore(deploy): vendor + document the StarRocks Kubernetes operator"`

---

## Task 2: Engine-kind + StarRocks config contract

**Files:** Modify `crates/egret-engine-mgmt-api/src/lib.rs`.

- [ ] **Step 1: Write failing tests.** Append to the crate's `#[cfg(test)] mod tests` (or add one):
  ```rust
  #[test]
  fn engine_kind_parses_and_defaults_trino() {
      assert_eq!(EngineKind::default(), EngineKind::Trino);
      assert_eq!(EngineKind::parse("starrocks"), Some(EngineKind::StarRocks));
      assert_eq!(EngineKind::parse("trino"), Some(EngineKind::Trino));
      assert_eq!(EngineKind::StarRocks.as_str(), "starrocks");
  }

  #[test]
  fn engine_spec_carries_starrocks_config() {
      let s = StarRocksConfig {
          fe_image: "starrocks/fe-ubuntu:3.3-latest".into(),
          cn_image: "starrocks/cn-ubuntu:3.3-latest".into(),
          fe_replicas: 1,
          cn_replicas: 1,
          storage_endpoint: "http://host.docker.internal:9000".into(),
          storage_path: "warehouse/starrocks".into(),
          storage_access_key: "egret".into(),
          storage_secret_key: "egretsecret".into(),
          resources: None,
      };
      assert_eq!(s.fe_replicas, 1);
  }
  ```

- [ ] **Step 2: Run → FAIL** (`EngineKind`/`StarRocksConfig` undefined). `cargo test -p egret-engine-mgmt-api engine_kind_ engine_spec_carries`.

- [ ] **Step 3: Add the types.** Before `EngineSpec` (~line 378):
  ```rust
  /// Which query engine an `EngineSpec` describes. Selects the provisioner.
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
  pub enum EngineKind {
      #[default]
      Trino,
      StarRocks,
  }
  impl EngineKind {
      pub fn as_str(self) -> &'static str {
          match self { EngineKind::Trino => "trino", EngineKind::StarRocks => "starrocks" }
      }
      pub fn parse(s: &str) -> Option<EngineKind> {
          match s { "trino" => Some(EngineKind::Trino), "starrocks" => Some(EngineKind::StarRocks), _ => None }
      }
  }

  /// StarRocks shared-data engine config (FE + CN). Storage params drive the
  /// FE builtin storage volume (object store); resources default when None.
  #[derive(Clone, Debug)]
  pub struct StarRocksConfig {
      pub fe_image: String,   // starrocks/fe-ubuntu:<ver>
      pub cn_image: String,   // starrocks/cn-ubuntu:<ver>
      pub fe_replicas: u32,
      pub cn_replicas: u32,
      pub storage_endpoint: String,   // aws_s3_endpoint
      pub storage_path: String,       // aws_s3_path = bucket/prefix
      pub storage_access_key: String,
      pub storage_secret_key: String,
      pub resources: Option<RoleResources>,
  }
  ```

- [ ] **Step 4: Extend `EngineSpec`.** Add two fields (after `ranger`):
  ```rust
      /// Engine type — selects the provisioner. Defaults to Trino.
      pub kind: EngineKind,
      /// Present when `kind == StarRocks`; the Trino `config`/`catalogs` are unused then.
      pub starrocks: Option<StarRocksConfig>,
  ```
  Because `EngineSpec` has no `#[derive(Default)]`, every constructor must set them. Grep `EngineSpec {` (currently only `trino.rs:1329` `sample_spec` + `1732` `jwt_tls_spec`, plus the `req` builder in `engines.rs`). Add `kind: EngineKind::Trino, starrocks: None` to each.

- [ ] **Step 5: Add `kind` to the Port trait's stateless methods.** Change `EngineProvisioningPort` (~line 411):
  ```rust
      async fn status(&self, kind: &str, namespace: &str, name: &str) -> Result<EngineStatus, String>;
      async fn update(&self, req: &EngineSpec) -> Result<(), String>;
      async fn delete(&self, kind: &str, namespace: &str, name: &str) -> Result<(), String>;
      async fn restart(&self, kind: &str, namespace: &str, name: &str) -> Result<(), String>;
  ```
  (`provision`/`update` already carry `kind` inside `EngineSpec`.)

- [ ] **Step 6: Run → PASS** (`cargo test -p egret-engine-mgmt-api`). The Port signature change breaks the adapter + engine-mgmt call sites — fixed in Tasks 4/5; **build of dependent crates will fail until then** (expected; keep this task's own crate green).

- [ ] **Step 7: Commit.** `feat(engine-api): EngineKind + StarRocksConfig + kind on the provisioning port`.

---

## Task 3: `StarRocksProvisioner` + CR/ConfigMap templating

**Files:** Create `crates/egret-orchestrator/src/starrocks.rs`; modify `crates/egret-orchestrator/src/lib.rs`.

Mirror `trino.rs`: pure templating fns returning `serde_json::Value`, unit-tested, plus a provisioner that `KubeClient::apply`s them.

- [ ] **Step 1: Write failing tests** (`starrocks.rs` `#[cfg(test)]`). Use a `sample_sr_spec()` helper (an `EngineSpec` with `kind: StarRocks`, `starrocks: Some(StarRocksConfig{..})`, `ranger: None`).
  ```rust
  #[test]
  fn cr_has_fe_and_cn_shared_data() {
      let cr = starrocks_cluster_cr(&sample_sr_spec());
      assert_eq!(cr["apiVersion"], "starrocks.com/v1");
      assert_eq!(cr["kind"], "StarRocksCluster");
      let spec = &cr["spec"];
      assert_eq!(spec["starRocksFeSpec"]["image"], "starrocks/fe-ubuntu:3.3-latest");
      assert_eq!(spec["starRocksFeSpec"]["configMapInfo"]["resolveKey"], "fe.conf");
      assert_eq!(spec["starRocksCnSpec"]["image"], "starrocks/cn-ubuntu:3.3-latest");
      // no BE in shared-data
      assert!(spec.get("starRocksBeSpec").is_none());
  }

  #[test]
  fn fe_configmap_has_shared_data_storage_and_ports() {
      let cm = starrocks_fe_configmap(&sample_sr_spec());
      let conf = cm["data"]["fe.conf"].as_str().unwrap();
      assert!(conf.contains("run_mode = shared_data"));
      assert!(conf.contains("query_port = 9030"));
      assert!(conf.contains("cloud_native_storage_type = S3"));
      assert!(conf.contains("aws_s3_endpoint = http://host.docker.internal:9000"));
      assert!(conf.contains("aws_s3_enable_path_style_access = true"));
      assert!(conf.contains("enable_load_volume_from_conf = true"));
      // no ranger unless bound
      assert!(!conf.contains("access_control = ranger"));
  }

  #[test]
  fn ranger_binding_adds_access_control_and_xml_configmap() {
      let mut spec = sample_sr_spec();
      spec.ranger = Some(RangerBinding { service_name: "sr1".into(),
          egret_url: "http://host.docker.internal:8080".into(), poll_interval_ms: 30_000 });
      let conf = starrocks_fe_configmap(&spec)["data"]["fe.conf"].as_str().unwrap().to_string();
      assert!(conf.contains("access_control = ranger"));
      let xml_cm = starrocks_ranger_configmap(&spec).unwrap();
      let xml = xml_cm["data"]["ranger-starrocks-security.xml"].as_str().unwrap();
      assert!(xml.contains("<name>ranger.plugin.starrocks.service.name</name>"));
      assert!(xml.contains("<value>sr1</value>"));
      assert!(xml.contains("http://host.docker.internal:8080"));
      // the CR mounts the xml configmap on the FE
      let cr = starrocks_cluster_cr(&spec);
      let mounts = cr["spec"]["starRocksFeSpec"]["configMaps"].as_array().unwrap();
      assert!(mounts.iter().any(|m| m["name"] == xml_cm["metadata"]["name"]));
  }

  #[test]
  fn no_ranger_binding_omits_xml_configmap() {
      assert!(starrocks_ranger_configmap(&sample_sr_spec()).is_none());
  }
  ```

- [ ] **Step 2: Run → FAIL.** `cargo test -p egret-orchestrator starrocks`.

- [ ] **Step 3: Implement the templating fns** in `starrocks.rs`. Read `data-portal/starrocks-kubernetes-operator/examples/starrocks/deploy_a_starrocks_cluster_running_in_shared_data_mode.yaml` for the exact shape. Key structure:
  - `starrocks_fe_configmap(spec) -> Value`: a `v1/ConfigMap` named `<name>-fe-cm`, `data["fe.conf"]` = the shared-data conf string built from `spec.starrocks` (ports; `run_mode = shared_data`; `cloud_native_meta_port = 6090`; the `aws_s3_*` block from `storage_*`; `enable_load_volume_from_conf = true`; append `access_control = ranger` when `spec.ranger.is_some()`).
  - `starrocks_ranger_configmap(spec) -> Option<Value>`: `Some` only when `spec.ranger.is_some()`; a `v1/ConfigMap` named `<name>-ranger-cm` with `data["ranger-starrocks-security.xml"]` (the same XML slice-1 shipped — `ranger.plugin.starrocks.service.name/policy.rest.url/source.impl/pollIntervalMs`, values from `spec.ranger`) + `data["ranger-starrocks-audit.xml"]` (`xasecure.audit.is.enabled=false`). **Keep XML comments free of any `--` sequence** (slice-1 finding: doubled hyphen in an XML comment crashes the StarRocks FE boot).
  - `starrocks_cluster_cr(spec) -> Value`: `starrocks.com/v1 StarRocksCluster` named `spec.name`, `spec.starRocksFeSpec` (image=`fe_image`, replicas=`fe_replicas`, `configMapInfo:{configMapName:"<name>-fe-cm", resolveKey:"fe.conf"}`, and when ranger `configMaps:[{name:"<name>-ranger-cm", mountPath:"<FE conf dir>"}]`), `spec.starRocksCnSpec` (image=`cn_image`, replicas=`cn_replicas`). Apply `resources` (limits/requests) when `Some`.

  Emit via `serde_json::json!`. Add `use egret_engine_mgmt_api::{EngineSpec, RangerBinding, ...};`.

- [ ] **Step 4: Implement `StarRocksProvisioner`.** Mirror `TrinoProvisioner`'s struct (holds `Arc<dyn KubeClient>`):
  ```rust
  pub struct StarRocksProvisioner { kube: std::sync::Arc<dyn KubeClient> }
  #[async_trait::async_trait]
  impl EngineProvisioner for StarRocksProvisioner {
      async fn provision(&self, spec: &EngineSpec) -> anyhow::Result<()> {
          self.kube.apply(&starrocks_fe_configmap(spec)).await?;
          if let Some(cm) = starrocks_ranger_configmap(spec) { self.kube.apply(&cm).await?; }
          self.kube.apply(&starrocks_cluster_cr(spec)).await?;
          Ok(())
      }
      async fn update(&self, spec: &EngineSpec) -> anyhow::Result<()> { self.provision(spec).await }
      async fn status(&self, namespace: &str, name: &str) -> anyhow::Result<EngineStatus> {
          // list pods labelled for this StarRocksCluster; map FE+CN readiness.
          // Mirror TrinoProvisioner::status pod-list + PodStatus mapping; the
          // operator labels pods `app.kubernetes.io/component=fe|cn` +
          // `app.starrocks.ownerreference/name=<name>`. Verify labels at smoke.
      }
      async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
          self.kube.delete("starrocks.com/v1", "StarRocksCluster", namespace, name).await?;
          // best-effort delete the two ConfigMaps
          let _ = self.kube.delete("v1", "ConfigMap", namespace, &format!("{name}-fe-cm")).await;
          let _ = self.kube.delete("v1", "ConfigMap", namespace, &format!("{name}-ranger-cm")).await;
          Ok(())
      }
  }
  ```
  For `status`, read `TrinoProvisioner::status` and reuse its pod-list helper on the `KubeClient`; label selector is the one the operator stamps (confirm at smoke — Task 8 fallback covers a mismatch).

- [ ] **Step 5: Export.** In `lib.rs`: `pub mod starrocks;` + `pub use starrocks::StarRocksProvisioner;`.

- [ ] **Step 6: Run → PASS.** `cargo test -p egret-orchestrator starrocks`. Then `cargo test -p egret-orchestrator` (existing trino tests still green after the `EngineSpec` field additions).

- [ ] **Step 7: Commit.** `feat(orchestrator): StarRocksProvisioner + shared-data CR/ConfigMap templating`.

---

## Task 4: Provisioner-registry dispatch (adapter)

**Files:** Modify `crates/egret-bin/src/main.rs`.

- [ ] **Step 1: Hold both provisioners in the adapter.** In `EgretEngineProvisioningPort` (struct near main.rs:472), replace the single inner provisioner with two:
  ```rust
  struct EgretEngineProvisioningPort {
      trino: std::sync::Arc<egret_orchestrator::TrinoProvisioner>,
      starrocks: std::sync::Arc<egret_orchestrator::StarRocksProvisioner>,
      kube: std::sync::Arc<dyn egret_orchestrator::KubeClient>,
  }
  impl EgretEngineProvisioningPort {
      fn for_kind(&self, kind: &str) -> &dyn egret_orchestrator::EngineProvisioner {
          match kind { "starrocks" => &*self.starrocks, _ => &*self.trino }
      }
  }
  ```

- [ ] **Step 2: Dispatch in each Port method.** Update the `impl EngineProvisioningPort for EgretEngineProvisioningPort` (main.rs:472):
  - `provision(&self, req)` → `self.for_kind(req.kind.as_str()).provision(req).await.map_err(|e| e.to_string())`
  - `update(&self, req)` → `self.for_kind(req.kind.as_str()).update(req)...`
  - `status(&self, kind, ns, name)` → `self.for_kind(kind).status(ns, name)...`
  - `delete(&self, kind, ns, name)` → `self.for_kind(kind).delete(ns, name)...`
  - `restart(&self, kind, ns, name)` → for `"starrocks"`, restart = re-apply (or delete+reprovision is out of scope); for MVP, StarRocks `restart` may return `Ok(())` with a "not supported; use update" — or roll the CN Deployment via the operator. Keep Trino's behavior unchanged.

- [ ] **Step 3: Construct both** where the adapter is built (main.rs ~1297):
  ```rust
  let trino = std::sync::Arc::new(egret_orchestrator::TrinoProvisioner::new(kube.clone()));
  let starrocks = std::sync::Arc::new(egret_orchestrator::StarRocksProvisioner::new(kube.clone()));
  std::sync::Arc::new(EgretEngineProvisioningPort { trino, starrocks, kube })
      as std::sync::Arc<dyn egret_engine_mgmt_api::EngineProvisioningPort>
  ```
  (Add `StarRocksProvisioner::new(kube)` in Task 3 if not already.)

- [ ] **Step 4: Build.** `cargo build -p egret-bin` → PASS (compiles once Task 5 fixes the engine-mgmt call sites; if building bin alone still fails on engine-mgmt, do Task 5 first then build). Commit.

- [ ] **Step 5: Commit.** `feat(bin): dispatch provisioning by engine kind (trino|starrocks)`.

---

## Task 5: `create_engine("starrocks")` + config surface + call-site kinds

**Files:** Modify `crates/egret-engine-mgmt/src/engines.rs`.

- [ ] **Step 1: Pass `kind` to the stateless Port calls.** At the three sites, thread `&engine.r#type` (the engine row's type column):
  - `status` (engines.rs:1824): `prov.status(&engine.r#type, &engine.namespace, &engine.name)`
  - `delete` (engines.rs:1960): `prov.delete(&engine.r#type, &engine.namespace, &engine.name)`
  - `restart` (engines.rs:2006): `prov.restart(&engine.r#type, &engine.namespace, &engine.name)`
  (Confirm the field name — the engine row struct exposes the type as `r#type` or `kind`; grep the row struct. Use whichever exists.)

- [ ] **Step 2: Add the StarRocks create path.** The current create handler builds a Trino `EngineSpec` (`req`) and calls `create_engine(&b.name, "trino", ns, wh.id, &spec)` (engines.rs:1634) then `prov.provision(&req)` (1685). Add a StarRocks branch (gated on a `kind`/`engine` field in the create body, defaulting to trino for back-compat):
  - Build `StarRocksConfig` from the request (fe/cn image + replicas + resources) with storage params defaulted from egret's storage config (`st` should expose the storage endpoint/bucket/keys the Trino auto-wire already uses; reuse them). Endpoint for kind pods = the same reachable base used for `engine_egret_url`'s host (e.g. `http://host.docker.internal:9000`).
  - Build `EngineSpec { name, namespace, image_version: <fe ver>, config: TrinoConfig::default(), catalogs: vec![], ranger: <bound profile or None>, kind: EngineKind::StarRocks, starrocks: Some(cfg) }`.
  - `create_engine(&b.name, "starrocks", &namespace, wh.id, &spec_json)` (persist with type `"starrocks"`), then `prov.provision(&req)`.
  - The Ranger `spec.ranger` is populated the same way Task-3/P3.2 did for Trino: from the bound profile name + `engine_egret_url` (reuse the existing `RangerBinding` resolution).

- [ ] **Step 3: Config surface.** Add a `StarRocksConfigBody` (fe/cn image version, replicas, resources) to the create request, mirroring how `trino_config` is accepted; validate images non-empty. Keep the wizard simple (image + sizes); storage is derived.

- [ ] **Step 4: Build + engine-mgmt tests.** `cargo test -p egret-engine-mgmt` → PASS (existing Trino create/status/delete tests still green with the threaded `kind`). Add a create test asserting a `"starrocks"`-type engine row is persisted (mirror the Trino create test; the fake provisioning port accepts any kind).

- [ ] **Step 5: Commit.** `feat(engine-mgmt): create + manage StarRocks engines (shared-data)`.

---

## Task 6: Auto-wire the Iceberg external catalog

**Files:** Modify `crates/egret-engine-mgmt/src/engines.rs` (post-provision hook for StarRocks).

Trino auto-wires via catalog *files* in the ConfigMap; StarRocks needs a runtime SQL step once the FE is reachable.

- [ ] **Step 1: Add a post-Ready external-catalog step.** After a StarRocks engine is provisioned and its FE is Ready (poll `status` until FE pod ready, bounded), connect to the FE (MySQL protocol, `StarRocksConnector`/mysql_async, service DNS `<name>-fe-service.<ns>:9030` or the assignment surface) and run:
  ```sql
  CREATE EXTERNAL CATALOG IF NOT EXISTS <warehouse>
  PROPERTIES ("type"="iceberg","iceberg.catalog.type"="rest",
             "iceberg.catalog.uri"="<engine_egret_url>/catalog",
             "iceberg.catalog.warehouse"="<warehouse>",
             "iceberg.catalog.token"="<minted engine token>");
  ```
  Reuse `engine_egret_url` + the per-engine catalog token already minted in the Trino create path (engines.rs mints a client/token before provisioning). Make this best-effort + logged (a failed auto-wire shouldn't fail the whole create — surface a warning; the admin can retry).

- [ ] **Step 2: Guard + test.** Unit-test the SQL builder (the `PROPERTIES` string) in isolation; the live connection is smoke-only. `cargo test -p egret-engine-mgmt`.

- [ ] **Step 3: Commit.** `feat(engine-mgmt): auto-wire Iceberg external catalog on provisioned StarRocks`.

---

## Task 7: Control-plane FE — StarRocks engine form

**Files:** Modify `frontend/src/routes/control-plane/` (engine create + detail).

- [ ] **Step 1: Engine-type selector on create.** The engine create flow is Trino-shaped; add an engine-kind choice (`Trino` | `StarRocks`). For StarRocks, show a minimal form: FE image version, CN image version, FE/CN replicas, resources (reuse existing inputs). Hide Trino-only sections (catalogs/coordinator-worker/resource-groups) when kind=StarRocks.

- [ ] **Step 2: List/detail.** Ensure the engines list + detail render a StarRocks engine (type badge, pods = FE/CN). Most of this is engine-type-agnostic; verify the detail page doesn't assume coordinator/worker.

- [ ] **Step 3: Regenerate the API client** if the create body changed (`cd frontend && npm run gen:api`).

- [ ] **Step 4: Typecheck + tests.** `cd frontend && npm run typecheck && npx vitest run src/routes/control-plane`. Add a test for the StarRocks create form (mirror the Trino one).

- [ ] **Step 5: Commit.** `feat(fe): provision StarRocks engines from the control plane`.

---

## Task 8: Gate + LIVE SMOKE (controller-driven)

- [ ] **Step 1: Full CI locally.** `cargo fmt --all`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` (DATABASE_URL=egret_test; the 2 pre-existing `egret-engine-mgmt/assignment.rs` failures are unrelated); `cd frontend && npm run lint && npm run typecheck && npx vitest run && npm run build` (the `useEngineConfigForm.dirty.test.tsx` typecheck error is pre-existing).

- [ ] **Step 2: Install the operator** into kind (Task 1 Step 3). Ensure MinIO reachable from cluster pods (`host.docker.internal:9000`) and a `warehouse` bucket exists.

- [ ] **Step 3: Provision a StarRocks engine** via the control-plane UI/API bound to the egret warehouse. Watch `kubectl -n <engines-ns> get pods -w` for FE+CN Ready. Confirm the `StarRocksCluster` CR + FE/ranger ConfigMaps exist (`kubectl get starrockscluster,cm`).

- [ ] **Step 4: Confirm shared-data + catalog.** FE log shows the builtin storage volume created on MinIO; `SHOW STORAGE VOLUMES;` + `SHOW CATALOGS;` (the auto-wired external Iceberg catalog present).

- [ ] **Step 5: Query proof.** Against the provisioned FE, `SELECT` from a seeded Iceberg table via the external catalog → rows. If bound to a policy profile, run the slice-1 masked/denied checks on the *provisioned* cluster to confirm Ranger still enforces.

- [ ] **Step 6: Report** exactly what ran (live queries, not tests). Fold any label-selector / mount-path / storage-bootstrap fixes back into the code + README.

**Fallback:** if FE won't bootstrap the volume, switch to a runtime `CREATE STORAGE VOLUME … TYPE=S3` step (documented). If `status()` shows no pods, fix the operator label selector (check `kubectl -n <ns> get pods --show-labels` on the running cluster). If the Ranger XML isn't read, confirm the `configMaps` mountPath is the FE conf dir on the `fe-ubuntu` image.

---

## Self-Review

- **Spec coverage:** operator install → T1; kind+config contract → T2; provisioner+templating → T3; registry dispatch → T4; create+config surface+call-site kinds → T5; auto-wire Iceberg → T6; FE → T7; Ranger relocation → T2/T3 (`spec.ranger` → FE ConfigMap); gate+smoke → T8. Covered.
- **Type consistency:** `EngineKind`/`StarRocksConfig` (T2) used identically by the provisioner (T3), adapter (T4), and create path (T5); `EngineProvisioningPort::{status,delete,restart}(kind, …)` new signature threaded in T4 (adapter) + T5 (call sites); CR/ConfigMap names (`<name>-fe-cm`/`-ranger-cm`) consistent between templating (T3) and delete (T3) + CR mount (T3).
- **No placeholders:** CR/ConfigMap JSON shape, shared-data `fe.conf` keys, the four dispatch methods, the three engine-mgmt call-site line anchors, and the `CREATE EXTERNAL CATALOG` SQL are concrete. Runtime-verified values (operator pod label selector, `fe-ubuntu` conf dir) are explicitly deferred to the T8 smoke with fallbacks — mirroring how P3.2's XML keys were validated live.
- **Ambiguity:** provisioning is pinned to shared-data FE+CN via the operator; BE/HA/autoscaling are out; the `restart` semantics for StarRocks are called out as MVP-limited.
