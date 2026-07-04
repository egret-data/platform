# Trino config expansion — per-role config, JVM, graceful shutdown, probes (Phase 1.5.1, 2026-07-04)

VI reference: `docs/vi/specs/2026-07-04-trino-config-expansion-design.md`.
Builds on Phase 1.5: `docs/en/specs/2026-07-03-engine-config-catalog-design.md` (shipped, #136).

**Goal:** Expand the Trino config surface egret exposes from the current small set
(shared JVM heap, cluster `query.max-memory`, per-node memory, raw
`config.properties`/env) to the operationally-important "tier RED" set, modelled
on the official `trinodb/trino` Helm chart:
1. **Per-role config** — coordinator and worker each get an independent block
   (replicas, resources, JVM heap, `query.max-memory-per-node`, additional JVM
   flags, probes).
2. **JVM** — heap as a fixed size **or** a percentage of the container limit
   (chart-recommended), plus raw `additionalJvmConfig` lines.
3. **Worker graceful shutdown** — drain running queries before termination.
4. **Configurable probes** — liveness / readiness / startup per role.
5. **Log level** + **resources split** (requests ≠ limits).

**Non-goals (this increment):** autoscaling (HPA/KEDA), fault-tolerant execution
(exchange manager), TLS/auth (Phase 1.6 proxy territory), scheduling
(nodeSelector/affinity), observability (JMX/Prometheus), session/event/access
control. GC method stays hardcoded (G1GC, 32M region) — not exposed (YAGNI).

## Data model — `TrinoConfig` becomes per-role (single source of truth)

The flat top-level engine-spec fields (`coordinatorReplicas`, `workerReplicas`,
`cpuMin`, `cpuMax`, `memory`) are **dropped**; everything moves into `trinoConfig`
so there is one source of truth. Wire shape (camelCase), stored at
`engine.spec.trinoConfig`:

```
TrinoConfig {
  // cluster-wide
  queryMaxMemory?: string          // config.properties query.max-memory
  logLevel?: string                // log.properties io.trino=<level> (default INFO)
  configProperties: Record<string,string>   // raw, both roles
  env: Record<string,string>       // both roles
  coordinator: RoleConfig
  worker: WorkerRoleConfig
}
RoleConfig {
  replicas: number
  resources: { cpuRequest?, cpuLimit?, memRequest?, memLimit?: string }
  heap: { maxSize?: string, maxPercent?: number, initialPercent?: number, minSize?: string }  // maxSize XOR maxPercent
  queryMaxMemoryPerNode?: string   // config.properties query.max-memory-per-node
  additionalJvmConfig: string[]    // extra jvm.config lines
  probes: { liveness?: Probe, readiness?: Probe, startup?: Probe }
}
Probe { initialDelaySeconds?, periodSeconds?, timeoutSeconds?, failureThreshold?, successThreshold?: number }
WorkerRoleConfig = RoleConfig & { gracefulShutdown?: { enabled: boolean, gracePeriodSeconds: number } }
```

`heap.maxSize` and `heap.maxPercent` are mutually exclusive; if both are set,
`maxSize` wins and a warning surfaces (documented precedence).

### Back-compat read
Existing dev engines store the OLD flat spec. `build_provision_req` falls back:
if `trinoConfig.coordinator` is absent, it synthesises per-role defaults from the
old flat fields (`coordinatorReplicas`→coordinator.replicas, `cpuMin/cpuMax/memory`
→ both roles' resources, `jvmHeap`→both roles' `heap.maxSize`,
`queryMaxMemoryPerNode`→both roles). No DB migration; old rows keep working, new
saves write the new shape.

## Orchestrator rendering (`trino.rs`)

- **`jvm_config(spec, role)`** — per role: `-server`, then heap: `-Xmx{maxSize}` +
  `-Xms{minSize}` if `maxSize` set, else `-XX:MaxRAMPercentage={maxPercent}` +
  `-XX:InitialRAMPercentage={initialPercent}` if `maxPercent` set (else the current
  1G default); the hardcoded GC block; then each `additionalJvmConfig` line verbatim.
- **`config_properties(spec, role)`** — coordinator/worker as today, plus per-role
  `query.max-memory-per-node={role.queryMaxMemoryPerNode}`, cluster
  `query.max-memory={queryMaxMemory}`, and for the worker when graceful shutdown is
  enabled: `shutdown.grace-period={gracePeriodSeconds}s`.
- **`log_properties(spec)`** — a new `log.properties` file in the config ConfigMap:
  `io.trino={logLevel|INFO}`. Mounted at `/etc/trino/log.properties` (subPath).
- **`trino_deployment(spec, role)`**:
  - resources: `requests` from `{cpuRequest, memRequest}`, `limits` from
    `{cpuLimit, memLimit}` (omit a side if unset; keep the current single-value
    behaviour as the fallback so nothing regresses).
  - probes: render `livenessProbe`/`readinessProbe`/`startupProbe` from the per-role
    `probes.*` (each an `httpGet /v1/info` on 8080 merged with the provided
    initialDelay/period/timeout/failure/success); if a probe is unset, keep the
    current generous readiness default and NO liveness (safe default).
  - **worker graceful shutdown** (when enabled): a `preStop` exec hook —
    `curl -X PUT -d '"SHUTTING_DOWN"' -H 'Content-type: application/json'
    -H 'X-Trino-User: admin' http://localhost:8080/v1/info/state` then
    `tail --pid=1 -f /dev/null` (blocks until the JVM drains + exits); set
    `terminationGracePeriodSeconds = max(2×gracePeriodSeconds, gracePeriodSeconds+30)`
    (the chart requires ≥ 2×). Coordinator is unaffected.
- The `egret/config-checksum` annotation already covers all of these (hash of the
  rendered ConfigMaps) so any change rolls the pods.

## Control-plane + create/update

- `EngineProvisionReq` + the kube-free carriers reshape to the per-role structure
  (mirrors the orchestrator types, re-declared to keep egret-controlplane kube-free).
- `trino_config_from_json` parses the nested per-role shape; `build_provision_req`
  reads per-role (with the back-compat fallback).
- **Create wizard stays simple**: the create body keeps taking a basic size
  (`workerReplicas` + one cpu/memory) and the handler seeds a default `trinoConfig`
  (both roles same resources, default heap, coordinator replicas 1). Per-role
  fine-tuning happens on the detail page via `PUT`.
- `update_engine` accepts the full per-role `trinoConfig`, persists it, re-renders,
  rolls.

## Frontend (EngineDetail)

The config section splits into three: **Cluster** (`queryMaxMemory`, `logLevel`,
raw `configProperties`/`env`), a **Coordinator** tab, and a **Worker** tab. Each
role tab: replicas, resources (cpu/mem request+limit), heap (a fixed-size input
**or** a percent input, radio-selected), `queryMaxMemoryPerNode`, an
`additionalJvmConfig` line editor, and three collapsible probe editors
(liveness/readiness/startup). The worker tab adds a **Graceful shutdown** toggle +
`gracePeriodSeconds`. Save → `updateEngine` with the full `trinoConfig` → "restarting
to apply" note.

## Test

- **Unit** (fake kube): `jvm_config` renders `-Xmx` for fixed and
  `-XX:MaxRAMPercentage` for percent + appends `additionalJvmConfig`; per-role
  resources split into requests/limits; the three probes render when set; the worker
  preStop + `terminationGracePeriodSeconds ≥ 2×grace` + `shutdown.grace-period`;
  `log.properties` carries the level; back-compat fallback synthesises per-role from
  an old flat spec.
- **Live (kind)**: provision → on the detail page set coordinator heap `maxPercent`,
  worker `additionalJvmConfig` + graceful shutdown + a custom readiness probe → roll
  → `kubectl get pod -o yaml` shows the rendered jvm.config / preStop /
  terminationGracePeriodSeconds / probes, and queries still run.

## Migration / compat notes
- No DB migration; the spec jsonb shape evolves in place with a back-compat read.
- The Engine list + EngineDetail read replicas/resources from `trinoConfig.<role>`
  now (not the old top-level) — update those reads.
