# Trino Worker Autoscaling (Phase 1.5.4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Autoscale the Trino worker Deployment via a native HPA or a KEDA ScaledObject (mode-exclusive), surfaced in a new **Autoscaling** Configuration sub-tab.

**Architecture:** New `Autoscaling` field on `TrinoConfig`; `trino.rs` renders `trino_hpa`/`trino_scaled_object` (a new managed k8s object, applied/deleted by the provisioner exactly like the 1.6a Ingress) and OMITS `spec.replicas` from the worker Deployment when autoscaling owns it. Kube-free carriers + `spec_from` adapter + an Autoscaling frontend sub-tab wired into `useEngineConfigForm`. KEDA triggers use the 1.5.3 structured-common + raw-override pattern.

**Tech Stack:** Rust (serde_json), K8s `autoscaling/v2` + `keda.sh/v1alpha1`, React 18 + shadcn.

**Spec:** `docs/en/specs/2026-07-04-trino-autoscaling-design.md`.
**Confirmed facts:** worker Deployment name = `{name}-worker`; its `spec.replicas` is set at `trino.rs:794`; HPA `minReplicas` = `spec.config.worker.base.replicas`.

---

## File Structure
- **Modify** `crates/egret-orchestrator/src/spec.rs` (types) + `trino.rs` (render + provisioner) + `lib.rs`.
- **Modify** `crates/egret-controlplane/src/engines.rs` (carriers) + `lib.rs`.
- **Modify** `crates/egret-bin/src/main.rs` (`spec_from`).
- **Modify** `frontend/src/api/controlPlane.ts` + `useEngineConfigForm.ts` + `EngineDetail.tsx`; **Create** `frontend/src/routes/control-plane/EngineAutoscaling.tsx`.

---

## Task 1: Orchestrator types

**Files:** Modify `crates/egret-orchestrator/src/spec.rs`, `lib.rs`

- [ ] **Step 1** Add (all `#[derive(Clone, Debug, Default)]`; `Value` = `serde_json::Value`):

```rust
#[derive(Clone, Debug, Default)]
pub struct CronTrigger { pub start: String, pub end: String, pub desired_replicas: u32, pub timezone: String }
#[derive(Clone, Debug, Default)]
pub struct PrometheusTrigger { pub server_address: String, pub metric_name: String, pub query: String, pub threshold: String }
#[derive(Clone, Debug, Default)]
pub struct KedaTrigger { pub kind: String, pub cron: Option<CronTrigger>, pub prometheus: Option<PrometheusTrigger>, pub raw_override: Option<Value> }
#[derive(Clone, Debug, Default)]
pub struct KedaConfig { pub min_replica_count: u32, pub max_replica_count: u32, pub polling_interval: Option<u32>, pub cooldown_period: Option<u32>, pub triggers: Vec<KedaTrigger>, pub fallback_raw: Option<Value> }
#[derive(Clone, Debug, Default)]
pub struct HpaConfig { pub max_replicas: u32, pub target_cpu_percent: Option<u32>, pub target_memory_percent: Option<u32>, pub behavior_raw: Option<Value> }
#[derive(Clone, Debug, Default)]
pub struct Autoscaling { pub mode: String, pub hpa: Option<HpaConfig>, pub keda: Option<KedaConfig> }
```

- [ ] **Step 2** Add `pub autoscaling: Option<Autoscaling>` to `TrinoConfig`. Re-export the 6 new structs from `lib.rs`.
- [ ] **Step 3** `cargo build -p egret-orchestrator` (additive). `cargo fmt --all`. Commit `refactor(orchestrator): autoscaling config types`.

---

## Task 2: Render HPA + ScaledObject + worker-omits-replicas (TDD)

