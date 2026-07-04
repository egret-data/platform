# Trino Pod & Scheduling Config (Phase 1.5.3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add K8s pod-placement + pod-shape config (nodeSelector/tolerations/affinity/topologySpread/serviceAccount/securityContext/init+sidecar containers/mounts/strategy/…) to the provisioned Trino Deployments, surfaced in a new **Pod & Scheduling** Configuration sub-tab.

**Architecture:** New per-role fields on `RoleConfig` + cluster-wide fields on `TrinoConfig` in egret-orchestrator; `trino_deployment` splices them into the podSpec/container/Deployment (preserving the worker graceful-shutdown termination/lifecycle). Kube-free carriers in egret-controlplane, `spec_from` adapter in egret-bin, and a `PodSchedulingSection` frontend component wired into `useEngineConfigForm`. Deeply-nested objects (affinity, init/sidecar containers, securityContext) use a **structured-common + raw-JSON override** — `raw_override` (a `serde_json::Value`) wins verbatim.

**Tech Stack:** Rust (serde_json), kube manifests, React 18 + shadcn + Tailwind v4.

**Spec:** `docs/en/specs/2026-07-04-trino-pod-scheduling-design.md`.
**Chart ref:** `/Users/1pro-l4r8/Documents/github/data-portal/trino-charts/charts/trino`.

---

## File Structure

- **Modify** `crates/egret-orchestrator/src/spec.rs` — new structs + `RoleConfig`/`TrinoConfig` fields.
- **Modify** `crates/egret-orchestrator/src/trino.rs` — `trino_deployment` splicing + helpers (`affinity_json`, `security_context_json`, `container_json`).
- **Modify** `crates/egret-controlplane/src/engines.rs` — carriers + `build_provision_req` mapping.
- **Modify** `crates/egret-bin/src/main.rs` — `spec_from` mapping.
- **Modify** `frontend/src/api/controlPlane.ts` + `useEngineConfigForm.ts`; **Create** `frontend/src/routes/control-plane/PodSchedulingSection.tsx`; **Modify** `EngineDetail.tsx` (add the sub-tab).

---

## Task 1: Orchestrator types

**Files:** Modify `crates/egret-orchestrator/src/spec.rs`, `lib.rs`

- [ ] **Step 1** Add these structs (all `#[derive(Clone, Debug, Default)]`; `Value` = `serde_json::Value`):

```rust
#[derive(Clone, Debug, Default)]
pub struct Toleration { pub key: Option<String>, pub operator: Option<String>, pub value: Option<String>, pub effect: Option<String>, pub toleration_seconds: Option<i64> }
#[derive(Clone, Debug, Default)]
pub struct TopologySpreadConstraint { pub max_skew: u32, pub topology_key: String, pub when_unsatisfiable: String, pub match_labels: BTreeMap<String, String> }
#[derive(Clone, Debug, Default)]
pub struct NodeSelectorRequirement { pub key: String, pub operator: String, pub values: Vec<String> }
#[derive(Clone, Debug, Default)]
pub struct Affinity { pub node_match: Vec<NodeSelectorRequirement>, pub pod_anti_affinity_topology_key: Option<String>, pub raw_override: Option<Value> }
#[derive(Clone, Debug, Default)]
pub struct ContainerPort { pub name: String, pub container_port: u16 }
#[derive(Clone, Debug, Default)]
pub struct SecurityContext { pub run_as_user: Option<i64>, pub run_as_group: Option<i64>, pub fs_group: Option<i64>, pub run_as_non_root: Option<bool>, pub raw_override: Option<Value> }
#[derive(Clone, Debug, Default)]
pub struct VolumeMount { pub name: String, pub mount_path: String, pub sub_path: Option<String> }
#[derive(Clone, Debug, Default)]
pub struct MiniContainer { pub name: String, pub image: String, pub command: Vec<String>, pub args: Vec<String>, pub env: BTreeMap<String, String>, pub volume_mounts: Vec<VolumeMount>, pub raw_override: Option<Value> }
#[derive(Clone, Debug, Default)]
pub struct EnvFromSource { pub config_map_ref: Option<String>, pub secret_ref: Option<String> }
#[derive(Clone, Debug, Default)]
pub struct ObjectMount { pub name: String, pub mount_path: String, pub sub_path: Option<String>, pub default_mode: Option<i32> }
#[derive(Clone, Debug, Default)]
pub struct DeploymentStrategy { pub kind: String, pub max_surge: Option<String>, pub max_unavailable: Option<String> }
```

