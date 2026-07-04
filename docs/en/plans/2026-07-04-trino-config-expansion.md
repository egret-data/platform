# Trino Config Expansion (Phase 1.5.1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give each Trino role (coordinator/worker) an independent config block —
replicas, resources (request/limit), JVM heap (fixed or percent) + extra JVM flags,
per-node query memory, log level, configurable probes, and worker graceful shutdown.

**Architecture:** Reshape the orchestrator `TrinoConfig` into a per-role structure
(single source of truth; drops the old flat replicas/resources), render per-role
`jvm.config`/`config.properties`/probes/preStop from it, thread it through the
control-plane (with a back-compat read for old flat specs) and egret-bin, and split
the EngineDetail config UI into Cluster + Coordinator + Worker tabs. Config changes
roll the Deployments (already wired via the config-checksum annotation).

**Tech Stack:** Rust (serde_json manifest templating, kube-rs), axum control-plane,
React/TS. Modelled on the official `trinodb/trino` Helm chart.

**Spec:** `docs/en/specs/2026-07-04-trino-config-expansion-design.md`.
**Chart reference:** `/Users/1pro-l4r8/Documents/github/data-portal/trino-charts/charts/trino`.

---

## File Structure

- **Modify** `crates/egret-orchestrator/src/spec.rs` — per-role `TrinoConfig`, `RoleConfig`, `WorkerRoleConfig`, `HeapConfig`, `RoleResources`, `Probe`, `GracefulShutdown`; `EngineSpec` drops top-level replicas/resources.
- **Modify** `crates/egret-orchestrator/src/trino.rs` — per-role `jvm_config`/`config_properties`, new `log_properties`, per-role `trino_deployment` (resources split, probes, worker preStop).
- **Modify** `crates/egret-controlplane/src/engines.rs` — per-role carriers, `trino_config_from_json`, `build_provision_req` (+ back-compat), create seed, update.
- **Modify** `crates/egret-bin/src/main.rs` — per-role req → per-role `EngineSpec`.
- **Modify** `frontend/src/api/controlPlane.ts` — per-role `TrinoConfig` types.
- **Modify** `frontend/src/routes/control-plane/EngineDetail.tsx` — Cluster + Coordinator + Worker tabs.
- **Modify** `frontend/src/routes/control-plane/ControlPlaneEngines.tsx` — read replicas from `trinoConfig` if the list shows it.

---

## Task 1: Orchestrator types → per-role `TrinoConfig`

**Files:** Modify `crates/egret-orchestrator/src/spec.rs`, `lib.rs`

- [ ] **Step 1: Replace the flat config + reshape `EngineSpec`**

Remove the old `TrinoConfig` (flat) and the `Resources` struct usage on `EngineSpec`
top-level. Add:

```rust
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct HeapConfig {
    pub max_size: Option<String>,      // "-Xmx" when set
    pub min_size: Option<String>,      // "-Xms"
    pub max_percent: Option<u8>,       // -XX:MaxRAMPercentage (used only if max_size is None)
    pub initial_percent: Option<u8>,   // -XX:InitialRAMPercentage
}

#[derive(Clone, Debug, Default)]
pub struct RoleResources {
    pub cpu_request: Option<String>,
    pub cpu_limit: Option<String>,
    pub mem_request: Option<String>,
    pub mem_limit: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub initial_delay_seconds: Option<u32>,
    pub period_seconds: Option<u32>,
    pub timeout_seconds: Option<u32>,
    pub failure_threshold: Option<u32>,
    pub success_threshold: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Probes {
    pub liveness: Option<Probe>,
    pub readiness: Option<Probe>,
    pub startup: Option<Probe>,
}

#[derive(Clone, Debug, Default)]
pub struct RoleConfig {
    pub replicas: u32,
    pub resources: RoleResources,
    pub heap: HeapConfig,
    pub query_max_memory_per_node: Option<String>,
    pub additional_jvm_config: Vec<String>,
    pub probes: Probes,
}

#[derive(Clone, Debug, Default)]
pub struct GracefulShutdown {
    pub enabled: bool,
    pub grace_period_seconds: u32,
}

#[derive(Clone, Debug, Default)]
pub struct WorkerRoleConfig {
    pub base: RoleConfig,
    pub graceful_shutdown: Option<GracefulShutdown>,
}

#[derive(Clone, Debug, Default)]
pub struct TrinoConfig {
    pub query_max_memory: Option<String>,
    pub log_level: Option<String>,
    pub config_properties: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub coordinator: RoleConfig,
    pub worker: WorkerRoleConfig,
}

#[derive(Clone, Debug)]
pub struct EngineSpec {
    pub name: String,
    pub namespace: String,
    pub image_version: String,
    pub config: TrinoConfig,
    pub catalogs: Vec<CatalogSpec>,
}
```

