# Engine Phase 1.5 — Trino on plain manifests + config + multi-catalog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provision Trino as plain stateless Kubernetes Deployments that egret
templates itself (dropping the Stackable operator/CRs/PVCs), expose a fuller config
surface, and let a cluster carry multiple catalogs.

**Architecture:** Rewrite `egret-orchestrator`'s Trino provisioner to apply a
coordinator Deployment + worker Deployment + Service + two ConfigMaps (config,
catalogs) + an optional env Secret via the existing `KubeClient` seam. A new
`engine_catalogs` table + a control-plane translation layer turn typed catalog
kinds (egret-warehouse/postgresql/tpch/tpcds/generic) into flat `.properties` files.
Config/catalog changes re-render the ConfigMaps and bump a pod-template checksum
annotation to roll the Deployments (Trino has no hot-reload).

**Tech Stack:** Rust (kube-rs `DynamicObject`, k8s-openapi), axum control-plane,
sqlx/Postgres, React/TS. Official `trinodb/trino` container image.

**Spec:** `docs/en/specs/2026-07-03-engine-config-catalog-design.md`.
**Builds on Phase 1:** `docs/en/plans/2026-07-03-engine-provisioning-trino.md` (shipped).

---

## File Structure

- **Modify** `crates/egret-orchestrator/src/spec.rs` — `EngineSpec` gains `config: TrinoConfig` + `catalogs: Vec<CatalogSpec>`, drops `wiring`/`extra_props`; add `TrinoConfig`, `CatalogSpec`.
- **Rewrite** `crates/egret-orchestrator/src/trino.rs` — manifest templating (Deployment/Service/ConfigMap/Secret) + `TrinoProvisioner`.
- **Create** `migrations/00NN_engine_catalogs.sql` — the `engine_catalogs` table.
- **Modify** `crates/egret-catalog/src/service/embedded.rs` (+ `mod.rs`/`lib.rs`) — `EngineCatalog` + CRUD.
- **Modify** `crates/egret-controlplane/src/engines.rs` — `CatalogReq`/`TrinoConfigReq` on `EngineProvisionReq`, the kind→`CatalogReq` translation, catalog routes, config on `PUT`.
- **Modify** `crates/egret-bin/src/main.rs` — map the richer `EngineProvisionReq` to the orchestrator `EngineSpec`.
- **Modify** `frontend/src/api/controlPlane.ts` — config fields + catalog types/fns.
- **Modify** `frontend/src/routes/control-plane/EngineDetail.tsx` — Config + Catalogs sections.
- **Modify** `deploy/k8s/README.md`, `justfile` — drop the Stackable operator install for Trino.

The control-plane owns all connector-kind logic; the orchestrator only sees flat
`{name, properties, env}` per catalog. This keeps kube out of the kind logic and
the connector logic out of kube.

---

## Task 1: Orchestrator types — `TrinoConfig` + `CatalogSpec`, reshape `EngineSpec`

**Files:**
- Modify: `crates/egret-orchestrator/src/spec.rs`
- Modify: `crates/egret-orchestrator/src/lib.rs` (re-exports)

- [ ] **Step 1: Replace the catalog/props fields + add config in `spec.rs`**

Remove `CatalogWiring`, `CatalogProp`, and the `wiring`/`extra_props` fields. Add:

```rust
use std::collections::BTreeMap;

/// Fuller Trino config surface (curated + raw). Curated keys win over raw on conflict.
#[derive(Clone, Debug, Default)]
pub struct TrinoConfig {
    pub jvm_heap: Option<String>,                 // e.g. "2G" -> -Xmx/-Xms
    pub query_max_memory: Option<String>,         // query.max-memory
    pub query_max_memory_per_node: Option<String>,// query.max-memory-per-node
    pub config_properties: BTreeMap<String, String>, // raw config.properties
    pub env: BTreeMap<String, String>,            // extra env on both roles
}

/// One catalog rendered as `<name>.properties`. `properties` may contain
/// `${ENV:VAR}` references; `env` supplies those vars (resolved secrets) and is
/// merged into the engine's env Secret.
#[derive(Clone, Debug)]
pub struct CatalogSpec {
    pub name: String,
    pub properties: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
}
```

Reshape `EngineSpec` (keep name/namespace/image_version/replicas/resources):

```rust
#[derive(Clone, Debug)]
pub struct EngineSpec {
    pub name: String,
    pub namespace: String,
    pub image_version: String, // official trinodb/trino tag, e.g. "470"
    pub coordinator_replicas: u32,
    pub worker_replicas: u32,
    pub resources: Resources,
    pub config: TrinoConfig,
    pub catalogs: Vec<CatalogSpec>,
}
```

- [ ] **Step 2: Update `lib.rs` re-exports**

Replace the `CatalogProp, CatalogWiring` exports with `CatalogSpec, TrinoConfig`:

```rust
pub use spec::{CatalogSpec, EngineSpec, EngineStatus, PodStatus, Resources, TrinoConfig};
```

- [ ] **Step 3: Build (expect trino.rs errors — fixed in Task 2)**