- [ ] **Step 2** Add to `RoleConfig`: `pub node_selector: BTreeMap<String,String>`, `pub tolerations: Vec<Toleration>`, `pub topology_spread: Vec<TopologySpreadConstraint>`, `pub affinity: Option<Affinity>`, `pub termination_grace_seconds: Option<u32>`, `pub additional_exposed_ports: Vec<ContainerPort>`.
- [ ] **Step 3** Add to `TrinoConfig`: `pub image_pull_secrets: Vec<String>`, `pub image_pull_policy: Option<String>`, `pub service_account: Option<String>`, `pub pod_security_context: Option<SecurityContext>`, `pub container_security_context: Option<SecurityContext>`, `pub init_containers: Vec<MiniContainer>`, `pub sidecar_containers: Vec<MiniContainer>`, `pub env_from: Vec<EnvFromSource>`, `pub config_mounts: Vec<ObjectMount>`, `pub secret_mounts: Vec<ObjectMount>`, `pub deployment_strategy: Option<DeploymentStrategy>`, `pub deployment_annotations: BTreeMap<String,String>`, `pub revision_history_limit: Option<u32>`, `pub progress_deadline_seconds: Option<u32>`, `pub share_process_namespace: Option<bool>`.
- [ ] **Step 4** Re-export all new structs from `lib.rs`. `cargo build -p egret-orchestrator` (additive — trino.rs unaffected). `cargo fmt --all`.
- [ ] **Step 5** Commit `refactor(orchestrator): pod/scheduling config types`.

---

## Task 2: Render per-role placement (TDD)

**Files:** Modify `crates/egret-orchestrator/src/trino.rs`