**Files:** Modify `crates/egret-orchestrator/src/trino.rs`

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn hpa_object_shape() {
    let mut s = sample_spec();
    s.config.worker.base.replicas = 2;
    s.config.autoscaling = Some(Autoscaling { mode: "hpa".into(), hpa: Some(HpaConfig { max_replicas: 5, target_cpu_percent: Some(60), target_memory_percent: Some(80), behavior_raw: None }), keda: None });
    let h = trino_hpa(&s).unwrap();
    assert_eq!(h["apiVersion"], "autoscaling/v2");
    assert_eq!(h["kind"], "HorizontalPodAutoscaler");
    assert_eq!(h["metadata"]["name"], format!("{}-worker", s.name));
    assert_eq!(h["spec"]["scaleTargetRef"]["name"], format!("{}-worker", s.name));
    assert_eq!(h["spec"]["minReplicas"], 2);
    assert_eq!(h["spec"]["maxReplicas"], 5);
    let metrics = h["spec"]["metrics"].as_array().unwrap();
    assert!(metrics.iter().any(|m| m["resource"]["name"] == "cpu" && m["resource"]["target"]["averageUtilization"] == 60));
    assert!(metrics.iter().any(|m| m["resource"]["name"] == "memory" && m["resource"]["target"]["averageUtilization"] == 80));
    assert!(trino_scaled_object(&s).is_none()); // mode=hpa ⇒ no ScaledObject
}
#[test]
fn scaledobject_and_triggers() {
    let mut s = sample_spec();
    s.config.autoscaling = Some(Autoscaling { mode: "keda".into(), hpa: None, keda: Some(KedaConfig {
        min_replica_count: 0, max_replica_count: 4, polling_interval: Some(30), cooldown_period: Some(300),
        triggers: vec![
            KedaTrigger { kind: "cron".into(), cron: Some(CronTrigger { start: "0 8 * * *".into(), end: "0 20 * * *".into(), desired_replicas: 3, timezone: "UTC".into() }), prometheus: None, raw_override: None },
            KedaTrigger { kind: "raw".into(), cron: None, prometheus: None, raw_override: Some(json!({"type":"kafka","metadata":{"topic":"t"}})) },
        ], fallback_raw: None }) });
    let so = trino_scaled_object(&s).unwrap();
    assert_eq!(so["apiVersion"], "keda.sh/v1alpha1");
    assert_eq!(so["kind"], "ScaledObject");
    assert_eq!(so["spec"]["scaleTargetRef"]["name"], format!("{}-worker", s.name));
    assert_eq!(so["spec"]["minReplicaCount"], 0);
    assert_eq!(so["spec"]["maxReplicaCount"], 4);
    assert_eq!(so["spec"]["triggers"][0]["type"], "cron");
    assert_eq!(so["spec"]["triggers"][0]["metadata"]["desiredReplicas"], "3"); // KEDA wants strings
    assert_eq!(so["spec"]["triggers"][1]["type"], "kafka"); // raw verbatim
    assert!(trino_hpa(&s).is_none());
}
#[test]
fn worker_omits_replicas_when_autoscaling() {
    let mut s = sample_spec();
    s.config.autoscaling = Some(Autoscaling { mode: "hpa".into(), hpa: Some(HpaConfig { max_replicas: 3, ..Default::default() }), keda: None });
    let w = trino_deployment(&s, Role::Worker);
    assert!(w["spec"].get("replicas").is_none() || w["spec"]["replicas"].is_null());
    // coordinator still has replicas:
    assert!(trino_deployment(&s, Role::Coordinator)["spec"]["replicas"].is_number());
    // and with no autoscaling the worker keeps replicas:
    let mut s2 = sample_spec();
    assert!(trino_deployment(&s2, Role::Worker)["spec"]["replicas"].is_number());
    let _ = &mut s2;
}
```

- [ ] **Step 2** Run → FAIL.

- [ ] **Step 3: Implement** `trino_hpa` + `trino_scaled_object` (each returns `Option<Value>`, `None` unless `autoscaling` is Some with the matching `mode`), plus a `keda_trigger_json(t)` helper (kind=="raw" → `raw_override` verbatim; kind=="cron" → `{type:"cron", metadata:{start,end, desiredReplicas: desired_replicas.to_string(), timezone}}`; kind=="prometheus" → `{type:"prometheus", metadata:{serverAddress, metricName, query, threshold}}`). HPA metrics: push a cpu Resource metric when `target_cpu_percent` is Some, a memory one when `target_memory_percent` is Some; `behavior` = `behavior_raw` when Some. ScaledObject: set `pollingInterval`/`cooldownPeriod` when Some, `fallback` = `fallback_raw` when Some. Both objects: `metadata.name` = `{name}-worker`, `namespace`, egret labels; the HPA `scaleTargetRef` = `{apiVersion:"apps/v1", kind:"Deployment", name:"{name}-worker"}`, the ScaledObject `scaleTargetRef` = `{name:"{name}-worker"}`.
- [ ] **Step 4: worker omits replicas** — in `trino_deployment`, when `role` is worker AND `spec.config.autoscaling.is_some()`, do NOT set `spec.replicas` (build the Deployment `spec` object without the `replicas` key, or remove it after). The coordinator branch is untouched.
- [ ] **Step 5** Run tests + clippy + fmt. Commit `feat(orchestrator): render HPA + KEDA ScaledObject + worker replicas omit`.

---

## Task 3: Provisioner apply/delete both objects (TDD)

**Files:** Modify `crates/egret-orchestrator/src/trino.rs`

- [ ] **Step 1: Failing test** (mirror the Ingress provisioner test)

```rust
#[tokio::test]
async fn provision_applies_autoscaler_and_delete_removes_both() {
    let mut s = sample_spec();
    s.config.autoscaling = Some(Autoscaling { mode: "hpa".into(), hpa: Some(HpaConfig { max_replicas: 3, ..Default::default() }), keda: None });
    let kube = FakeKube::default();
    let p = TrinoProvisioner::new(std::sync::Arc::new(kube.clone()));
    p.provision(&s).await.unwrap();
    assert!(kube.applied_kinds_names().contains(&("HorizontalPodAutoscaler".into(), format!("{}-worker", s.name))));
    p.delete(&s.namespace, &s.name).await.unwrap();
    // delete is best-effort for BOTH kinds regardless of mode:
    assert!(kube.deleted_kinds_names().iter().any(|(k, n)| k == "HorizontalPodAutoscaler" && n == &format!("{}-worker", s.name)));
    assert!(kube.deleted_kinds_names().iter().any(|(k, n)| k == "ScaledObject"));
}
```

  (Adapt the FakeKube accessor names to whatever the existing Ingress provisioner test uses — grep the test module for how it asserts applied/deleted objects and mirror it.)

- [ ] **Step 2** Run → FAIL.

- [ ] **Step 3: Implement** — in `TrinoProvisioner::apply_all`, after the Ingress block: `if let Some(h) = trino_hpa(spec) { self.kube.apply(&h).await?; }` and `if let Some(so) = trino_scaled_object(spec) { self.kube.apply(&so).await?; }`. In `delete`: best-effort delete BOTH `self.kube.delete("autoscaling/v2", "HorizontalPodAutoscaler", namespace, &format!("{name}-worker"))` and `self.kube.delete("keda.sh/v1alpha1", "ScaledObject", namespace, &format!("{name}-worker"))` (ignore errors — harmless when absent). In `update`: when `autoscaling` is None OR the mode changed, best-effort delete the OTHER/both objects so switching modes / turning off removes the stale one (mirror how the Ingress `update` cleans up when disabled).
- [ ] **Step 4** Run tests + clippy + fmt. Commit `feat(orchestrator): provisioner applies/deletes HPA + ScaledObject`.

---

## Task 4: Control-plane carriers + build_provision_req

**Files:** Modify `crates/egret-controlplane/src/engines.rs`, `lib.rs`; Test `tests/routes_api.rs`

- [ ] **Step 1** Add carriers `CronTriggerReq`, `PrometheusTriggerReq`, `KedaTriggerReq{kind,cron,prometheus,raw_override}`, `KedaConfigReq`, `HpaConfigReq`, `AutoscalingReq{mode,hpa,keda}` (identical field names; raw = `Option<serde_json::Value>`); add `autoscaling: Option<AutoscalingReq>` to `TrinoConfigReq`; export from `lib.rs`.
- [ ] **Step 2** Parse in `trino_config_from_json`: wire camelCase under `trinoConfig.autoscaling`: `{mode, hpa:{maxReplicas,targetCpuPercent,targetMemoryPercent,behaviorRaw}, keda:{minReplicaCount,maxReplicaCount,pollingInterval,cooldownPeriod,triggers:[{kind,cron:{start,end,desiredReplicas,timezone},prometheus:{serverAddress,metricName,query,threshold},rawOverride}],fallbackRaw}}`. raw fields pass through as `serde_json::Value`. No secret resolution.
- [ ] **Step 3** `build_provision_req` maps it straight through (already consumes the full `TrinoConfigReq`).
- [ ] **Step 4** Route test: `PUT` a `trinoConfig` with `autoscaling.mode="hpa"` + `hpa.maxReplicas=5` (and separately a keda cron trigger) → 200; `GET` reflects them. `DATABASE_URL=… cargo test -p egret-controlplane`.
- [ ] **Step 5** fmt; commit `feat(controlplane): autoscaling carriers`.

---

## Task 5: egret-bin spec_from adapter

**Files:** Modify `crates/egret-bin/src/main.rs`

- [ ] **Step 1** Extend `spec_from` to map `AutoscalingReq`→`egret_orchestrator::Autoscaling` (helpers `hpa_config_from`, `keda_config_from`, `keda_trigger_from`, `cron_trigger_from`, `prometheus_trigger_from`; raw `Option<Value>` cloned through).
- [ ] **Step 2** `cargo build --workspace 2>&1 | grep "^error"` empty. `cargo fmt --all -- --check`. Commit `feat(bin): map autoscaling to orchestrator spec`.

---

## Task 6: Frontend api + hook + Autoscaling sub-tab

**Files:** Modify `frontend/src/api/controlPlane.ts`, `useEngineConfigForm.ts`, `EngineDetail.tsx`; Create `frontend/src/routes/control-plane/EngineAutoscaling.tsx`

- [ ] **Step 1** TS `TrinoConfig.autoscaling?: { mode: "hpa"|"keda"; hpa?: {maxReplicas:number; targetCpuPercent?:number; targetMemoryPercent?:number; behaviorRaw?:unknown}; keda?: {minReplicaCount:number; maxReplicaCount:number; pollingInterval?:number; cooldownPeriod?:number; triggers: {kind:string; cron?:{start,end,desiredReplicas,timezone}; prometheus?:{serverAddress,metricName,query,threshold}; rawOverride?:unknown}[]; fallbackRaw?:unknown} }`. `tsc`.
- [ ] **Step 2** Add the state to `useEngineConfigForm` (state + prefill in the `[engine.spec]` effect + include in the built `trinoConfig` payload with omit-empty; drop `autoscaling` entirely when mode is unset/"off"). Expose in the return object.
- [ ] **Step 3** Create `EngineAutoscaling.tsx`: a mode `<Select>` (Off / HPA / KEDA). When **HPA**: maxReplicas + targetCpuPercent + targetMemoryPercent `<Input type=number>` + a behavior raw-JSON `<Textarea>` (reuse `validateRawJson` + `onValidityChange`). When **KEDA**: minReplicaCount/maxReplicaCount/pollingInterval/cooldownPeriod inputs + a fallback raw-JSON + a triggers builder (add/remove rows; each row a kind `<Select>` cron/prometheus/raw → the structured fields (cron: start/end/desiredReplicas/timezone; prometheus: serverAddress/metricName/query/threshold) or a raw-JSON `<Textarea>`). A note: "KEDA requires the KEDA operator installed in the cluster." Import `validateRawJson` from `PodSchedulingSection` (or move it to a shared util if cleaner).
- [ ] **Step 4** In `EngineDetail.tsx`, add an 8th Configuration sub-tab `Autoscaling` (after `Pod & Scheduling`) rendering `<EngineAutoscaling {...cfg slices} onValidityChange={setAutoscaleValid} />`; fold `autoscaleValid` into the Configuration Save `disabled` (alongside `podSchedValid`).
- [ ] **Step 5** `tsc`/`eslint`/`build`/`vitest run src/routes/control-plane`. Commit `feat(ui): Autoscaling Configuration sub-tab (HPA + KEDA)`.

---

## Task 7: Gate + live test + PR

- [ ] **Step 1** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`.
- [ ] **Step 2** Frontend `tsc` / `eslint` / `vitest run` / `vite build`.
- [ ] **Step 3** Live on kind: rebuild egret-bin, restart serve. PUT `demo2` with `autoscaling.mode="hpa"`, `hpa.maxReplicas=3`, `hpa.targetCpuPercent=60` → roll → `kubectl get hpa -n egret-engines` shows `demo2-worker` HPA targeting the `demo2-worker` Deployment; `kubectl get deploy demo2-worker -o yaml` has NO `spec.replicas`. Then PUT `autoscaling` back off → the HPA is deleted + the worker Deployment gets `replicas` again. (KEDA: PUT a ScaledObject config → `kubectl get scaledobject` shows the object rendered; note it only reconciles with the KEDA operator installed.)
- [ ] **Step 4** Drive the Autoscaling tab (Playwright): mode Off/HPA/KEDA switches the fields; set HPA maxReplicas + Save → PUT 200; a bad raw-JSON disables Save.
- [ ] **Step 5** Commit; open PR; run the full local gate before an admin squash-merge.