Run: `cargo build -p egret-orchestrator 2>&1 | grep -E "error\[|wiring|extra_props" | head`
Expected: errors only in `trino.rs` (references to the removed fields). That's fine — Task 2 rewrites it.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-orchestrator/src/spec.rs crates/egret-orchestrator/src/lib.rs
git commit -m "refactor(orchestrator): EngineSpec carries TrinoConfig + CatalogSpec list"
```

---

## Task 2: Rewrite `trino.rs` — plain-manifest templating (TDD)

**Files:**
- Rewrite: `crates/egret-orchestrator/src/trino.rs`

Replace the CR templating with manifest templating. GVKs are plain: `apps/v1`
`Deployment`, `v1` `Service`/`ConfigMap`/`Secret`.

- [ ] **Step 1: Failing tests**

Replace the `#[cfg(test)] mod tests` `sample_spec()` and assertions with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::*;
    use std::collections::BTreeMap;

    fn sample_spec() -> EngineSpec {
        let mut props = BTreeMap::new();
        props.insert("iceberg.catalog.type".into(), "rest".into());
        props.insert("iceberg.rest-catalog.uri".into(), "http://egret:8080/catalog".into());
        props.insert("iceberg.rest-catalog.oauth2.credential".into(), "${ENV:EGRET_C0}".into());
        let mut env = BTreeMap::new();
        env.insert("EGRET_C0".into(), "id:secret".into());
        EngineSpec {
            name: "demo".into(), namespace: "egret-engines".into(), image_version: "470".into(),
            coordinator_replicas: 1, worker_replicas: 2,
            resources: Resources { cpu_min: "1".into(), cpu_max: "2".into(), memory: "4Gi".into() },
            config: TrinoConfig { jvm_heap: Some("2G".into()), query_max_memory: Some("3GB".into()),
                query_max_memory_per_node: None, config_properties: BTreeMap::new(), env: BTreeMap::new() },
            catalogs: vec![CatalogSpec { name: "egret".into(), properties: props, env }],
        }
    }

    #[test]
    fn config_map_has_coordinator_and_worker_config() {
        let cm = trino_config_map(&sample_spec());
        assert_eq!(cm["kind"], "ConfigMap");
        assert_eq!(cm["metadata"]["name"], "demo-config");
        let data = &cm["data"];
        assert!(data["config-coordinator.properties"].as_str().unwrap().contains("coordinator=true"));
        assert!(data["config-coordinator.properties"].as_str().unwrap().contains("node-scheduler.include-coordinator=false"));
        assert!(data["config-coordinator.properties"].as_str().unwrap().contains("query.max-memory=3GB"));
        assert!(data["config-worker.properties"].as_str().unwrap().contains("coordinator=false"));
        assert!(data["config-worker.properties"].as_str().unwrap().contains("discovery.uri=http://demo-trino:8080"));
        assert!(data["jvm.config"].as_str().unwrap().contains("-Xmx2G"));
        assert!(data["node.properties"].as_str().unwrap().contains("node.environment="));
    }

    #[test]
    fn single_node_includes_coordinator() {
        let mut s = sample_spec(); s.worker_replicas = 0;
        let cm = trino_config_map(&s);
        assert!(cm["data"]["config-coordinator.properties"].as_str().unwrap().contains("node-scheduler.include-coordinator=true"));
    }

    #[test]
    fn catalogs_config_map_has_one_file_per_catalog() {
        let cm = trino_catalogs_config_map(&sample_spec());
        assert_eq!(cm["metadata"]["name"], "demo-catalogs");
        let f = cm["data"]["egret.properties"].as_str().unwrap();
        assert!(f.contains("iceberg.catalog.type=rest"));
        assert!(f.contains("iceberg.rest-catalog.oauth2.credential=${ENV:EGRET_C0}"));
    }

    #[test]
    fn env_secret_aggregates_catalog_env() {
        let sec = trino_env_secret(&sample_spec());
        assert_eq!(sec["kind"], "Secret");
        assert_eq!(sec["metadata"]["name"], "demo-env");
        assert_eq!(sec["stringData"]["EGRET_C0"], "id:secret");
    }

    #[test]
    fn deployment_coordinator_mounts_config_and_scales() {
        let d = trino_deployment(&sample_spec(), Role::Coordinator);
        assert_eq!(d["kind"], "Deployment");
        assert_eq!(d["metadata"]["name"], "demo-coordinator");
        assert_eq!(d["spec"]["replicas"], 1);
        assert_eq!(d["spec"]["template"]["spec"]["containers"][0]["image"], "trinodb/trino:470");
        // config checksum annotation present (rolls pods on change)
        assert!(d["spec"]["template"]["metadata"]["annotations"]["egret/config-checksum"].is_string());
        let w = trino_deployment(&sample_spec(), Role::Worker);
        assert_eq!(w["metadata"]["name"], "demo-worker");
        assert_eq!(w["spec"]["replicas"], 2);
    }

    #[test]
    fn service_targets_coordinator() {
        let s = trino_service(&sample_spec());
        assert_eq!(s["metadata"]["name"], "demo-trino");
        assert_eq!(s["spec"]["selector"]["app"], "demo-coordinator");
        assert_eq!(s["spec"]["ports"][0]["port"], 8080);
    }
}
```

- [ ] **Step 2: Run — expect failure**

Run: `cargo test -p egret-orchestrator trino::tests`
Expected: FAIL — functions undefined.

- [ ] **Step 3: Implement the templating** (replace the whole non-test body of `trino.rs`)

```rust
use crate::kube_client::KubeClient;
use crate::provisioner::EngineProvisioner;
use crate::spec::{EngineSpec, EngineStatus, PodStatus};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub enum Role { Coordinator, Worker }
impl Role {
    fn suffix(self) -> &'static str { match self { Role::Coordinator => "coordinator", Role::Worker => "worker" } }
    fn is_coord(self) -> bool { matches!(self, Role::Coordinator) }
}