Keep `CatalogSpec`, `EngineStatus`, `PodStatus`. Delete the old `Resources` struct if
nothing else uses it (grep first; `trino_deployment` in Task 3 uses `RoleResources` now).

- [ ] **Step 2: Update `lib.rs` re-exports**

```rust
pub use spec::{
    CatalogSpec, EngineSpec, EngineStatus, GracefulShutdown, HeapConfig, PodStatus,
    Probe, Probes, RoleConfig, RoleResources, TrinoConfig, WorkerRoleConfig,
};
```

- [ ] **Step 3: Build (trino.rs errors expected)**

Run: `cargo build -p egret-orchestrator 2>&1 | grep error | grep -v "trino.rs" | head`
Expected: empty (all errors confined to trino.rs — Tasks 2/3 fix them).

- [ ] **Step 4: fmt + commit**

```bash
cargo fmt --all
git add crates/egret-orchestrator/src/spec.rs crates/egret-orchestrator/src/lib.rs
git commit -m "refactor(orchestrator): per-role TrinoConfig (heap/resources/probes/graceful-shutdown)"
```

---

## Task 2: Orchestrator rendering — per-role jvm.config / config.properties / log.properties (TDD)

**Files:** Modify `crates/egret-orchestrator/src/trino.rs`

- [ ] **Step 1: Failing tests** (replace the `sample_spec()` + config tests)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::*;
    use std::collections::BTreeMap;

    fn role(replicas: u32) -> RoleConfig {
        RoleConfig { replicas, ..Default::default() }
    }
    fn sample_spec() -> EngineSpec {
        let mut coord = role(1);
        coord.heap.max_size = Some("2G".into());
        coord.query_max_memory_per_node = Some("1GB".into());
        let mut worker = role(2);
        worker.heap.max_percent = Some(80);
        worker.additional_jvm_config = vec!["-XX:MaxHeapFreeRatio=100".into()];
        EngineSpec {
            name: "demo".into(), namespace: "egret-engines".into(), image_version: "470".into(),
            config: TrinoConfig {
                query_max_memory: Some("4GB".into()), log_level: Some("DEBUG".into()),
                config_properties: BTreeMap::new(), env: BTreeMap::new(),
                coordinator: coord,
                worker: WorkerRoleConfig {
                    base: worker,
                    graceful_shutdown: Some(GracefulShutdown { enabled: true, grace_period_seconds: 120 }),
                },
            },
            catalogs: vec![],
        }
    }

    #[test]
    fn coordinator_jvm_uses_fixed_heap() {
        let j = jvm_config(&sample_spec().config.coordinator);
        assert!(j.contains("-Xmx2G"));
        assert!(!j.contains("MaxRAMPercentage"));
    }
    #[test]
    fn worker_jvm_uses_percent_and_extra_flags() {
        let s = sample_spec();
        let j = jvm_config(&s.config.worker.base);
        assert!(j.contains("-XX:MaxRAMPercentage=80"));
        assert!(!j.contains("-Xmx"));
        assert!(j.contains("-XX:MaxHeapFreeRatio=100"));
    }
    #[test]
    fn config_properties_per_role_memory_and_shutdown() {
        let s = sample_spec();
        let cm = trino_config_map(&s);
        let cc = cm["data"]["config-coordinator.properties"].as_str().unwrap();
        assert!(cc.contains("query.max-memory=4GB"));
        assert!(cc.contains("query.max-memory-per-node=1GB"));
        let wc = cm["data"]["config-worker.properties"].as_str().unwrap();
        assert!(wc.contains("shutdown.grace-period=120s"));
    }
    #[test]
    fn log_properties_carries_level() {
        let cm = trino_config_map(&sample_spec());
        assert!(cm["data"]["log.properties"].as_str().unwrap().contains("io.trino=DEBUG"));
    }
}
```

- [ ] **Step 2: Run — expect failure**

Run: `cargo test -p egret-orchestrator trino::tests`
Expected: FAIL (signatures changed / functions missing).

- [ ] **Step 3: Implement the per-role config rendering**

Replace `jvm_config`, `config_properties`, and extend `trino_config_map`:

```rust
use crate::spec::{RoleConfig, TrinoConfig};

