# Trino worker autoscaling (Phase 1.5.4, 2026-07-04)

VI reference: `docs/vi/specs/2026-07-04-trino-autoscaling-design.md`.
Builds on the shipped engine provisioning (1.5 → 1.6b, 1.5.3) — reuses the 1.6a
Ingress "extra managed object" apply/delete pattern and the 1.5.3 structured-common +
raw-override pattern.

**Goal:** Autoscale the Trino WORKER Deployment — a native **HPA** (CPU/memory
utilization) or a **KEDA `ScaledObject`** (scale-to-zero + cron/prometheus triggers) —
surfaced in a new **Autoscaling** Configuration sub-tab. The coordinator stays a single
fixed replica.

**Non-goals:** coordinator autoscaling; custom-metric HPA via a prometheus-adapter
(KEDA's prometheus trigger covers query-based scaling); installing the KEDA operator
(assumed present for real KEDA reconciliation); VPA.

## Chart-verified shapes
- HPA (`autoscaling/v2`): `maxReplicas`, `targetCPUUtilizationPercentage` (→ a cpu
  Resource metric), `targetMemoryUtilizationPercentage` (→ a memory metric), `behavior`.
- KEDA (`keda.sh/v1alpha1 ScaledObject`): `pollingInterval`, `cooldownPeriod`,
  `minReplicaCount` (**0 = scale-to-zero**), `maxReplicaCount`, `fallback`, `triggers`.

## Data model — `TrinoConfig.autoscaling: Option<Autoscaling>` (targets the worker)

```
Autoscaling { mode: String /* "hpa" | "keda" */, hpa: Option<HpaConfig>, keda: Option<KedaConfig> }
HpaConfig { max_replicas: u32, target_cpu_percent: Option<u32>, target_memory_percent: Option<u32>, behavior_raw: Option<Value> }
KedaConfig { min_replica_count: u32, max_replica_count: u32, polling_interval: Option<u32>, cooldown_period: Option<u32>, triggers: Vec<KedaTrigger>, fallback_raw: Option<Value> }
KedaTrigger { kind: String /* "cron" | "prometheus" | "raw" */, cron: Option<CronTrigger>, prometheus: Option<PrometheusTrigger>, raw_override: Option<Value> }
CronTrigger { start: String, end: String, desired_replicas: u32, timezone: String }
PrometheusTrigger { server_address: String, metric_name: String, query: String, threshold: String }
```

`mode` is exclusive — KEDA already creates its own HPA, so HPA and KEDA are never both
active. `autoscaling` absent ⇒ the worker keeps its fixed `replicas` (today's behaviour).

## Orchestrator rendering (`trino.rs`)

- `trino_hpa(spec) -> Option<Value>` (when `autoscaling.mode == "hpa"`): an
  `autoscaling/v2 HorizontalPodAutoscaler` named after the worker Deployment,
  `scaleTargetRef` → the worker Deployment, `minReplicas` = the worker's configured
  `replicas`, `maxReplicas` = `hpa.max_replicas`, `metrics` = a cpu Resource metric when
  `target_cpu_percent` is set + a memory metric when `target_memory_percent` is set,
  `behavior` = `behavior_raw` when set.
- `trino_scaled_object(spec) -> Option<Value>` (when `mode == "keda"`): a
  `keda.sh/v1alpha1 ScaledObject`, `scaleTargetRef.name` → the worker Deployment,
  `minReplicaCount`/`maxReplicaCount`, optional `pollingInterval`/`cooldownPeriod`,
  `fallback` = `fallback_raw` when set, and `triggers` — each `KedaTrigger` renders to
  `{type:"cron", metadata:{start,end,desiredReplicas,timezone}}` /
  `{type:"prometheus", metadata:{serverAddress,metricName,query,threshold}}` / the
  `raw_override` verbatim (for `kind == "raw"`).
- **Provisioner**: `apply_all` applies whichever object the mode selects; `delete` (and
  `update` when autoscaling is off/switched) best-effort deletes BOTH the HPA and the
  ScaledObject by name — exactly the 1.6a Ingress slot.
- **Replicas conflict**: when `autoscaling` is Some, the WORKER Deployment OMITS the
  `replicas` field (the HPA/KEDA owns it — setting both makes them fight). The coordinator
  Deployment always keeps its `replicas`. Scale-down composes with the worker
  graceful-shutdown preStop (existing).

## Control-plane + egret-bin

Kube-free carriers mirror every type (`raw_override`/`behavior_raw`/`fallback_raw` =
`Option<serde_json::Value>`); `build_provision_req` maps straight through (no secret
resolution). `spec_from` (egret-bin) maps carriers → orchestrator types.

## Frontend — new **Autoscaling** Configuration sub-tab

An 8th sub-tab, added to `useEngineConfigForm`: a mode `<Select>` (Off / HPA / KEDA);
**HPA** fields (maxReplicas, targetCpu%, targetMemory%, a behavior raw-JSON `<Textarea>`)
when HPA; **KEDA** fields (minReplicaCount, maxReplicaCount, pollingInterval,
cooldownPeriod, a fallback raw-JSON, and a triggers builder — each trigger a kind
`<Select>` (cron/prometheus/raw) → the structured fields or a raw-JSON `<Textarea>`) when
KEDA. Raw-JSON textareas validate on blur (reuse `validateRawJson`) and block Save on
error (the `onValidityChange` pattern from Pod & Scheduling). A note that KEDA needs the
KEDA operator installed in the cluster.

## Test
- **Unit** (`trino.rs`): the HPA object (scaleTargetRef, min from worker replicas, max,
  cpu+memory metrics, behavior raw); the ScaledObject (min/max, triggers cron/prometheus/
  raw); the worker Deployment OMITS `replicas` when autoscaling is set (and keeps it when
  not); apply/delete of both objects.
- **Live (kind)**: enable HPA (maxReplicas 3, cpu 60%) → provision → `kubectl get hpa`
  shows the HPA targeting the worker Deployment; `kubectl get deploy <worker> -o yaml`
  has no `spec.replicas`. (KEDA: the `ScaledObject` renders + applies but only reconciles
  with the KEDA operator present — opt-in, documented, like the nginx-ingress caveat.)

## Compat
- Optional; no DB migration; toggling autoscaling rolls the engine + applies/deletes the
  HPA/ScaledObject. Existing engines keep fixed replicas.
