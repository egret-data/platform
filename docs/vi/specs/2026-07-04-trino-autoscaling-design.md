# Autoscaling worker Trino (Phase 1.5.4, 2026-07-04)

Bản EN (canonical): `docs/en/specs/2026-07-04-trino-autoscaling-design.md`.
Nối tiếp engine provisioning đã ship; tái dùng pattern "object k8s quản lý thêm"
apply/delete của 1.6a Ingress + pattern structured-common + raw-override của 1.5.3.

**Mục tiêu:** Autoscale WORKER Deployment — **HPA** native (CPU/memory) hoặc **KEDA
ScaledObject** (scale-to-zero + trigger cron/prometheus) — trong sub-tab **Autoscaling**
mới. Coordinator giữ 1 replica cố định.

**Non-goals:** autoscale coordinator; HPA custom-metric qua prometheus-adapter (KEDA
prometheus trigger đã lo query-based); cài KEDA operator (giả định có sẵn); VPA.

## Chart shapes
- HPA (`autoscaling/v2`): maxReplicas, targetCPUUtilizationPercentage, targetMemoryUtilizationPercentage, behavior.
- KEDA (`ScaledObject`): pollingInterval, cooldownPeriod, minReplicaCount (**0 = scale-to-zero**), maxReplicaCount, fallback, triggers.

## Data model — `TrinoConfig.autoscaling: Option<Autoscaling>` (nhắm worker)
`Autoscaling { mode: "hpa"|"keda", hpa, keda }` — **mode loại trừ nhau** (KEDA tự tạo HPA).
`HpaConfig { max_replicas, target_cpu_percent?, target_memory_percent?, behavior_raw? }`.
`KedaConfig { min_replica_count (0=zero), max_replica_count, polling_interval?, cooldown_period?, triggers: Vec<KedaTrigger>, fallback_raw? }`.
`KedaTrigger { kind: "cron"|"prometheus"|"raw", cron?, prometheus?, raw_override? }`;
`CronTrigger{start,end,desired_replicas,timezone}`; `PrometheusTrigger{server_address,metric_name,query,threshold}`.
Absent ⇒ worker giữ `replicas` cố định (như hiện tại).

## Orchestrator
- `trino_hpa(spec)` (mode=hpa): `autoscaling/v2 HPA`, scaleTargetRef→worker Deployment,
  minReplicas=worker.replicas, maxReplicas, metrics cpu/memory (theo field set), behavior=behavior_raw.
- `trino_scaled_object(spec)` (mode=keda): `ScaledObject`, min/max, polling/cooldown, fallback=fallback_raw,
  triggers render `{type:"cron",metadata:{...}}`/`{type:"prometheus",metadata:{...}}`/raw_override verbatim.
- Provisioner: apply object theo mode; delete/update xoá best-effort CẢ HPA lẫn ScaledObject (slot 1.6a Ingress).
- **Replicas conflict**: khi autoscaling Some, WORKER Deployment **bỏ `replicas`** (HPA/KEDA sở hữu). Coordinator giữ nguyên. Scale-down phối hợp graceful-shutdown preStop.

## Control-plane + egret-bin
Carrier kube-free mirror mọi type (raw = `Option<serde_json::Value>`); build_provision_req map thẳng (không resolve secret). spec_from map carrier→orchestrator.

## Frontend — sub-tab Autoscaling (thứ 8)
Thêm vào `useEngineConfigForm`: mode `<Select>` (Off/HPA/KEDA); HPA fields (maxReplicas/targetCpu%/targetMemory%/behavior raw) khi HPA; KEDA fields (min/max/polling/cooldown/fallback raw + trigger builder: kind Select cron/prometheus/raw → structured hoặc raw-JSON) khi KEDA. Raw-JSON validate blur (`validateRawJson`) chặn Save khi lỗi. Note KEDA cần operator.

## Test
Unit (HPA object; ScaledObject + triggers; worker Deployment bỏ replicas khi autoscaling; apply/delete cả hai) + live kind (bật HPA → `kubectl get hpa` nhắm worker Deployment; worker deploy không có `spec.replicas`). KEDA: ScaledObject render+apply nhưng chỉ reconcile khi có KEDA operator — opt-in, note như nginx-ingress caveat.

Compat: optional, không migration, toggle roll engine + apply/delete HPA/ScaledObject. Chi tiết xem bản EN.