fn jvm_config(spec: &EngineSpec) -> String {
    let heap = spec.config.jvm_heap.clone().unwrap_or_else(|| "1G".into());
    [
        "-server", &format!("-Xmx{heap}"), &format!("-Xms{heap}"), "-XX:+UseG1GC",
        "-XX:G1HeapRegionSize=32M", "-XX:+ExplicitGCInvokesConcurrent",
        "-XX:+HeapDumpOnOutOfMemoryError", "-XX:+ExitOnOutOfMemoryError",
        "-XX:-OmitStackTraceInFastThrow", "-Djdk.attach.allowAttachSelf=true",
    ].join("\n") + "\n"
}

fn config_properties(spec: &EngineSpec, coord: bool) -> String {
    let mut out = String::new();
    if coord {
        out.push_str("coordinator=true\n");
        let inc = spec.worker_replicas == 0;
        out.push_str(&format!("node-scheduler.include-coordinator={inc}\n"));
    } else {
        out.push_str("coordinator=false\n");
    }
    out.push_str("http-server.http.port=8080\n");
    out.push_str(&format!("discovery.uri=http://{}-trino:8080\n", spec.name));
    if let Some(v) = &spec.config.query_max_memory { out.push_str(&format!("query.max-memory={v}\n")); }
    if let Some(v) = &spec.config.query_max_memory_per_node { out.push_str(&format!("query.max-memory-per-node={v}\n")); }
    for (k, v) in &spec.config.config_properties { out.push_str(&format!("{k}={v}\n")); }
    out
}

pub fn trino_config_map(spec: &EngineSpec) -> Value {
    json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-config", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": {
            "config-coordinator.properties": config_properties(spec, true),
            "config-worker.properties": config_properties(spec, false),
            "jvm.config": jvm_config(spec),
            "node.properties": "node.environment=egret\nnode.data-dir=/data/trino\n",
        }
    })
}

pub fn trino_catalogs_config_map(spec: &EngineSpec) -> Value {
    let mut data = serde_json::Map::new();
    for c in &spec.catalogs {
        let body: String = c.properties.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        data.insert(format!("{}.properties", c.name), json!(body));
    }
    json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-catalogs", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": data
    })
}

pub fn trino_env_secret(spec: &EngineSpec) -> Value {
    let mut data = serde_json::Map::new();
    for (k, v) in &spec.config.env { data.insert(k.clone(), json!(v)); }
    for c in &spec.catalogs { for (k, v) in &c.env { data.insert(k.clone(), json!(v)); } }
    json!({
        "apiVersion": "v1", "kind": "Secret",
        "metadata": { "name": format!("{}-env", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": data
    })
}

fn config_checksum(spec: &EngineSpec) -> String {
    // Rolls the pods whenever rendered config or catalogs change.
    let mut h = DefaultHasher::new();
    trino_config_map(spec).to_string().hash(&mut h);
    trino_catalogs_config_map(spec).to_string().hash(&mut h);
    trino_env_secret(spec).to_string().hash(&mut h);
    format!("{:016x}", h.finish())
}

pub fn trino_deployment(spec: &EngineSpec, role: Role) -> Value {
    let app = format!("{}-{}", spec.name, role.suffix());
    let cfg_file = if role.is_coord() { "config-coordinator.properties" } else { "config-worker.properties" };
    let replicas = if role.is_coord() { spec.coordinator_replicas } else { spec.worker_replicas };
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
                "spec": {
                    "containers": [{
                        "name": "trino",
                        "image": format!("trinodb/trino:{}", spec.image_version),
                        "ports": [{ "containerPort": 8080 }],
                        "envFrom": [{ "secretRef": { "name": format!("{}-env", spec.name) } }],
                        "resources": {
                            "requests": { "cpu": spec.resources.cpu_min, "memory": spec.resources.memory },
                            "limits": { "cpu": spec.resources.cpu_max, "memory": spec.resources.memory }
                        },
                        "readinessProbe": { "httpGet": { "path": "/v1/info", "port": 8080 },
                            "initialDelaySeconds": 15, "periodSeconds": 10, "timeoutSeconds": 5, "failureThreshold": 12 },
                        "volumeMounts": [
                            { "name": "config", "mountPath": "/etc/trino/config.properties", "subPath": cfg_file },
                            { "name": "config", "mountPath": "/etc/trino/jvm.config", "subPath": "jvm.config" },
                            { "name": "config", "mountPath": "/etc/trino/node.properties", "subPath": "node.properties" },
                            { "name": "catalogs", "mountPath": "/etc/trino/catalog" },
                            { "name": "data", "mountPath": "/data/trino" }
                        ]
                    }],
                    "volumes": [
                        { "name": "config", "configMap": { "name": format!("{}-config", spec.name) } },
                        { "name": "catalogs", "configMap": { "name": format!("{}-catalogs", spec.name) } },
                        { "name": "data", "emptyDir": {} }
                    ]
                }
            }
        }
    })
}

