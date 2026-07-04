# Mở rộng config Trino — per-role, JVM, graceful shutdown, probes (Phase 1.5.1, 2026-07-04)

Bản EN (canonical): `docs/en/specs/2026-07-04-trino-config-expansion-design.md`.
Nối tiếp Phase 1.5: `docs/en/specs/2026-07-03-engine-config-catalog-design.md` (#136).

**Mục tiêu:** Mở rộng config Trino egret expose lên "tier RED" (theo chart chính thức):
1. **Per-role** — coordinator + worker mỗi bên 1 block độc lập (replicas, resources,
   JVM heap, query.max-memory-per-node, additionalJvmConfig, probes).
2. **JVM** — heap fixed HOẶC theo % container (chart khuyến nghị) + raw additionalJvmConfig.
3. **Worker graceful shutdown** — drain query đang chạy trước khi kill.
4. **Probes** cấu hình được (liveness/readiness/startup per-role).
5. **Log level** + **resources tách request/limit**.

**Non-goals:** autoscaling, fault-tolerant execution, TLS/auth (→ Phase 1.6 proxy),
scheduling, observability, session/event/access-control. GC giữ default cứng (G1GC 32M).

## Data model — TrinoConfig per-role (source of truth duy nhất)
Bỏ field flat top-level (coordinatorReplicas/workerReplicas/cpuMin/cpuMax/memory) →
gộp hết vào `trinoConfig` (lưu ở `engine.spec.trinoConfig`):
```
TrinoConfig { queryMaxMemory?, logLevel?, configProperties, env, coordinator: RoleConfig, worker: WorkerRoleConfig }
RoleConfig { replicas, resources{cpuRequest?,cpuLimit?,memRequest?,memLimit?},
             heap{maxSize? XOR maxPercent?, initialPercent?, minSize?},
             queryMaxMemoryPerNode?, additionalJvmConfig[], probes{liveness?,readiness?,startup?} }
Probe { initialDelaySeconds?, periodSeconds?, timeoutSeconds?, failureThreshold?, successThreshold? }
WorkerRoleConfig = RoleConfig + gracefulShutdown?{enabled, gracePeriodSeconds}
```
heap.maxSize XOR maxPercent (maxSize thắng nếu cả 2). **Back-compat**: row cũ thiếu
`trinoConfig.coordinator` → synth per-role default từ field flat cũ (không migration DB).

## Orchestrator (trino.rs)
- `jvm_config(role)`: `-Xmx{maxSize}`+`-Xms{minSize}` HOẶC `-XX:MaxRAMPercentage`+`InitialRAMPercentage`;
  GC default; append `additionalJvmConfig`.
- `config_properties(role)`: per-role `query.max-memory-per-node`; cluster `query.max-memory`;
  worker + `shutdown.grace-period` khi bật gracefulShutdown.
- Thêm file **`log.properties`** (`io.trino=<level>`) vào config ConfigMap.
- `trino_deployment(role)`: resources request/limit tách; 3 probe (httpGet /v1/info) nếu set
  (mặc định giữ readiness generous, không liveness); **worker preStop**:
  `curl -X PUT -d '"SHUTTING_DOWN"' .../v1/info/state` + `tail --pid=1 -f /dev/null`,
  `terminationGracePeriodSeconds = max(2×grace, grace+30)`.

## Control-plane / Frontend / Test
- `EngineProvisionReq` + carrier reshape per-role; `build_provision_req` đọc per-role (+ fallback).
  **Create wizard giữ đơn giản** (size cơ bản → seed default trinoConfig); tinh chỉnh per-role ở PUT.
- Frontend EngineDetail: **Cluster** (queryMaxMemory/logLevel/raw) + tab **Coordinator** + tab
  **Worker** (replicas, resources, heap fixed/%, maxMemoryPerNode, additionalJvmConfig, 3 probe;
  worker thêm gracefulShutdown). Engine list/detail đọc replicas/resources từ `trinoConfig.<role>`.
- Test: unit (heap fixed/%, additionalJvmConfig, resources split, 3 probe, worker preStop +
  termination≥2×grace, log.properties, back-compat) + live kind (set heap% + gracefulShutdown +
  probe → roll → kiểm pod spec + query chạy).

Chi tiết đầy đủ xem bản EN.