fn jvm_config(role: &RoleConfig) -> String {
    let mut lines = vec!["-server".to_string()];
    if let Some(sz) = &role.heap.max_size {
        lines.push(format!("-Xmx{sz}"));
        lines.push(format!("-Xms{}", role.heap.min_size.clone().unwrap_or_else(|| sz.clone())));
    } else if let Some(pct) = role.heap.max_percent {
        lines.push(format!("-XX:MaxRAMPercentage={pct}"));
        if let Some(ip) = role.heap.initial_percent { lines.push(format!("-XX:InitialRAMPercentage={ip}")); }
    } else {
        lines.push("-Xmx1G".into());
        lines.push("-Xms1G".into());
    }
    lines.extend([
        "-XX:+UseG1GC".into(), "-XX:G1HeapRegionSize=32M".into(),
        "-XX:+ExplicitGCInvokesConcurrent".into(), "-XX:+HeapDumpOnOutOfMemoryError".into(),
        "-XX:+ExitOnOutOfMemoryError".into(), "-XX:-OmitStackTraceInFastThrow".into(),
        "-Djdk.attach.allowAttachSelf=true".into(),
    ]);
    lines.extend(role.additional_jvm_config.iter().cloned());
    lines.join("\n") + "\n"
}

fn config_properties(spec: &EngineSpec, coord: bool) -> String {
    let cfg = &spec.config;
    let role = if coord { &cfg.coordinator } else { &cfg.worker.base };
    let mut out = String::new();
    if coord {
        out.push_str("coordinator=true\n");
        let inc = cfg.worker.base.replicas == 0;
        out.push_str(&format!("node-scheduler.include-coordinator={inc}\n"));
        out.push_str("discovery.uri=http://localhost:8080\n");
    } else {
        out.push_str("coordinator=false\n");
        out.push_str(&format!("discovery.uri=http://{}-trino:8080\n", spec.name));
    }
    out.push_str("http-server.http.port=8080\n");
    if let Some(v) = &cfg.query_max_memory { out.push_str(&format!("query.max-memory={v}\n")); }
    if let Some(v) = &role.query_max_memory_per_node { out.push_str(&format!("query.max-memory-per-node={v}\n")); }
    if !coord {
        if let Some(gs) = &cfg.worker.graceful_shutdown {
            if gs.enabled { out.push_str(&format!("shutdown.grace-period={}s\n", gs.grace_period_seconds)); }
        }
    }
    for (k, v) in &cfg.config_properties { out.push_str(&format!("{k}={v}\n")); }
    out
}