Study `trino_deployment` first: it builds `container` (a `serde_json::Value`) then `pod_spec` (an object with `containers`/`volumes`, plus worker graceful-shutdown `terminationGracePeriodSeconds`/`lifecycle`), then the Deployment. Add a helper + splice per-role fields from the role's config (`cfg` = coordinator or worker `RoleConfig`).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn per_role_placement_renders() {
    let mut s = sample_spec();
    let c = &mut s.config.coordinator;
    c.node_selector.insert("disktype".into(), "ssd".into());
    c.tolerations = vec![Toleration { key: Some("dedicated".into()), operator: Some("Equal".into()), value: Some("trino".into()), effect: Some("NoSchedule".into()), toleration_seconds: None }];
    c.topology_spread = vec![TopologySpreadConstraint { max_skew: 1, topology_key: "kubernetes.io/hostname".into(), when_unsatisfiable: "ScheduleAnyway".into(), match_labels: [("app".to_string(), "x".to_string())].into() }];
    c.termination_grace_seconds = Some(60);
    c.additional_exposed_ports = vec![ContainerPort { name: "jmx".into(), container_port: 9080 }];
    let d = trino_deployment(&s, Role::Coordinator);
    let ps = &d["spec"]["template"]["spec"];
    assert_eq!(ps["nodeSelector"]["disktype"], "ssd");
    assert_eq!(ps["tolerations"][0]["key"], "dedicated");
    assert_eq!(ps["tolerations"][0]["effect"], "NoSchedule");
    assert_eq!(ps["topologySpreadConstraints"][0]["maxSkew"], 1);
    assert_eq!(ps["terminationGracePeriodSeconds"], 60);
    let ports = &ps["containers"][0]["ports"];
    assert!(ports.as_array().unwrap().iter().any(|p| p["name"] == "jmx" && p["containerPort"] == 9080));
}
#[test]
fn affinity_structured_and_raw_override() {
    let mut s = sample_spec();
    s.config.coordinator.affinity = Some(Affinity {
        node_match: vec![NodeSelectorRequirement { key: "role".into(), operator: "In".into(), values: vec!["trino".into()] }],
        pod_anti_affinity_topology_key: Some("kubernetes.io/hostname".into()), raw_override: None });
    let ps = trino_deployment(&s, Role::Coordinator)["spec"]["template"]["spec"].clone();
    assert_eq!(ps["affinity"]["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]["nodeSelectorTerms"][0]["matchExpressions"][0]["key"], "role");
    assert_eq!(ps["affinity"]["podAntiAffinity"]["preferredDuringSchedulingIgnoredDuringExecution"][0]["podAffinityTerm"]["topologyKey"], "kubernetes.io/hostname");
    // raw override wins verbatim
    let mut s2 = sample_spec();
    s2.config.coordinator.affinity = Some(Affinity { node_match: vec![], pod_anti_affinity_topology_key: None, raw_override: Some(serde_json::json!({"custom": true})) });
    assert_eq!(trino_deployment(&s2, Role::Coordinator)["spec"]["template"]["spec"]["affinity"]["custom"], true);
}
```

- [ ] **Step 2** Run → FAIL.

- [ ] **Step 3: Implement** — add `fn affinity_json(a: &Affinity) -> Value`:

```rust
fn affinity_json(a: &crate::spec::Affinity, app_label: &str) -> Value {
    if let Some(raw) = &a.raw_override { return raw.clone(); }
    let mut out = serde_json::Map::new();
    if !a.node_match.is_empty() {
        let exprs: Vec<Value> = a.node_match.iter().map(|r| json!({ "key": r.key, "operator": r.operator, "values": r.values })).collect();
        out.insert("nodeAffinity".into(), json!({ "requiredDuringSchedulingIgnoredDuringExecution": { "nodeSelectorTerms": [{ "matchExpressions": exprs }] } }));
    }
    if let Some(tk) = &a.pod_anti_affinity_topology_key {
        out.insert("podAntiAffinity".into(), json!({ "preferredDuringSchedulingIgnoredDuringExecution": [{
            "weight": 100, "podAffinityTerm": { "topologyKey": tk, "labelSelector": { "matchLabels": { "app": app_label } } } }] }));
    }
    Value::Object(out)
}
```

  Then in `trino_deployment`, with `cfg = the role's RoleConfig` and `app_label = format!("{}-{}", spec.name, role_str)`, after building `pod_spec`, insert (only when non-empty/some):
  - `pod_spec["nodeSelector"]` = map → object; `pod_spec["tolerations"]` = array of `{key?,operator?,value?,effect?,tolerationSeconds?}` (omit `None` fields); `pod_spec["topologySpreadConstraints"]` = array of `{maxSkew,topologyKey,whenUnsatisfiable,labelSelector:{matchLabels}}`; `pod_spec["affinity"] = affinity_json(a, &app_label)`.
  - `termination_grace_seconds`: set `pod_spec["terminationGracePeriodSeconds"]` **only if not already set** by worker graceful shutdown (check `pod_spec.get("terminationGracePeriodSeconds").is_none()`).
  - `additional_exposed_ports`: append to `container["ports"]` (create the array if absent — the container may already have a base http port; APPEND, don't replace).

- [ ] **Step 4** Run tests + clippy + fmt. Commit `feat(orchestrator): per-role pod placement (nodeSelector/tolerations/affinity/topology/ports)`.

---

## Task 3: Render cluster pod-level + Deployment (TDD)

**Files:** Modify `crates/egret-orchestrator/src/trino.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn cluster_pod_and_deployment_fields() {
    let mut s = sample_spec();
    s.config.service_account = Some("trino-sa".into());
    s.config.image_pull_secrets = vec!["regcred".into()];
    s.config.image_pull_policy = Some("Always".into());
    s.config.share_process_namespace = Some(true);
    s.config.pod_security_context = Some(SecurityContext { run_as_user: Some(1000), fs_group: Some(1000), ..Default::default() });
    s.config.container_security_context = Some(SecurityContext { run_as_non_root: Some(true), raw_override: Some(json!({"readOnlyRootFilesystem": true})), ..Default::default() });
    s.config.deployment_strategy = Some(DeploymentStrategy { kind: "Recreate".into(), max_surge: None, max_unavailable: None });
    s.config.deployment_annotations.insert("team".into(), "data".into());
    s.config.revision_history_limit = Some(3);
    s.config.progress_deadline_seconds = Some(600);
    let d = trino_deployment(&s, Role::Coordinator);
    let ps = &d["spec"]["template"]["spec"];
    assert_eq!(ps["serviceAccountName"], "trino-sa");
    assert_eq!(ps["imagePullSecrets"][0]["name"], "regcred");
    assert_eq!(ps["shareProcessNamespace"], true);
    assert_eq!(ps["securityContext"]["runAsUser"], 1000);
    assert_eq!(ps["containers"][0]["imagePullPolicy"], "Always");
    assert_eq!(ps["containers"][0]["securityContext"]["runAsNonRoot"], true);
    assert_eq!(ps["containers"][0]["securityContext"]["readOnlyRootFilesystem"], true); // raw merged
    assert_eq!(d["spec"]["strategy"]["type"], "Recreate");
    assert_eq!(d["spec"]["revisionHistoryLimit"], 3);
    assert_eq!(d["spec"]["progressDeadlineSeconds"], 600);
    assert_eq!(d["metadata"]["annotations"]["team"], "data");
}
```

- [ ] **Step 2** Run → FAIL.

- [ ] **Step 3: Implement** — add `fn security_context_json(sc: &SecurityContext) -> Value` (start from an object with the flat fields that are `Some` — `runAsUser`/`runAsGroup`/`fsGroup`/`runAsNonRoot` — then if `raw_override` is `Some`, merge its keys ON TOP via `for (k,v) in raw.as_object() { out.insert(k,v) }`). Splice (cluster fields from `spec.config`):
  - pod: `serviceAccountName`, `imagePullSecrets` (array of `{name}`), `shareProcessNamespace`, `securityContext` (pod).
  - container: `imagePullPolicy`, `securityContext` (container).
  - Deployment `spec`: `strategy` (`{type: kind}` + `rollingUpdate:{maxSurge?,maxUnavailable?}` when kind==RollingUpdate and either set), `revisionHistoryLimit`, `progressDeadlineSeconds`.
  - Deployment `metadata.annotations`: merge `deployment_annotations` (create the map if absent).
- [ ] **Step 4** Run tests + clippy + fmt. Commit `feat(orchestrator): cluster pod + Deployment scheduling fields`.

---

## Task 4: Render init/sidecar containers + envFrom + mounts (TDD)

**Files:** Modify `crates/egret-orchestrator/src/trino.rs`

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn containers_envfrom_mounts_render_and_gs_preserved() {
    let mut s = sample_spec();
    // worker graceful shutdown sets a grace period we must not clobber:
    s.config.worker.termination_grace_seconds = Some(30); // must lose to graceful shutdown
    s.config.worker_graceful_shutdown_enabled_helper(); // see note; or set via the WorkerRoleConfig graceful_shutdown
    s.config.init_containers = vec![MiniContainer { name: "wait".into(), image: "busybox".into(), command: vec!["sh".into(), "-c".into()], args: vec!["echo hi".into()], ..Default::default() }];
    s.config.sidecar_containers = vec![MiniContainer { name: "raw-side".into(), image: "".into(), raw_override: Some(json!({"name":"exporter","image":"prom/exporter"})), ..Default::default() }];
    s.config.env_from = vec![EnvFromSource { config_map_ref: Some("shared-env".into()), secret_ref: None }];
    s.config.config_mounts = vec![ObjectMount { name: "extra-cm".into(), mount_path: "/etc/extra".into(), sub_path: None, default_mode: None }];
    s.config.secret_mounts = vec![ObjectMount { name: "kt".into(), mount_path: "/etc/krb".into(), sub_path: Some("krb5.keytab".into()), default_mode: Some(0o400) }];
    let d = trino_deployment(&s, Role::Worker);
    let ps = &d["spec"]["template"]["spec"];
    assert_eq!(ps["initContainers"][0]["name"], "wait");
    assert!(ps["containers"].as_array().unwrap().iter().any(|c| c["name"] == "exporter")); // sidecar raw appended
    assert_eq!(ps["containers"][0]["envFrom"][0]["configMapRef"]["name"], "shared-env");
    assert!(ps["volumes"].as_array().unwrap().iter().any(|v| v["name"] == "extra-cm" && v["configMap"]["name"] == "extra-cm"));
    assert!(ps["volumes"].as_array().unwrap().iter().any(|v| v["secret"]["secretName"] == "kt"));
    let vm = &ps["containers"][0]["volumeMounts"];
    assert!(vm.as_array().unwrap().iter().any(|m| m["mountPath"] == "/etc/krb" && m["subPath"] == "krb5.keytab"));
    // graceful shutdown grace period preserved (worker GS uses its own value, not 30):
    assert_ne!(ps["terminationGracePeriodSeconds"], 30);
}
```

  NOTE: set the worker graceful shutdown via the real `WorkerRoleConfig.graceful_shutdown = Some(GracefulShutdown{enabled:true, grace_period_seconds:240})` in the test (drop the `..._helper()` pseudo-call). The assertion is that `termination_grace_seconds=30` does NOT override the GS 240.

- [ ] **Step 2** Run → FAIL.

- [ ] **Step 3: Implement** — add `fn container_json(c: &MiniContainer) -> Value` (raw_override wins verbatim; else `{name,image, command?, args?, env: [{name,value}], volumeMounts: [{name,mountPath,subPath?}]}` omitting empties). Then:
  - `pod_spec["initContainers"]` = array of `container_json` when `init_containers` non-empty.
  - Append `sidecar_containers` (as `container_json`) to `pod_spec["containers"]`.
  - `container["envFrom"]` = array of `{configMapRef:{name}}` / `{secretRef:{name}}` per entry.
  - For each `config_mounts`/`secret_mounts`: push a volume onto `pod_spec["volumes"]` (`{name, configMap:{name, defaultMode?}}` or `{name, secret:{secretName, defaultMode?}}`) AND a volumeMount onto `container["volumeMounts"]` (`{name, mountPath, subPath?}`). Use `name` as the volume name.
  - Confirm the worker-graceful-shutdown block still runs and its `terminationGracePeriodSeconds` is set BEFORE the Task-2 `termination_grace_seconds` guard (so GS wins).
- [ ] **Step 4** Run full `cargo test -p egret-orchestrator` + clippy + fmt. Commit `feat(orchestrator): init/sidecar containers + envFrom + config/secret mounts`.

---

## Task 5: Control-plane carriers + `build_provision_req`

**Files:** Modify `crates/egret-controlplane/src/engines.rs`, `lib.rs`; Test `tests/routes_api.rs`

- [ ] **Step 1** Add kube-free carrier structs mirroring EVERY orchestrator type from Task 1 (`TolerationReq`, `TopologySpreadConstraintReq`, `NodeSelectorRequirementReq`, `AffinityReq`, `ContainerPortReq`, `SecurityContextReq`, `VolumeMountReq`, `MiniContainerReq`, `EnvFromSourceReq`, `ObjectMountReq`, `DeploymentStrategyReq`) — same field names, `raw_override: Option<serde_json::Value>`. Add the per-role fields to `RoleConfigReq` and the cluster fields to `TrinoConfigReq`. Export the new carriers from `lib.rs`.
- [ ] **Step 2** Parse them in `trino_config_from_json`: the wire uses camelCase (`nodeSelector`, `tolerations`, `topologySpread`, `affinity{nodeMatch,podAntiAffinityTopologyKey,rawOverride}`, `terminationGraceSeconds`, `additionalExposedPorts`, and cluster `imagePullSecrets`, `imagePullPolicy`, `serviceAccount`, `podSecurityContext`, `containerSecurityContext`, `initContainers`, `sidecarContainers`, `envFrom`, `configMounts`, `secretMounts`, `deploymentStrategy`, `deploymentAnnotations`, `revisionHistoryLimit`, `progressDeadlineSeconds`, `shareProcessNamespace`). `raw_override`/security-context/affinity/container carry raw JSON through as `serde_json::Value`. NO secret resolution — these reference k8s objects by name.
- [ ] **Step 3** `build_provision_req` maps carriers → the provision request straight through (these fields need no `st`-side resolution).
- [ ] **Step 4** Route test: `PUT` a `trinoConfig` with `coordinator.nodeSelector`, `serviceAccount`, an `initContainers` entry, and an `affinity.rawOverride` → 200; `GET` reflects them. `DATABASE_URL=… cargo test -p egret-controlplane`.
- [ ] **Step 5** fmt; commit `feat(controlplane): pod/scheduling carriers`.

---

## Task 6: egret-bin `spec_from` adapter

**Files:** Modify `crates/egret-bin/src/main.rs`

- [ ] **Step 1** Extend `spec_from` to map every new carrier → its orchestrator type (field-for-field; `Vec`→`Vec`, `Option<Value>`→`Option<Value>`). Add small helpers if the nesting warrants (e.g. `mini_container_from`, `affinity_from`, `security_context_from`).
- [ ] **Step 2** `cargo build --workspace 2>&1 | grep "^error"` empty. `cargo fmt --all -- --check`. Commit `feat(bin): map pod/scheduling carriers to orchestrator spec`.

---

## Task 7: Frontend api types + hook + PodSchedulingSection (cluster-wide)

**Files:** Modify `frontend/src/api/controlPlane.ts`, `useEngineConfigForm.ts`; Create `frontend/src/routes/control-plane/PodSchedulingSection.tsx`

- [ ] **Step 1** TS types on `TrinoConfig`: the cluster fields (`imagePullSecrets?: string[]`, `imagePullPolicy?: string`, `serviceAccount?: string`, `podSecurityContext?`, `containerSecurityContext?`, `initContainers?`, `sidecarContainers?`, `envFrom?`, `configMounts?`, `secretMounts?`, `deploymentStrategy?`, `deploymentAnnotations?`, `revisionHistoryLimit?`, `progressDeadlineSeconds?`, `shareProcessNamespace?`) and the per-role fields on the role type (`nodeSelector?`, `tolerations?`, `topologySpread?`, `affinity?`, `terminationGraceSeconds?`, `additionalExposedPorts?`). `tsc`.
- [ ] **Step 2** Add all the state to `useEngineConfigForm` (state + prefill in the `[engine.spec]` effect + include them in the built `trinoConfig` payload; per-role fields fold into the `formToRole`/`roleToForm` conversion). Keep the payload builder's omit-empty style consistent.
- [ ] **Step 3** Create `PodSchedulingSection.tsx` with the CLUSTER-WIDE part: serviceAccount `<Input>`, imagePullSecrets (string-list rows), pullPolicy `<Select>` (Always/IfNotPresent/Never), pod + container securityContext (the flat number/bool fields + a raw-JSON `<Textarea>` override with on-blur `JSON.parse` validation → inline error + an `onValidityChange` prop so an invalid raw blocks Save), envFrom rows (configMapRef/secretRef), config-mounts + secret-mounts rows (name/mountPath/subPath/defaultMode), deployment strategy (`<Select>` kind + maxSurge/maxUnavailable inputs), deploymentAnnotations (`KeyValueEditor`), revisionHistoryLimit/progressDeadlineSeconds inputs, shareProcessNamespace `<Switch>`. `tsc`/`build`.
- [ ] **Step 4** Commit `feat(ui): pod/scheduling api + hook + cluster-wide section`.

---

## Task 8: Frontend per-role builders + mini-container builder

**Files:** Modify `frontend/src/routes/control-plane/PodSchedulingSection.tsx`

- [ ] **Step 1** Add the PER-ROLE block: a Coordinator/Worker toggle (`Tabs` or a segmented control) selecting which role's fields are edited; nodeSelector (`KeyValueEditor`), tolerations rows (key/operator/value/effect/tolerationSeconds), topologySpread rows (maxSkew/topologyKey/whenUnsatisfiable/matchLabels), affinity (node-match requirement rows {key/operator/values} + anti-affinity topologyKey input + a raw-JSON override `<Textarea>`), terminationGraceSeconds input, additionalExposedPorts rows (name/port).
- [ ] **Step 2** Add a reusable `MiniContainerEditor` (name/image/command list/args list/env KV/volumeMounts rows + a raw-JSON override textarea) used for both init + sidecar container lists. Raw-JSON textareas use the same validate-on-blur helper from Task 7.
- [ ] **Step 3** `tsc`/`eslint`/`build`. Commit `feat(ui): per-role placement + mini-container builders`.

---

## Task 9: Wire the Pod & Scheduling sub-tab

**Files:** Modify `frontend/src/routes/control-plane/EngineDetail.tsx`

- [ ] **Step 1** Add a 7th Configuration sub-tab trigger `Pod & Scheduling` (after Advanced) rendering `<PodSchedulingSection {...cfg slices} onValidityChange={setPodSchedValid} />`.
- [ ] **Step 2** Thread an invalid-raw-JSON flag into the Save button: `disabled={!dirty || saving || !podSchedValid}` (a raw-JSON parse error blocks Save on the Configuration tab).
- [ ] **Step 3** `tsc`/`eslint`/`build`; the new sub-tab renders + prefills + saves. Commit `feat(ui): Pod & Scheduling Configuration sub-tab`.

---

## Task 10: Gate + live test + PR

- [ ] **Step 1** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`.
- [ ] **Step 2** Frontend `tsc` / `eslint` / `vitest run` / `vite build`.
- [ ] **Step 3** Live on kind: rebuild egret-bin, restart serve. On `demo2` PUT a config with a **schedulable** nodeSelector (a label the kind node actually has, e.g. `kubernetes.io/os: linux`), a toleration, `serviceAccount: default`, pod securityContext `runAsUser: 1000` + `fsGroup: 1000`, `imagePullPolicy: IfNotPresent`, a sidecar container (e.g. `busybox` `sleep 3600`), and an affinity `rawOverride`. Roll → both pods Ready → `kubectl get pod <coordinator> -o yaml` (ANSI-stripped) shows `nodeSelector`, `tolerations`, `serviceAccountName: default`, `securityContext.runAsUser: 1000`, the sidecar container, and the verbatim affinity. Confirm the WORKER still has its graceful-shutdown `terminationGracePeriodSeconds` (not overridden).
- [ ] **Step 4** Drive the Pod & Scheduling tab in the browser (Playwright): the cluster + per-role (toggle coord/worker) fields render; set a field + Save → PUT 200. Confirm a bad raw-JSON blocks Save (button disabled + inline error).
- [ ] **Step 5** Commit; open PR; run the full local gate before an admin squash-merge.

---

## Self-Review notes

- **Spec coverage:** types (T1) ✓; per-role placement incl. structured+raw affinity (T2) ✓; cluster pod + Deployment fields + securityContext raw-merge (T3) ✓; init/sidecar + envFrom + mounts + GS-preservation (T4) ✓; carriers (T5) ✓; egret-bin (T6) ✓; frontend api+hook+cluster UI (T7) ✓; per-role + mini-container UI (T8) ✓; sub-tab wiring + raw-JSON-blocks-save (T9) ✓; live verify + GS-not-clobbered + Playwright (T10) ✓.
- **Type consistency:** orchestrator structs (T1) ↔ carriers `*Req` (T5) ↔ TS types (T7) ↔ wire camelCase (T5 Step 2). `raw_override` is `Option<serde_json::Value>` end-to-end.
- **Merge rule (critical):** worker graceful-shutdown sets `terminationGracePeriodSeconds` first; the per-role `termination_grace_seconds` splice is guarded by `is_none()` so GS wins (T2 Step 3, tested T4).
- **No secret resolution:** all references are k8s object names / raw JSON — carriers map straight through (T5 Step 3).
- **Raw-JSON safety:** invalid raw override blocks Save client-side (T9 Step 2); server passes JSON opaquely and the API server rejects a bad object at apply, surfacing through the provision path.
- **Deferred:** custom pod lifecycle beyond GS preStop; HPA/KEDA (1.5.4); observability (1.5.5).