pub fn trino_service(spec: &EngineSpec) -> Value {
    json!({
        "apiVersion": "v1", "kind": "Service",
        "metadata": { "name": format!("{}-trino", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "spec": { "selector": { "app": format!("{}-coordinator", spec.name) },
                  "ports": [{ "port": 8080, "targetPort": 8080, "name": "http" }] }
    })
}
```

- [ ] **Step 4: Run — expect pass**

Run: `cargo test -p egret-orchestrator trino::tests`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-orchestrator/src/trino.rs
git commit -m "feat(orchestrator): template Trino as plain Deployments + ConfigMaps + Service"
```

---

## Task 3: Rewrite `TrinoProvisioner` for manifests (TDD)

**Files:**
- Modify: `crates/egret-orchestrator/src/trino.rs` (the `impl EngineProvisioner`)

- [ ] **Step 1: Failing tests** (append to `mod tests`)

```rust
#[tokio::test]
async fn provision_applies_all_objects() {
    use crate::kube_client::FakeKube;
    let kube = std::sync::Arc::new(FakeKube::default());
    let p = TrinoProvisioner::new(kube.clone());
    p.provision(&sample_spec()).await.unwrap();
    let a = kube.applied.lock().unwrap();
    for k in ["ConfigMap/demo-config", "ConfigMap/demo-catalogs", "Secret/demo-env",
              "Deployment/demo-coordinator", "Deployment/demo-worker", "Service/demo-trino"] {
        assert!(a.contains_key(k), "missing {k}");
    }
}

#[tokio::test]
async fn delete_removes_all_objects() {
    use crate::kube_client::FakeKube;
    let kube = std::sync::Arc::new(FakeKube::default());
    let p = TrinoProvisioner::new(kube.clone());
    p.delete("egret-engines", "demo").await.unwrap();
    let d = kube.deleted.lock().unwrap();
    assert_eq!(d.len(), 6); // 2 deployments, service, 2 configmaps, secret
}
```

- [ ] **Step 2: Run — expect failure**

Run: `cargo test -p egret-orchestrator`
Expected: FAIL.

- [ ] **Step 3: Implement** (replace the `impl EngineProvisioner for TrinoProvisioner` block)

```rust
pub struct TrinoProvisioner { kube: Arc<dyn KubeClient> }
impl TrinoProvisioner { pub fn new(kube: Arc<dyn KubeClient>) -> Self { Self { kube } } }

impl TrinoProvisioner {
    async fn apply_all(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // ConfigMaps + Secret first so the Deployments mount current data; the
        // checksum annotation on the pod template rolls pods when they change.
        self.kube.apply(&trino_config_map(spec)).await?;
        self.kube.apply(&trino_catalogs_config_map(spec)).await?;
        self.kube.apply(&trino_env_secret(spec)).await?;
        self.kube.apply(&trino_service(spec)).await?;
        self.kube.apply(&trino_deployment(spec, Role::Coordinator)).await?;
        self.kube.apply(&trino_deployment(spec, Role::Worker)).await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl EngineProvisioner for TrinoProvisioner {
    async fn provision(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        if let Err(e) = self.apply_all(spec).await {
            let _ = self.delete(&spec.namespace, &spec.name).await;
            return Err(e);
        }
        Ok(())
    }
    async fn status(&self, namespace: &str, name: &str) -> anyhow::Result<EngineStatus> {
        let pods = self.kube.list_pods(namespace, name).await?;
        let pod_status: Vec<PodStatus> = pods.into_iter()
            .map(|(name, phase, ready)| PodStatus { name, phase, ready }).collect();
        let phase = if pod_status.is_empty() { "provisioning" }
            else if pod_status.iter().all(|p| p.ready) { "running" }
            else if pod_status.iter().any(|p| p.phase == "Failed") { "failed" }
            else { "provisioning" };
        Ok(EngineStatus { phase: phase.into(), message: None, pods: pod_status })
    }
    async fn update(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // Re-apply everything; the recomputed checksum annotation rolls the pods
        // so Trino reloads the (static) config + catalogs.
        self.apply_all(spec).await
    }
    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        let _ = self.kube.delete("apps/v1", "Deployment", namespace, &format!("{name}-coordinator")).await;
        let _ = self.kube.delete("apps/v1", "Deployment", namespace, &format!("{name}-worker")).await;
        let _ = self.kube.delete("v1", "Service", namespace, &format!("{name}-trino")).await;
        let _ = self.kube.delete("v1", "ConfigMap", namespace, &format!("{name}-config")).await;
        let _ = self.kube.delete("v1", "ConfigMap", namespace, &format!("{name}-catalogs")).await;
        let _ = self.kube.delete("v1", "Secret", namespace, &format!("{name}-env")).await;
        Ok(())
    }
}
```

- [ ] **Step 4: Run + clippy + commit**

```bash
cargo test -p egret-orchestrator
cargo clippy -p egret-orchestrator
cargo fmt --all
git add crates/egret-orchestrator/src/trino.rs
git commit -m "feat(orchestrator): TrinoProvisioner applies/rolls/deletes plain manifests"
```
Expected: all orchestrator tests pass, clippy clean.

---

## Task 4: `engine_catalogs` migration + store CRUD

**Files:**
- Create: `migrations/00NN_engine_catalogs.sql` (use the next free number — check `migrations/` for the highest; Phase 1 used 0056)
- Modify: `crates/egret-catalog/src/service/embedded.rs`, `service/mod.rs`, `lib.rs`
- Modify: `crates/egret-metastore/src/lib.rs` (touch to re-embed — known staleness gotcha)

- [ ] **Step 1: Migration**

```sql
CREATE TABLE engine_catalogs (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    engine_id   uuid NOT NULL REFERENCES engines(id) ON DELETE CASCADE,
    name        text NOT NULL,
    kind        text NOT NULL CHECK (kind IN ('egret-warehouse','postgresql','tpch','tpcds','generic')),
    config      jsonb NOT NULL DEFAULT '{}'::jsonb,
    secret_ref  uuid,
    is_primary  boolean NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (engine_id, name)
);
```

- [ ] **Step 2: `EngineCatalog` + CRUD in `embedded.rs`** (mirror the `Engine` CRUD from Phase 1)

```rust
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct EngineCatalog {
    pub id: Uuid,
    pub engine_id: Uuid,
    pub name: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub secret_ref: Option<Uuid>,
    pub is_primary: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl CatalogStore {
    pub async fn create_engine_catalog(&self, engine_id: Uuid, name: &str, kind: &str, config: &serde_json::Value, secret_ref: Option<Uuid>, is_primary: bool) -> Result<EngineCatalog, CatalogError> {
        sqlx::query_as::<_, EngineCatalog>(
            "INSERT INTO engine_catalogs (engine_id,name,kind,config,secret_ref,is_primary) VALUES ($1,$2,$3,$4,$5,$6) \
             RETURNING id,engine_id,name,kind,config,secret_ref,is_primary,created_at")
            .bind(engine_id).bind(name).bind(kind).bind(config).bind(secret_ref).bind(is_primary)
            .fetch_one(self.pool()).await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.constraint() == Some("engine_catalogs_engine_id_name_key") => CatalogError::AlreadyExists(name.into()),
                _ => CatalogError::Db(e),
            })
    }
    pub async fn list_engine_catalogs(&self, engine_id: Uuid) -> Result<Vec<EngineCatalog>, CatalogError> {
        Ok(sqlx::query_as::<_, EngineCatalog>("SELECT id,engine_id,name,kind,config,secret_ref,is_primary,created_at FROM engine_catalogs WHERE engine_id=$1 ORDER BY is_primary DESC, created_at").bind(engine_id).fetch_all(self.pool()).await?)
    }
    pub async fn get_engine_catalog(&self, id: Uuid) -> Result<Option<EngineCatalog>, CatalogError> {
        Ok(sqlx::query_as::<_, EngineCatalog>("SELECT id,engine_id,name,kind,config,secret_ref,is_primary,created_at FROM engine_catalogs WHERE id=$1").bind(id).fetch_optional(self.pool()).await?)
    }
    pub async fn delete_engine_catalog(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM engine_catalogs WHERE id=$1").bind(id).execute(self.pool()).await?;
        Ok(())
    }
}
```

Export `EngineCatalog` from `service/mod.rs` + `lib.rs` (next to `Engine`).

- [ ] **Step 3: Re-embed + test**

```bash
touch crates/egret-metastore/src/lib.rs
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-catalog
```
Add a test that creates an engine (reuse the Phase-1 warehouse-seed chain) + a primary catalog + an extra catalog, lists (primary first), deletes the extra. Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add migrations/ crates/egret-catalog/src crates/egret-catalog/tests crates/egret-metastore/src/lib.rs
git commit -m "feat(catalog): engine_catalogs table + CRUD (cascade on engine delete)"
```

---

## Task 5: Control-plane — catalog translation, config, catalog routes

**Files:**
- Modify: `crates/egret-controlplane/src/engines.rs`
- Test: `crates/egret-controlplane/tests/routes_api.rs`

- [ ] **Step 1: Extend `EngineProvisionReq` with catalogs + config**

Add kube-free carrier types + fields (the orchestrator equivalents, re-declared so
egret-controlplane stays kube-free):

```rust
#[derive(Clone)]
pub struct CatalogReq { pub name: String, pub properties: std::collections::BTreeMap<String,String>, pub env: std::collections::BTreeMap<String,String> }
#[derive(Clone, Default)]
pub struct TrinoConfigReq {
    pub jvm_heap: Option<String>,
    pub query_max_memory: Option<String>,
    pub query_max_memory_per_node: Option<String>,
    pub config_properties: std::collections::BTreeMap<String,String>,
    pub env: std::collections::BTreeMap<String,String>,
}
```

Replace the flat wiring fields on `EngineProvisionReq` (`catalog_uri`, `token_uri`,
`warehouse`, `s3_endpoint`, `client_id`, `client_secret`) with:

```rust
    pub catalogs: Vec<CatalogReq>,
    pub config: TrinoConfigReq,
```

Export `CatalogReq` + `TrinoConfigReq` from `lib.rs`.

- [ ] **Step 2: The kind→CatalogReq translation**

```rust
/// Translate one stored catalog row into a flat Trino catalog (properties + env).
/// `env_prefix` namespaces this catalog's secret env vars (e.g. "C0").
async fn build_catalog_req(st: &CpState, engine: &Engine, cat: &EngineCatalog, env_prefix: &str) -> Result<CatalogReq, CpError> {
    use std::collections::BTreeMap;
    let mut props = BTreeMap::new();
    let mut env = BTreeMap::new();
    match cat.kind.as_str() {
        "egret-warehouse" => {
            // Warehouse id + a minted client live in cat.config; the OAuth credential
            // is passed via env so it never lands in the ConfigMap.
            let wh_name = cat.config["warehouse"].as_str().unwrap_or_default().to_string();
            let cred_var = format!("EGRET_{env_prefix}");
            props.insert("connector.name".into(), "iceberg".into());
            props.insert("iceberg.catalog.type".into(), "rest".into());
            props.insert("iceberg.rest-catalog.uri".into(), format!("{}/catalog", st.engine_egret_url));
            props.insert("iceberg.rest-catalog.warehouse".into(), wh_name);
            props.insert("iceberg.rest-catalog.security".into(), "OAUTH2".into());
            props.insert("iceberg.rest-catalog.oauth2.server-uri".into(), format!("{}/oauth/token", st.engine_egret_url));
            props.insert("iceberg.rest-catalog.oauth2.credential".into(), format!("${{ENV:{cred_var}}}"));
            props.insert("iceberg.rest-catalog.vended-credentials-enabled".into(), "true".into());
            props.insert("fs.native-s3.enabled".into(), "true".into());
            props.insert("s3.endpoint".into(), cat.config["s3_endpoint"].as_str().unwrap_or_default().into());
            props.insert("s3.region".into(), "us-east-1".into());
            props.insert("s3.path-style-access".into(), "true".into());
            let cred = cat.config["credential"].as_str().unwrap_or_default().to_string();
            env.insert(cred_var, cred);
        }
        "postgresql" => {
            props.insert("connector.name".into(), "postgresql".into());
            props.insert("connection-url".into(), format!("jdbc:postgresql://{}:{}/{}",
                cat.config["host"].as_str().unwrap_or_default(), cat.config["port"].as_i64().unwrap_or(5432), cat.config["database"].as_str().unwrap_or_default()));
            props.insert("connection-user".into(), cat.config["user"].as_str().unwrap_or_default().into());
            let pw_var = format!("PG_{env_prefix}");
            props.insert("connection-password".into(), format!("${{ENV:{pw_var}}}"));
            if let Some(ref_id) = cat.secret_ref {
                let pw = st.secrets.resolve_plaintext(None, ref_id).await.map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?;
                env.insert(pw_var, pw);
            }
        }
        "tpch" => { props.insert("connector.name".into(), "tpch".into()); }
        "tpcds" => { props.insert("connector.name".into(), "tpcds".into()); }
        "generic" => {
            props.insert("connector.name".into(), cat.config["connectorName"].as_str().unwrap_or_default().into());
            if let Some(obj) = cat.config["properties"].as_object() {
                for (k, v) in obj { props.insert(k.clone(), v.as_str().unwrap_or_default().into()); }
            }
        }
        other => return Err(CpError::BadRequest(format!("unknown catalog kind: {other}"))),
    }
    Ok(CatalogReq { name: cat.name.clone(), properties: props, env })
}

/// Build the full provisioning request from the engine + its stored catalogs + config.
async fn build_provision_req(st: &CpState, engine: &Engine) -> Result<EngineProvisionReq, CpError> {
    let cats = st.storage.list_engine_catalogs(engine.id).await?;
    let mut catalogs = Vec::new();
    for (i, c) in cats.iter().enumerate() {
        catalogs.push(build_catalog_req(st, engine, c, &format!("C{i}")).await?);
    }
    let cfg = engine.spec.get("trinoConfig").cloned().unwrap_or_default();
    Ok(EngineProvisionReq {
        name: engine.name.clone(),
        namespace: engine.namespace.clone(),
        image_version: engine.spec["imageVersion"].as_str().unwrap_or(DEFAULT_IMAGE_VERSION).into(),
        coordinator_replicas: engine.spec["coordinatorReplicas"].as_u64().unwrap_or(1) as u32,
        worker_replicas: engine.spec["workerReplicas"].as_u64().unwrap_or(1) as u32,
        cpu_min: engine.spec["cpuMin"].as_str().unwrap_or("1").into(),
        cpu_max: engine.spec["cpuMax"].as_str().unwrap_or("2").into(),
        memory: engine.spec["memory"].as_str().unwrap_or("4Gi").into(),
        catalogs,
        config: trino_config_from_json(&cfg),
    })
}

fn trino_config_from_json(v: &serde_json::Value) -> TrinoConfigReq {
    use std::collections::BTreeMap;
    let map = |k: &str| -> BTreeMap<String,String> {
        v[k].as_object().map(|o| o.iter().filter_map(|(k,val)| val.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default()
    };
    TrinoConfigReq {
        jvm_heap: v["jvmHeap"].as_str().map(Into::into),
        query_max_memory: v["queryMaxMemory"].as_str().map(Into::into),
        query_max_memory_per_node: v["queryMaxMemoryPerNode"].as_str().map(Into::into),
        config_properties: map("configProperties"),
        env: map("env"),
    }
}
```

- [ ] **Step 3: Rework `create_engine` to store the primary catalog row + use `build_provision_req`**

`create_engine` still: gate, resolve warehouse (name + provisioned check) + storage endpoint, `ensure_engine_role` + `create_client`, create the `engines` row. THEN instead of building the flat wiring req, create the **primary catalog row**:

```rust
let cat_config = serde_json::json!({
    "warehouse": wh.name, "s3_endpoint": s3_endpoint,
    "credential": format!("{}:{}", client.client_id, client.client_secret),
    "clientRowId": client.id,
});
st.storage.create_engine_catalog(engine.id, "egret", "egret-warehouse", &cat_config, None, true).await?;
let req = build_provision_req(&st, &engine).await?;
// ... provisioner.provision(&req) with rollback on failure (delete catalogs + engine row) ...
```

`update_engine` becomes: gate, load engine, apply the new `TrinoConfig` fields into
`engine.spec` (persist), then `build_provision_req` + `provisioner.update(&req)`.

- [ ] **Step 4: Catalog sub-routes**

```rust
.route("/engines/{id}/catalogs", get(list_catalogs).post(add_catalog))
.route("/engines/{id}/catalogs/{cid}", delete(remove_catalog))
```

- `list_catalogs` → `list_engine_catalogs` → DTOs (id, name, kind, isPrimary).
- `add_catalog` (body: `{name, kind, config, secretRef?}`) → validate kind, `create_engine_catalog`, then `build_provision_req` + `provisioner.update` (re-render + roll). On provisioner error, delete the row + 400.
- `remove_catalog` → load; 400 if `is_primary`; `delete_engine_catalog`; if the row had a minted client (`config.clientRowId`), best-effort `revoke_client`; `build_provision_req` + `provisioner.update`.

- [ ] **Step 5: Route test**

Extend `routes_api.rs`: with the `FakeEngineProvisioner`, after creating an engine,
`POST /engines/{id}/catalogs` a `tpch` catalog → 201, `GET` lists 2 (primary + tpch),
`DELETE` the tpch → 200, deleting the primary → 400. Run `cargo test -p egret-controlplane`.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-controlplane/src/engines.rs crates/egret-controlplane/src/lib.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): multi-catalog + config surface; primary catalog row + catalog routes"
```

---

## Task 6: egret-bin adapter — map the richer req to the orchestrator spec

**Files:**
- Modify: `crates/egret-bin/src/main.rs`

- [ ] **Step 1** Update `impl egret_controlplane::EngineProvisioner for EgretEngineProvisioner`: build `egret_orchestrator::EngineSpec` from the req — map `req.catalogs` → `Vec<CatalogSpec>` (`CatalogSpec { name, properties, env }`), `req.config` → `egret_orchestrator::TrinoConfig` (field-for-field), drop the old `CatalogWiring`/`extra_props` mapping. Keep the status/pod mapping.
- [ ] **Step 2** `cargo build -p egret-bin` (allow time). Commit.

```bash
git add crates/egret-bin/src/main.rs Cargo.lock
git commit -m "feat(bin): map catalog/config request to the manifest EngineSpec"
```

---

## Task 7: Frontend — api client (config + catalogs)

**Files:**
- Modify: `frontend/src/api/controlPlane.ts`

- [ ] **Step 1** Add to `CreateEngineInput` optional config fields `jvmHeap?, queryMaxMemory?, queryMaxMemoryPerNode?, configProperties?: Record<string,string>, env?: Record<string,string>` (sent under a `trinoConfig` object matching the backend — verify the exact wire shape in `engines.rs`). Add:

```ts
export interface EngineCatalog { id: string; name: string; kind: string; isPrimary: boolean }
export interface AddCatalogInput { name: string; kind: string; config: Record<string, unknown>; secretRef?: string | null }
export function listEngineCatalogs(id: string): Promise<EngineCatalog[]> { return apiJson(`/control-plane/engines/${id}/catalogs`); }
export function addEngineCatalog(id: string, input: AddCatalogInput): Promise<EngineCatalog> { return apiJson(`/control-plane/engines/${id}/catalogs`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input) }); }
export async function deleteEngineCatalog(id: string, cid: string): Promise<void> { await apiFetch(`/control-plane/engines/${id}/catalogs/${cid}`, { method: "DELETE" }); }
```

- [ ] **Step 2** `cd frontend && npx tsc --noEmit`; commit.

---

## Task 8: Frontend — EngineDetail Config + Catalogs sections

**Files:**
- Modify: `frontend/src/routes/control-plane/EngineDetail.tsx`

- [ ] **Step 1** **Config section**: extend the existing config card with curated fields (jvmHeap, queryMaxMemory, queryMaxMemoryPerNode) + a raw key→value editor for `configProperties` (and env). Save → `updateEngine` (already wired) including the new `trinoConfig`. Show a "restarting to apply" note on success (Trino rolls to reload).
- [ ] **Step 2** **Catalogs section**: `useQuery(listEngineCatalogs)` → a table (name, kind, primary badge). An **Add catalog** dialog whose fields switch on `kind`:
  - `egret-warehouse` → warehouse picker (provisioned only) — sends `config: { warehouseId }` (backend resolves name/endpoint/credential like the primary).
  - `postgresql` → host, port, database, user + a Secret picker (`listSecrets`) for the password → `config: { host, port, database, user }`, `secretRef`.
  - `tpch` / `tpcds` → just a name.
  - `generic` → connectorName + a key→value properties editor → `config: { connectorName, properties }`.
  Submit → `addEngineCatalog` → invalidate. Remove per non-primary row → `deleteEngineCatalog`.
- [ ] **Step 3** `tsc` + `eslint` + `vite build`; commit.

Note for Step 1: `egret-warehouse` add-catalog needs the backend `add_catalog` to
resolve the warehouse (name + endpoint) and mint a client — same as the primary. If
Task 5's `add_catalog` only stores raw `config`, extend it so `kind == egret-warehouse`
runs the same resolve+mint the primary does before storing the row.

---

## Task 9: Dev/e2e harness — drop the Stackable operator for Trino

**Files:**
- Modify: `deploy/k8s/README.md`, `justfile`

- [ ] **Step 1** `justfile` `k8s-up`: keep `kind create cluster` + `kubectl create namespace egret-engines`, but REMOVE the Stackable Trino operator install (Trino no longer needs an operator). Keep a note that StarRocks/RisingWave phases will add their operators.
- [ ] **Step 2** `deploy/k8s/README.md`: update the manual e2e — no operator prerequisite for Trino; egret applies Deployments directly. RBAC now needs create/get/patch/delete on `deployments` (apps), `services`/`configmaps`/`secrets` (core), get/list `pods` — replace the `trinoclusters`/`trinocatalogs` rules. Update the walkthrough: provision → pods Ready → `SHOW SCHEMAS` → add a `tpch` catalog → roll → cross-catalog `SELECT`.
- [ ] **Step 3** Commit.

---

## Task 10: Full gate + PR

- [ ] **Step 1** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`.
- [ ] **Step 2** Frontend `npx tsc --noEmit`, `npm run lint`, `npx vitest run`, `npm run build`.
- [ ] **Step 3** Live-test on the running kind cluster: rebuild egret, delete the old Stackable `demo-trino` engine, provision a fresh one (plain Deployments now), verify pods Ready + `SHOW SCHEMAS`, add a `tpch` catalog, verify `SELECT count(*) FROM tpch.tiny.nation`.
- [ ] **Step 4** Commit; open PR; run the full local gate before an admin squash-merge (GitHub CI out of quota).

---

## Self-Review notes

- **Spec coverage:** manifest pivot (Task 1-3) ✓; config surface curated+raw (Task 1,2,5,8) ✓; multi-catalog table (Task 4) + translation + routes (Task 5) + kinds egret-warehouse/postgresql/tpch/tpcds/generic (Task 5) ✓; primary catalog as first row (Task 5) ✓; secrets from egret Secrets → env (Task 5) ✓; rolling restart via checksum annotation (Task 2,3) ✓; frontend (Task 7,8) ✓; drop operator/PVC (Task 3,9) ✓; egret-bin (Task 6) ✓.
- **Type consistency:** `TrinoConfig`/`CatalogSpec` (orchestrator) ↔ `TrinoConfigReq`/`CatalogReq` (controlplane) ↔ `trinoConfig`/catalog JSON (wire) — field names line up (jvm_heap↔jvmHeap etc.). `EngineSpec` no longer has `wiring`/`extra_props`; every reader (trino.rs, egret-bin) updated in Tasks 2/6.
- **Env namespacing:** each catalog's secret env vars are prefixed (`C0`,`C1`,…) in `build_catalog_req` so two catalogs never collide on an env key.
- **Deferred (own spec):** the egret→Trino reverse proxy (Phase 1.6).