---

## Self-Review notes
- **Spec coverage:** types (T1) ✓; HPA + ScaledObject + triggers + worker-omits-replicas (T2) ✓; provisioner apply/delete both (T3) ✓; carriers (T4) ✓; egret-bin (T5) ✓; frontend api+hook+sub-tab (T6) ✓; live verify HPA + replicas-omit + delete-on-off + Playwright (T7) ✓.
- **Type consistency:** orchestrator (T1) ↔ carriers `*Req` (T4) ↔ TS (T6) ↔ wire camelCase (T4 Step 2). raw fields `Option<serde_json::Value>` end-to-end.
- **Mode exclusivity:** `trino_hpa`/`trino_scaled_object` each render only for their `mode`; the provisioner deletes BOTH on delete + the stale one on update, so switching HPA↔KEDA↔off never leaves an orphan.
- **Replicas conflict:** worker Deployment omits `spec.replicas` iff `autoscaling` is Some (T2 Step 4, tested); HPA `minReplicas` = the configured worker replicas.
- **KEDA operator caveat:** the ScaledObject renders + applies but only reconciles with the KEDA operator present (documented; live test verifies the object, not reconciliation).
- **Deferred:** coordinator autoscaling; prometheus-adapter custom-metric HPA; VPA; observability (1.5.5).