fn log_properties(spec: &EngineSpec) -> String {
    format!("io.trino={}\n", spec.config.log_level.clone().unwrap_or_else(|| "INFO".into()))
}
```

In `trino_config_map`, add `"log.properties": log_properties(spec)` to the `data`
map (alongside the two config-*.properties + jvm.config + node.properties). NOTE
`jvm.config` in the config map is now built per-role too — the map needs
`jvm-coordinator.config` + `jvm-worker.config` (like the config-*.properties split),
each `jvm_config(&spec.config.coordinator)` / `jvm_config(&spec.config.worker.base)`;
update the Deployment mount in Task 3 to subPath the right jvm file to
`/etc/trino/jvm.config`. Update the existing `trino_config_map` test if it asserts a
single `jvm.config` key.

- [ ] **Step 4: Run — expect pass**

Run: `cargo test -p egret-orchestrator trino::tests`
Expected: PASS.

- [ ] **Step 5: fmt + commit**

```bash
cargo fmt --all
git add crates/egret-orchestrator/src/trino.rs
git commit -m "feat(orchestrator): per-role jvm.config (fixed/percent + extra flags), per-role config, log.properties"
```

---

## Task 3: Orchestrator rendering — Deployment resources split, probes, worker preStop (TDD)

**Files:** Modify `crates/egret-orchestrator/src/trino.rs`

- [ ] **Step 1: Failing tests** (append to `mod tests`)

```rust
#[test]
fn deployment_resources_split_requests_limits() {
    let mut s = sample_spec();
    s.config.coordinator.resources = RoleResources {
        cpu_request: Some("1".into()), cpu_limit: Some("2".into()),
        mem_request: Some("2Gi".into()), mem_limit: Some("4Gi".into()),
    };
    let d = trino_deployment(&s, Role::Coordinator);
    let res = &d["spec"]["template"]["spec"]["containers"][0]["resources"];
    assert_eq!(res["requests"]["cpu"], "1");
    assert_eq!(res["limits"]["cpu"], "2");
    assert_eq!(res["requests"]["memory"], "2Gi");
    assert_eq!(res["limits"]["memory"], "4Gi");
}
#[test]
fn deployment_renders_custom_probes() {
    let mut s = sample_spec();
    s.config.coordinator.probes.readiness = Some(Probe { timeout_seconds: Some(7), failure_threshold: Some(20), ..Default::default() });
    s.config.coordinator.probes.liveness = Some(Probe { initial_delay_seconds: Some(60), ..Default::default() });
    let d = trino_deployment(&s, Role::Coordinator);
    let c = &d["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(c["readinessProbe"]["timeoutSeconds"], 7);
    assert_eq!(c["readinessProbe"]["failureThreshold"], 20);
    assert_eq!(c["livenessProbe"]["httpGet"]["path"], "/v1/info");
    assert_eq!(c["livenessProbe"]["initialDelaySeconds"], 60);
}
#[test]
fn worker_graceful_shutdown_prestop_and_termination() {
    let d = trino_deployment(&sample_spec(), Role::Worker);
    let spec = &d["spec"]["template"]["spec"];
    // termination >= 2 * gracePeriod (120) => 240
    assert_eq!(spec["terminationGracePeriodSeconds"], 240);
    let cmd = spec["containers"][0]["lifecycle"]["preStop"]["exec"]["command"][2].as_str().unwrap();
    assert!(cmd.contains("SHUTTING_DOWN"));
    assert!(cmd.contains("/v1/info/state"));
    assert!(cmd.contains("tail --pid=1"));
}
#[test]
fn coordinator_has_no_prestop() {
    let d = trino_deployment(&sample_spec(), Role::Coordinator);
    assert!(d["spec"]["template"]["spec"]["containers"][0]["lifecycle"].is_null());
}
```

- [ ] **Step 2: Run — expect failure**

Run: `cargo test -p egret-orchestrator trino::tests`
Expected: FAIL.

- [ ] **Step 3: Implement `trino_deployment`** (rewrite the resources/probe/lifecycle parts)

```rust
fn probe_json(defaults_ready: bool, p: Option<&crate::spec::Probe>) -> Value {
    // httpGet /v1/info on 8080, merged with any provided fields; readiness default is generous.
    let (mut init, mut period, mut timeout, mut fail, mut succ) =
        if defaults_ready { (15u32, 10, 5, 12, 1) } else { (10, 10, 5, 3, 1) };
    if let Some(p) = p {
        if let Some(v) = p.initial_delay_seconds { init = v; }
        if let Some(v) = p.period_seconds { period = v; }
        if let Some(v) = p.timeout_seconds { timeout = v; }
        if let Some(v) = p.failure_threshold { fail = v; }
        if let Some(v) = p.success_threshold { succ = v; }
    }
    json!({ "httpGet": { "path": "/v1/info", "port": 8080 },
            "initialDelaySeconds": init, "periodSeconds": period, "timeoutSeconds": timeout,
            "failureThreshold": fail, "successThreshold": succ })
}

pub fn trino_deployment(spec: &EngineSpec, role: Role) -> Value {
    let app = format!("{}-{}", spec.name, role.suffix());
    let (rc, jvm_file, cfg_file, replicas) = if role.is_coord() {
        (&spec.config.coordinator, "jvm-coordinator.config", "config-coordinator.properties", spec.config.coordinator.replicas)
    } else {
        (&spec.config.worker.base, "jvm-worker.config", "config-worker.properties", spec.config.worker.base.replicas)
    };

    // resources: only include a side if any of its values are set; fall back to nothing.
    let mut requests = serde_json::Map::new();
    let mut limits = serde_json::Map::new();
    if let Some(v) = &rc.resources.cpu_request { requests.insert("cpu".into(), json!(v)); }
    if let Some(v) = &rc.resources.mem_request { requests.insert("memory".into(), json!(v)); }
    if let Some(v) = &rc.resources.cpu_limit { limits.insert("cpu".into(), json!(v)); }
    if let Some(v) = &rc.resources.mem_limit { limits.insert("memory".into(), json!(v)); }
    let mut resources = serde_json::Map::new();
    if !requests.is_empty() { resources.insert("requests".into(), Value::Object(requests)); }
    if !limits.is_empty() { resources.insert("limits".into(), Value::Object(limits)); }

    // probes: readiness always (generous default), liveness/startup only if configured.
    let mut container = serde_json::json!({
        "name": "trino",
        "image": format!("trinodb/trino:{}", spec.image_version),
        "ports": [{ "containerPort": 8080 }],
        "envFrom": [{ "secretRef": { "name": format!("{}-env", spec.name) } }],
        "resources": Value::Object(resources),
        "readinessProbe": probe_json(true, rc.probes.readiness.as_ref()),
        "volumeMounts": [
            { "name": "config", "mountPath": "/etc/trino/config.properties", "subPath": cfg_file },
            { "name": "config", "mountPath": "/etc/trino/jvm.config", "subPath": jvm_file },
            { "name": "config", "mountPath": "/etc/trino/node.properties", "subPath": "node.properties" },
            { "name": "config", "mountPath": "/etc/trino/log.properties", "subPath": "log.properties" },
            { "name": "catalogs", "mountPath": "/etc/trino/catalog" },
            { "name": "data", "mountPath": "/data/trino" }
        ]
    });
    if let Some(p) = rc.probes.liveness.as_ref() { container["livenessProbe"] = probe_json(false, Some(p)); }
    if let Some(p) = rc.probes.startup.as_ref() { container["startupProbe"] = probe_json(false, Some(p)); }

    // pod-spec: worker graceful shutdown adds a preStop + terminationGracePeriodSeconds.
    let mut pod_spec = serde_json::json!({
        "containers": [container],
        "volumes": [
            { "name": "config", "configMap": { "name": format!("{}-config", spec.name) } },
            { "name": "catalogs", "configMap": { "name": format!("{}-catalogs", spec.name) } },
            { "name": "data", "emptyDir": {} }
        ]
    });
    if !role.is_coord() {
        if let Some(gs) = &spec.config.worker.graceful_shutdown {
            if gs.enabled {
                let grace = gs.grace_period_seconds;
                let term = std::cmp::max(grace * 2, grace + 30);
                pod_spec["terminationGracePeriodSeconds"] = json!(term);
                let cmd = format!(
                    "curl -s -X PUT -d '\"SHUTTING_DOWN\"' -H 'Content-type: application/json' -H 'X-Trino-User: admin' http://localhost:8080/v1/info/state\ntail --pid=1 -f /dev/null"
                );
                pod_spec["containers"][0]["lifecycle"] = json!({ "preStop": { "exec": { "command": ["/bin/sh", "-c", cmd] } } });
            }
        }
    }

    json!({
        "apiVersion": "apps/v1", "kind": "Deployment",
        "metadata": { "name": app, "namespace": spec.namespace,
                      "labels": { "app": app, "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "spec": {
            "replicas": replicas,
            "selector": { "matchLabels": { "app": app } },
            "template": {
                "metadata": { "labels": { "app": app, "app.kubernetes.io/instance": spec.name, "egret/engine": spec.name },
                              "annotations": { "egret/config-checksum": config_checksum(spec) } },
                "spec": pod_spec
            }
        }
    })
}
```

Also update `trino_config_map` to emit `jvm-coordinator.config` +
`jvm-worker.config` (per Task 2 note) instead of a single `jvm.config`.

- [ ] **Step 4: Run all orchestrator tests + clippy**

Run: `cargo test -p egret-orchestrator && cargo clippy -p egret-orchestrator`
Expected: all pass, clippy clean. (The Task-2/3 provisioner tests `provision_applies_all_objects` etc. still pass — object names unchanged.)

- [ ] **Step 5: fmt + commit**

```bash
cargo fmt --all
git add crates/egret-orchestrator/src/trino.rs
git commit -m "feat(orchestrator): per-role resources/probes + worker graceful-shutdown preStop"
```

---

## Task 4: Control-plane — per-role carriers + back-compat + create/update

**Files:** Modify `crates/egret-controlplane/src/engines.rs`; Test `tests/routes_api.rs`

- [ ] **Step 1** Reshape the kube-free carrier `TrinoConfigReq` into the per-role
mirror of the orchestrator types (`RoleConfigReq`, `WorkerRoleConfigReq`,
`HeapReq`, `RoleResourcesReq`, `ProbeReq`, `ProbesReq`, `GracefulShutdownReq`) and
put `coordinator`/`worker` on it; `EngineProvisionReq` drops the flat
`coordinator_replicas`/`worker_replicas`/`cpu_*`/`memory` fields (they live in
`config.coordinator`/`worker` now). Export the new carrier types from `lib.rs`.

- [ ] **Step 2** Rewrite `trino_config_from_json(v)` to parse the nested per-role
`trinoConfig` wire shape (camelCase: `queryMaxMemory`, `logLevel`, `configProperties`,
`env`, `coordinator{replicas,resources{cpuRequest,cpuLimit,memRequest,memLimit},heap{maxSize,minSize,maxPercent,initialPercent},queryMaxMemoryPerNode,additionalJvmConfig,probes{liveness,readiness,startup}}`, `worker{...,gracefulShutdown{enabled,gracePeriodSeconds}}`). Each field optional with sensible defaults.

- [ ] **Step 3** `build_provision_req(st, engine)` reads `engine.spec["trinoConfig"]`
via `trino_config_from_json`. **Back-compat:** if `trinoConfig.coordinator` is
absent (old flat spec), synthesise it: `coordinator.replicas` from
`engine.spec["coordinatorReplicas"]` (default 1), `worker.replicas` from
`workerReplicas`, both roles' resources from `cpuMin`/`cpuMax`/`memory`
(→ cpu_request=cpuMin, cpu_limit=cpuMax, mem_request=mem_limit=memory), both roles'
`heap.max_size` from the old `trinoConfig.jvmHeap` if present, both roles'
`query_max_memory_per_node` from the old `trinoConfig.queryMaxMemoryPerNode`.

- [ ] **Step 4** `create_engine`: keep the simple wizard body
(`coordinatorReplicas`/`workerReplicas`/`cpuMin`/`cpuMax`/`memory`), but on create
**seed** `engine.spec.trinoConfig` in the new per-role shape from those inputs
(both roles same resources, coordinator replicas from body, worker replicas from
body). `update_engine` persists the full per-role `trinoConfig` from the body.

- [ ] **Step 5** Route test: after `create_engine`, `GET /engines/{id}` returns
`spec.trinoConfig.coordinator.replicas` etc.; `PUT` with a per-role `trinoConfig`
(coordinator heap maxSize, worker gracefulShutdown) round-trips. Run
`DATABASE_URL=... cargo test -p egret-controlplane` (NOT the whole workspace).

- [ ] **Step 6** `cargo fmt --all`; commit
`feat(controlplane): per-role trinoConfig carriers + back-compat read + create seed`.
(egret-bin won't compile until Task 5 — expected; verify `cargo build -p egret-controlplane` in isolation.)

---

## Task 5: egret-bin adapter — per-role req → per-role EngineSpec

**Files:** Modify `crates/egret-bin/src/main.rs`

- [ ] **Step 1** In `EgretEngineProvisioner`'s `EngineProvisionReq`→`EngineSpec`
mapping, build the orchestrator per-role `TrinoConfig` from `req.config`: map
`coordinator`/`worker` `RoleConfigReq`→`RoleConfig` (replicas, `RoleResources`,
`HeapConfig`, `query_max_memory_per_node`, `additional_jvm_config`, `Probes` with
each `Probe`), the worker's `GracefulShutdown`, and the cluster-wide
`query_max_memory`/`log_level`/`config_properties`/`env`. Map `req.catalogs` →
`Vec<CatalogSpec>` as before. `EngineSpec` no longer has top-level replicas/resources.

- [ ] **Step 2** `cargo build --workspace 2>&1 | grep error` → empty. `cargo fmt --all -- --check`. Commit `feat(bin): map per-role trinoConfig to the orchestrator EngineSpec`.

---

## Task 6: Frontend — api types + EngineDetail Cluster/Coordinator/Worker tabs

**Files:** Modify `frontend/src/api/controlPlane.ts`, `frontend/src/routes/control-plane/EngineDetail.tsx`, `ControlPlaneEngines.tsx`

- [ ] **Step 1** Reshape the TS `TrinoConfig` interface to per-role (mirror the wire
shape from Task 2): `{ queryMaxMemory?, logLevel?, configProperties?, env?, coordinator: RoleConfig, worker: WorkerRoleConfig }`, `RoleConfig { replicas?, resources?: {cpuRequest?,cpuLimit?,memRequest?,memLimit?}, heap?: {maxSize?,minSize?,maxPercent?,initialPercent?}, queryMaxMemoryPerNode?, additionalJvmConfig?: string[], probes?: {liveness?,readiness?,startup?} }`, `Probe { initialDelaySeconds?, periodSeconds?, timeoutSeconds?, failureThreshold?, successThreshold? }`, `WorkerRoleConfig = RoleConfig & { gracefulShutdown?: {enabled, gracePeriodSeconds} }`. `tsc --noEmit`.

- [ ] **Step 2** EngineDetail config section → a tabbed card: **Cluster** (queryMaxMemory,
logLevel `<Select>` INFO/DEBUG/WARN/ERROR, raw configProperties/env KV editors),
**Coordinator** tab, **Worker** tab. Each role tab: replicas (number); resources
(cpuRequest/cpuLimit/memRequest/memLimit); heap — a radio "Fixed size | Percent"
switching between a maxSize/minSize input pair and a maxPercent/initialPercent pair;
queryMaxMemoryPerNode; an additionalJvmConfig line editor (string[]); three collapsible
probe editors (initialDelay/period/timeout/failure/success). Worker tab adds a
Graceful shutdown toggle + gracePeriodSeconds. Prefill from `engine.spec.trinoConfig`
(now per-role; the DTO already surfaces it). Save → `updateEngine` with the full
`trinoConfig` → "restarting to apply" note. Reuse the Phase-1.5 `KeyValueEditor`.

- [ ] **Step 3** In `ControlPlaneEngines.tsx`, if the list shows replicas/size, read
them from `engine.spec.trinoConfig?.worker?.replicas` (with a fallback to the old
`engine.spec.workerReplicas`). `tsc` / `eslint` / `vite build`. Commit
`feat(ui): per-role Trino config editor (Cluster/Coordinator/Worker tabs)`.

---

## Task 7: Full gate + live test + PR

- [ ] **Step 1** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`.
- [ ] **Step 2** Frontend `tsc` / `eslint` / `vitest run` / `vite build`.
- [ ] **Step 3** Live on kind: rebuild egret + restart serve; provision a fresh engine; on the detail page set coordinator heap `maxPercent=75`, worker `additionalJvmConfig=["-XX:MaxHeapFreeRatio=100"]` + graceful shutdown (grace 30) + a readiness `timeoutSeconds=8`; Save → engine rolls; `kubectl -n egret-engines get pod <coordinator> -o yaml` shows `-XX:MaxRAMPercentage=75` in jvm.config, `kubectl get pod <worker> -o yaml` shows the preStop + `terminationGracePeriodSeconds: 60`; run `SHOW SCHEMAS FROM egret` to confirm it still queries.
- [ ] **Step 4** Commit; open PR; run the full local gate before an admin squash-merge (GitHub CI out of quota).

---

## Self-Review notes

- **Spec coverage:** per-role model (Task 1) ✓; heap fixed/percent + additionalJvmConfig (Task 2) ✓; per-role config.properties + query.max-memory-per-node + log.properties + shutdown.grace-period (Task 2) ✓; resources request/limit split + configurable probes + worker preStop/terminationGracePeriod (Task 3) ✓; control-plane carriers + back-compat + create seed (Task 4) ✓; egret-bin (Task 5) ✓; per-role UI tabs (Task 6) ✓; live verify (Task 7) ✓.
- **Type consistency:** orchestrator `RoleConfig`/`HeapConfig`/`RoleResources`/`Probe`/`GracefulShutdown` ↔ control-plane `*Req` carriers ↔ TS interfaces ↔ wire `trinoConfig.{coordinator,worker}` — field names line up (max_size↔maxSize, cpu_request↔cpuRequest, grace_period_seconds↔gracePeriodSeconds). `EngineSpec` drops top-level replicas/resources in Task 1; every reader (trino.rs Tasks 2/3, egret-bin Task 5) updated.
- **jvm.config split:** Task 2 changes the config ConfigMap to per-role `jvm-{role}.config`; Task 3's Deployment subPath-mounts the right one to `/etc/trino/jvm.config` — the two are consistent.
- **Deferred (own increments):** autoscaling, fault-tolerant execution, TLS/auth (Phase 1.6), scheduling, observability.
