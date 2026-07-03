# Engine Provisioning — Phase 1 (Trino on Kubernetes) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a platform admin provision, configure, and delete a **Trino** query
engine from egret's UI (egret on Kubernetes, via the Stackable Trino operator),
auto-wired to egret's embedded Iceberg catalog.

**Architecture:** A new `egret-orchestrator` crate wraps a kube-rs client behind a
`KubeClient` trait (fakeable for tests) and an `EngineProvisioner` trait. Provisioning
Trino applies three Kubernetes resources — a `Secret` (Catalog Client creds), a
`TrinoCatalog` CR (generic `iceberg` connector → egret's REST catalog), and a
`TrinoCluster` CR (coordinator + workers). egret's `engines` table records intent;
live status is read from the CR + pods. Control-plane routes + a `ControlPlaneEngines`
UI drive it. Operators are a pre-installed prerequisite; egret only applies CRs.

**Tech Stack:** Rust (kube-rs `Api<DynamicObject>`, k8s-openapi), axum control-plane,
sqlx/Postgres, React/TS frontend, Stackable `trino.stackable.tech/v1alpha1` CRDs.

**Spec:** `docs/en/specs/2026-07-03-engine-provisioning-design.md`.

**Reference (real CRDs):** `/Users/1pro-l4r8/Documents/github/data-portal/trino-operator`
(GVK `trino.stackable.tech/v1alpha1`; `TrinoCluster`, `TrinoCatalog` with a
`connector.generic.{connectorName, properties}` shape).

---

## File Structure

- **Create** `crates/egret-orchestrator/Cargo.toml` — new crate manifest.
- **Create** `crates/egret-orchestrator/src/lib.rs` — public types + re-exports.
- **Create** `crates/egret-orchestrator/src/kube_client.rs` — `KubeClient` trait + `LiveKube` impl + `FakeKube` (test).
- **Create** `crates/egret-orchestrator/src/spec.rs` — `EngineSpec`, `EngineStatus`, `PodStatus`.
- **Create** `crates/egret-orchestrator/src/trino.rs` — CR templating (pure fns) + `TrinoProvisioner`.
- **Create** `crates/egret-orchestrator/src/provisioner.rs` — `EngineProvisioner` trait.
- **Create** `migrations/0056_engines.sql` — `engines` table.
- **Modify** `crates/egret-catalog/src/service/embedded.rs` — `Engine` struct + CRUD.
- **Modify** `crates/egret-catalog/src/service/mod.rs` + `lib.rs` — export `Engine`.
- **Create** `crates/egret-controlplane/src/engines.rs` — `EngineProvisioner` (control-plane trait) + DTOs + routes.
- **Modify** `crates/egret-controlplane/src/lib.rs` — `CpState.engine_provisioner` + router mount + exports.
- **Modify** `crates/egret-bin/src/main.rs` — build kube client + `LakekeeperTrinoProvisioner` + wire into `CpState`.
- **Modify** `frontend/src/api/controlPlane.ts` — engine types + CRUD fns.
- **Modify** `frontend/src/routes/control-plane/ControlPlaneEngines.tsx` — fill the stub (list + wizard).
- **Create** `frontend/src/routes/control-plane/EngineDetail.tsx` — detail + config editing + delete.
- **Modify** `frontend/src/App.tsx` — route `engines/:id`.
- **Create** `deploy/k8s/README.md` + `justfile` `k8s-up` — kind + Stackable operator for e2e.

Note: the control-plane `EngineProvisioner` trait (implemented in egret-bin over
`egret-orchestrator`) mirrors the existing `WarehouseProvisioner`
(`crates/egret-controlplane/src/warehouses.rs:35-38` + `crates/egret-bin/src/main.rs`
`impl WarehouseProvisioner for LakekeeperProvisioner`). Follow that pattern for the
`CpState` field, the egret-bin impl, and the route error mapping.

---

## Task 1: New `egret-orchestrator` crate skeleton

**Files:**
- Create: `crates/egret-orchestrator/Cargo.toml`
- Create: `crates/egret-orchestrator/src/lib.rs`
- Create: `crates/egret-orchestrator/src/spec.rs`
- Modify: `Cargo.toml` (workspace members + workspace deps)

- [ ] **Step 1: Add the crate to the workspace + deps**

In the root `Cargo.toml`, add `"crates/egret-orchestrator"` to `[workspace.members]`
(keep the list sorted), and under `[workspace.dependencies]` add:

```toml
kube = { version = "0.99", features = ["client", "runtime", "derive"] }
k8s-openapi = { version = "0.24", features = ["latest"] }
```

(Match the versions already resolved in the workspace if these differ — run
`cargo update -p kube` after and commit `Cargo.lock`.)

- [ ] **Step 2: Crate manifest**

`crates/egret-orchestrator/Cargo.toml`:

```toml
[package]
name = "egret-orchestrator"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
kube.workspace = true
k8s-openapi.workspace = true
serde.workspace = true
serde_json.workspace = true
async-trait.workspace = true
anyhow.workspace = true
tracing.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt"] }
```

- [ ] **Step 3: `spec.rs` — the engine spec + status DTOs**

`crates/egret-orchestrator/src/spec.rs`:

```rust
//! Engine-agnostic provisioning spec + status (no k8s types leak out).

/// CPU/memory request for a Stackable role group.
#[derive(Clone, Debug)]
pub struct Resources {
    pub cpu_min: String, // e.g. "500m"
    pub cpu_max: String, // e.g. "2"
    pub memory: String,  // e.g. "4Gi"
}

/// One extra Trino catalog property (raw value only; the wired egret catalog is
/// built separately with a Secret-backed credential).
#[derive(Clone, Debug)]
pub struct CatalogProp {
    pub key: String,
    pub value: String,
}

/// The resolved coordinates + secret needed to wire a Trino to egret's catalog.
#[derive(Clone, Debug)]
pub struct CatalogWiring {
    pub catalog_uri: String,   // "http://egret:8080/catalog"
    pub token_uri: String,     // "http://egret:8080/oauth/token"
    pub warehouse: String,     // warehouse NAME
    pub s3_endpoint: String,   // "http://minio:9000"
    pub client_id: String,
    pub client_secret: String,
}

/// Desired state of a Trino engine.
#[derive(Clone, Debug)]
pub struct EngineSpec {
    pub name: String,      // DNS-1123 k8s name
    pub namespace: String,
    pub image_version: String,   // e.g. "481"
    pub coordinator_replicas: u32,
    pub worker_replicas: u32,
    pub resources: Resources,
    pub wiring: CatalogWiring,
    pub extra_props: Vec<CatalogProp>,
}

#[derive(Clone, Debug, Default)]
pub struct PodStatus {
    pub name: String,
    pub ready: bool,
    pub phase: String,
}

#[derive(Clone, Debug, Default)]
pub struct EngineStatus {
    /// High-level phase: "provisioning" | "running" | "failed" | "unknown".
    pub phase: String,
    pub message: Option<String>,
    pub pods: Vec<PodStatus>,
}
```

- [ ] **Step 4: `lib.rs` re-exports**

`crates/egret-orchestrator/src/lib.rs`:

```rust
//! egret-orchestrator: provisions query engines onto Kubernetes via operators.
//! Phase 1: Trino (Stackable operator). Engines are described by [`EngineSpec`]
//! and driven through the [`EngineProvisioner`] trait; k8s access goes through the
//! [`KubeClient`] trait so tests can inject a fake.

pub mod kube_client;
pub mod provisioner;
pub mod spec;
pub mod trino;

pub use kube_client::{KubeClient, LiveKube};
pub use provisioner::EngineProvisioner;
pub use spec::{CatalogProp, CatalogWiring, EngineSpec, EngineStatus, PodStatus, Resources};
pub use trino::TrinoProvisioner;
```

- [ ] **Step 5: Stub the other modules so the crate compiles**

Create empty-ish `provisioner.rs`, `kube_client.rs`, `trino.rs` with just the items
referenced above declared as `todo!()`/empty traits (filled in later tasks). Then:

Run: `cargo build -p egret-orchestrator`
Expected: compiles (warnings about unused are fine).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-orchestrator Cargo.toml Cargo.lock
git commit -m "feat(orchestrator): scaffold egret-orchestrator crate + engine spec types"
```

---

## Task 2: CR templating (pure functions, unit-tested)

**Files:**
- Modify: `crates/egret-orchestrator/src/trino.rs`
- Test: inline `#[cfg(test)]` in `trino.rs`

The three resources are built as `serde_json::Value` (applied later as
`DynamicObject`). GVK: `trino.stackable.tech/v1alpha1`, kinds `TrinoCluster` +
`TrinoCatalog`. The catalog is linked to the cluster by a label
(`trino: <name>`) matched by `clusterConfig.catalogLabelSelector`.

- [ ] **Step 1: Failing test for the catalog CR**

Add to `crates/egret-orchestrator/src/trino.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::*;

    fn sample_spec() -> EngineSpec {
        EngineSpec {
            name: "demo".into(),
            namespace: "egret-engines".into(),
            image_version: "481".into(),
            coordinator_replicas: 1,
            worker_replicas: 2,
            resources: Resources { cpu_min: "500m".into(), cpu_max: "2".into(), memory: "4Gi".into() },
            wiring: CatalogWiring {
                catalog_uri: "http://egret:8080/catalog".into(),
                token_uri: "http://egret:8080/oauth/token".into(),
                warehouse: "demo-catalog".into(),
                s3_endpoint: "http://minio:9000".into(),
                client_id: "egret_abc".into(),
                client_secret: "sekret".into(),
            },
            extra_props: vec![],
        }
    }

    #[test]
    fn catalog_cr_uses_generic_iceberg_rest() {
        let s = sample_spec();
        let cr = trino_catalog_cr(&s);
        assert_eq!(cr["apiVersion"], "trino.stackable.tech/v1alpha1");
        assert_eq!(cr["kind"], "TrinoCatalog");
        assert_eq!(cr["metadata"]["labels"]["trino"], "demo");
        let props = &cr["spec"]["connector"]["generic"]["properties"];
        assert_eq!(cr["spec"]["connector"]["generic"]["connectorName"], "iceberg");
        assert_eq!(props["iceberg.catalog.type"]["value"], "rest");
        assert_eq!(props["iceberg.rest-catalog.uri"]["value"], "http://egret:8080/catalog");
        assert_eq!(props["iceberg.rest-catalog.warehouse"]["value"], "demo-catalog");
        // credential is injected from the Secret, not inline
        assert_eq!(
            props["iceberg.rest-catalog.oauth2.credential"]["valueFromSecret"]["name"],
            "demo-catalog-creds"
        );
    }

    #[test]
    fn secret_holds_client_credential() {
        let cr = trino_secret(&sample_spec());
        assert_eq!(cr["kind"], "Secret");
        assert_eq!(cr["metadata"]["name"], "demo-catalog-creds");
        assert_eq!(cr["stringData"]["credential"], "egret_abc:sekret");
    }

    #[test]
    fn cluster_cr_sets_replicas_and_label_selector() {
        let cr = trino_cluster_cr(&sample_spec());
        assert_eq!(cr["kind"], "TrinoCluster");
        assert_eq!(cr["spec"]["clusterConfig"]["catalogLabelSelector"]["matchLabels"]["trino"], "demo");
        assert_eq!(cr["spec"]["coordinators"]["roleGroups"]["default"]["replicas"], 1);
        assert_eq!(cr["spec"]["workers"]["roleGroups"]["default"]["replicas"], 2);
        assert_eq!(cr["spec"]["image"]["productVersion"], "481");
    }
}
```

- [ ] **Step 2: Run — expect failure (functions undefined)**

Run: `cargo test -p egret-orchestrator trino::tests`
Expected: FAIL — `cannot find function trino_catalog_cr`.

- [ ] **Step 3: Implement the templating functions**

In `crates/egret-orchestrator/src/trino.rs` (above the tests):

```rust
use crate::spec::EngineSpec;
use serde_json::{json, Value};

fn creds_secret_name(spec: &EngineSpec) -> String {
    format!("{}-creds", spec.name)
}

/// Kubernetes `Secret` holding the Catalog Client credential (`client_id:client_secret`).
pub fn trino_secret(spec: &EngineSpec) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": { "name": creds_secret_name(spec), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": { "credential": format!("{}:{}", spec.wiring.client_id, spec.wiring.client_secret) }
    })
}

/// `TrinoCatalog` — generic `iceberg` connector wired to egret's REST catalog.
pub fn trino_catalog_cr(spec: &EngineSpec) -> Value {
    let w = &spec.wiring;
    let mut props = serde_json::Map::new();
    let mut v = |k: &str, val: String| { props.insert(k.into(), json!({ "value": val })); };
    v("iceberg.catalog.type", "rest".into());
    v("iceberg.rest-catalog.uri", w.catalog_uri.clone());
    v("iceberg.rest-catalog.warehouse", w.warehouse.clone());
    v("iceberg.rest-catalog.security", "OAUTH2".into());
    v("iceberg.rest-catalog.oauth2.server-uri", w.token_uri.clone());
    v("iceberg.rest-catalog.vended-credentials-enabled", "true".into());
    v("fs.native-s3.enabled", "true".into());
    v("s3.endpoint", w.s3_endpoint.clone());
    v("s3.region", "us-east-1".into());
    v("s3.path-style-access", "true".into());
    // OAuth credential from the Secret (never inline in the CR).
    props.insert(
        "iceberg.rest-catalog.oauth2.credential".into(),
        json!({ "valueFromSecret": { "name": creds_secret_name(spec), "key": "credential" } }),
    );
    for p in &spec.extra_props {
        props.insert(p.key.clone(), json!({ "value": p.value }));
    }
    json!({
        "apiVersion": "trino.stackable.tech/v1alpha1",
        "kind": "TrinoCatalog",
        "metadata": { "name": format!("{}-egret", spec.name), "namespace": spec.namespace,
                      "labels": { "trino": spec.name, "app.kubernetes.io/managed-by": "egret" } },
        "spec": { "connector": { "generic": { "connectorName": "iceberg", "properties": props } } }
    })
}

/// `TrinoCluster` — coordinator + workers, resources, and the catalog label selector.
pub fn trino_cluster_cr(spec: &EngineSpec) -> Value {
    let res = json!({
        "cpu": { "min": spec.resources.cpu_min, "max": spec.resources.cpu_max },
        "memory": { "limit": spec.resources.memory }
    });
    json!({
        "apiVersion": "trino.stackable.tech/v1alpha1",
        "kind": "TrinoCluster",
        "metadata": { "name": spec.name, "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret" } },
        "spec": {
            "image": { "productVersion": spec.image_version },
            "clusterConfig": { "catalogLabelSelector": { "matchLabels": { "trino": spec.name } } },
            "coordinators": { "roleGroups": { "default": {
                "replicas": spec.coordinator_replicas,
                "config": { "resources": res.clone() } } } },
            "workers": { "roleGroups": { "default": {
                "replicas": spec.worker_replicas,
                "config": { "resources": res } } } }
        }
    })
}
```

- [ ] **Step 4: Run — expect pass**

Run: `cargo test -p egret-orchestrator trino::tests`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-orchestrator/src/trino.rs
git commit -m "feat(orchestrator): Trino CR templating (Secret + TrinoCatalog + TrinoCluster)"
```

---

## Task 3: `KubeClient` trait + fake, `EngineProvisioner` trait, `TrinoProvisioner`

**Files:**
- Modify: `crates/egret-orchestrator/src/kube_client.rs`, `provisioner.rs`, `trino.rs`
- Test: inline in `trino.rs`

- [ ] **Step 1: `provisioner.rs` — the trait**

```rust
use crate::spec::{EngineSpec, EngineStatus};

#[async_trait::async_trait]
pub trait EngineProvisioner: Send + Sync {
    async fn provision(&self, spec: &EngineSpec) -> anyhow::Result<()>;
    async fn status(&self, namespace: &str, name: &str) -> anyhow::Result<EngineStatus>;
    async fn update(&self, spec: &EngineSpec) -> anyhow::Result<()>;
    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()>;
}
```

- [ ] **Step 2: `kube_client.rs` — the seam + a fake**

```rust
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Minimal k8s surface egret needs. `apply` is server-side-apply of one resource
/// identified by (apiVersion, kind, namespace, name); `delete` removes it; the
/// pod/CR reads back status. Keeps kube-rs out of the templating + tests.
#[async_trait::async_trait]
pub trait KubeClient: Send + Sync {
    async fn apply(&self, obj: &Value) -> anyhow::Result<()>;
    async fn delete(&self, api_version: &str, kind: &str, namespace: &str, name: &str) -> anyhow::Result<()>;
    /// Pods labeled `app.kubernetes.io/instance=<name>` in the namespace.
    async fn list_pods(&self, namespace: &str, instance: &str) -> anyhow::Result<Vec<(String, String, bool)>>; // (name, phase, ready)
}

/// In-memory fake for unit tests: records applied/deleted objects.
#[derive(Default)]
pub struct FakeKube {
    pub applied: Mutex<BTreeMap<String, Value>>, // key = "kind/name"
    pub deleted: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl KubeClient for FakeKube {
    async fn apply(&self, obj: &Value) -> anyhow::Result<()> {
        let key = format!("{}/{}", obj["kind"].as_str().unwrap_or(""), obj["metadata"]["name"].as_str().unwrap_or(""));
        self.applied.lock().unwrap().insert(key, obj.clone());
        Ok(())
    }
    async fn delete(&self, _av: &str, kind: &str, _ns: &str, name: &str) -> anyhow::Result<()> {
        self.deleted.lock().unwrap().push(format!("{kind}/{name}"));
        Ok(())
    }
    async fn list_pods(&self, _ns: &str, _instance: &str) -> anyhow::Result<Vec<(String, String, bool)>> {
        Ok(vec![("demo-coordinator-0".into(), "Running".into(), true)])
    }
}
```

- [ ] **Step 3: Failing test for `TrinoProvisioner::provision` (applies 3 resources, rolls back)**

Add to `trino.rs` tests:

```rust
#[tokio::test]
async fn provision_applies_secret_catalog_cluster() {
    use crate::kube_client::FakeKube;
    use std::sync::Arc;
    let kube = Arc::new(FakeKube::default());
    let p = TrinoProvisioner::new(kube.clone());
    p.provision(&sample_spec()).await.unwrap();
    let applied = kube.applied.lock().unwrap();
    assert!(applied.contains_key("Secret/demo-creds"));
    assert!(applied.contains_key("TrinoCatalog/demo-egret"));
    assert!(applied.contains_key("TrinoCluster/demo"));
}

#[tokio::test]
async fn status_maps_pods_to_phase() {
    use crate::kube_client::FakeKube;
    use std::sync::Arc;
    let p = TrinoProvisioner::new(Arc::new(FakeKube::default()));
    let st = p.status("egret-engines", "demo").await.unwrap();
    assert_eq!(st.phase, "running"); // fake returns a ready pod
    assert_eq!(st.pods.len(), 1);
}
```

- [ ] **Step 4: Run — expect failure (no `TrinoProvisioner`)**

Run: `cargo test -p egret-orchestrator`
Expected: FAIL — `TrinoProvisioner` not found.

- [ ] **Step 5: Implement `TrinoProvisioner`**

Add to `trino.rs`:

```rust
use crate::kube_client::KubeClient;
use crate::provisioner::EngineProvisioner;
use crate::spec::{EngineStatus, PodStatus};
use std::sync::Arc;

pub struct TrinoProvisioner {
    kube: Arc<dyn KubeClient>,
}

impl TrinoProvisioner {
    pub fn new(kube: Arc<dyn KubeClient>) -> Self { Self { kube } }
}

#[async_trait::async_trait]
impl EngineProvisioner for TrinoProvisioner {
    async fn provision(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // Order: Secret -> Catalog -> Cluster. On any failure, best-effort delete
        // what we applied so a partial cluster never lingers.
        let secret = trino_secret(spec);
        let catalog = trino_catalog_cr(spec);
        let cluster = trino_cluster_cr(spec);
        if let Err(e) = async {
            self.kube.apply(&secret).await?;
            self.kube.apply(&catalog).await?;
            self.kube.apply(&cluster).await?;
            anyhow::Ok(())
        }.await {
            let _ = self.delete(&spec.namespace, &spec.name).await;
            return Err(e);
        }
        Ok(())
    }

    async fn status(&self, namespace: &str, name: &str) -> anyhow::Result<EngineStatus> {
        let pods = self.kube.list_pods(namespace, name).await?;
        let pod_status: Vec<PodStatus> = pods.into_iter()
            .map(|(name, phase, ready)| PodStatus { name, phase, ready })
            .collect();
        let phase = if pod_status.is_empty() { "provisioning" }
            else if pod_status.iter().all(|p| p.ready) { "running" }
            else if pod_status.iter().any(|p| p.phase == "Failed") { "failed" }
            else { "provisioning" };
        Ok(EngineStatus { phase: phase.into(), message: None, pods: pod_status })
    }

    async fn update(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // Server-side apply is idempotent: re-applying is the update path.
        self.kube.apply(&trino_secret(spec)).await?;
        self.kube.apply(&trino_catalog_cr(spec)).await?;
        self.kube.apply(&trino_cluster_cr(spec)).await?;
        Ok(())
    }

    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        let v = "trino.stackable.tech/v1alpha1";
        let _ = self.kube.delete(v, "TrinoCluster", namespace, name).await;
        let _ = self.kube.delete(v, "TrinoCatalog", namespace, &format!("{name}-egret")).await;
        let _ = self.kube.delete("v1", "Secret", namespace, &format!("{name}-creds")).await;
        Ok(())
    }
}
```

- [ ] **Step 6: Run — expect pass**

Run: `cargo test -p egret-orchestrator`
Expected: PASS (6 tests).

- [ ] **Step 7: Implement `LiveKube` (real kube-rs) — no unit test (needs a cluster)**

In `kube_client.rs`, add a `LiveKube { client: kube::Client }` implementing
`KubeClient` via `kube::Api::<kube::core::DynamicObject>::namespaced_with(...)`
using an `ApiResource` derived from `apiVersion`+`kind`, `PatchParams::apply("egret")`,
and `Api::<k8s_openapi::api::core::v1::Pod>` filtered by
`label_selector = "app.kubernetes.io/instance={instance}"` for `list_pods`. A missing
CRD surfaces as an error containing `the server could not find the requested resource`
→ map to a friendly message at the call site. Provide
`LiveKube::in_cluster().await` (prod) and `LiveKube::from_kubeconfig().await` (dev).

Run: `cargo build -p egret-orchestrator`
Expected: compiles.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-orchestrator/src
git commit -m "feat(orchestrator): KubeClient seam + TrinoProvisioner (provision/status/update/delete)"
```

---

## Task 4: `engines` migration + store CRUD

**Files:**
- Create: `migrations/0056_engines.sql`
- Modify: `crates/egret-catalog/src/service/embedded.rs`, `service/mod.rs`, `lib.rs`
- Modify: `crates/egret-metastore/src/lib.rs` (touch to re-embed migrations — see memory `egret-sqlx-migration-staleness`)
- Test: `crates/egret-catalog/tests/embedded.rs` (extend)

- [ ] **Step 1: Migration**

`migrations/0056_engines.sql`:

```sql
CREATE TABLE engines (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name         text NOT NULL UNIQUE,
    type         text NOT NULL CHECK (type IN ('trino','starrocks','risingwave')),
    namespace    text NOT NULL,
    warehouse_id uuid NOT NULL REFERENCES warehouses(id),
    spec         jsonb NOT NULL,
    status       text NOT NULL DEFAULT 'provisioning',
    created_at   timestamptz NOT NULL DEFAULT now()
);
```

- [ ] **Step 2: `Engine` struct + CRUD in `embedded.rs`**

Add (mirroring the `Warehouse`/`create_warehouse` pattern already in the file):

```rust
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Engine {
    pub id: Uuid,
    pub name: String,
    pub r#type: String,
    pub namespace: String,
    pub warehouse_id: Uuid,
    pub spec: serde_json::Value,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl CatalogStore {
    pub async fn create_engine(&self, name: &str, r#type: &str, namespace: &str, warehouse_id: Uuid, spec: &serde_json::Value) -> Result<Engine, CatalogError> {
        sqlx::query_as::<_, Engine>(
            "INSERT INTO engines (name, type, namespace, warehouse_id, spec) VALUES ($1,$2,$3,$4,$5) \
             RETURNING id, name, type, namespace, warehouse_id, spec, status, created_at")
            .bind(name).bind(r#type).bind(namespace).bind(warehouse_id).bind(spec)
            .fetch_one(self.pool()).await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.constraint() == Some("engines_name_key") => CatalogError::AlreadyExists(name.into()),
                _ => CatalogError::Db(e),
            })
    }
    pub async fn list_engines(&self) -> Result<Vec<Engine>, CatalogError> {
        Ok(sqlx::query_as::<_, Engine>("SELECT id, name, type, namespace, warehouse_id, spec, status, created_at FROM engines ORDER BY created_at").fetch_all(self.pool()).await?)
    }
    pub async fn get_engine(&self, id: Uuid) -> Result<Option<Engine>, CatalogError> {
        Ok(sqlx::query_as::<_, Engine>("SELECT id, name, type, namespace, warehouse_id, spec, status, created_at FROM engines WHERE id=$1").bind(id).fetch_optional(self.pool()).await?)
    }
    pub async fn update_engine(&self, id: Uuid, spec: &serde_json::Value, status: &str) -> Result<(), CatalogError> {
        sqlx::query("UPDATE engines SET spec=$2, status=$3 WHERE id=$1").bind(id).bind(spec).bind(status).execute(self.pool()).await?;
        Ok(())
    }
    pub async fn set_engine_status(&self, id: Uuid, status: &str) -> Result<(), CatalogError> {
        sqlx::query("UPDATE engines SET status=$2 WHERE id=$1").bind(id).bind(status).execute(self.pool()).await?;
        Ok(())
    }
    pub async fn delete_engine(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("DELETE FROM engines WHERE id=$1").bind(id).execute(self.pool()).await?;
        Ok(())
    }
}
```

Export `Engine` from `service/mod.rs` and `lib.rs` (add to the existing
`pub use ... {CatalogClient, ... , Engine, ...}` lists).

- [ ] **Step 3: Force migration re-embed + test**

```bash
touch crates/egret-metastore/src/lib.rs
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-catalog
```
Expected: PASS (add one test that creates + lists + deletes an engine referencing a seeded warehouse, following the existing `embedded.rs` test's warehouse setup).

- [ ] **Step 4: Commit**

```bash
git add migrations/0056_engines.sql crates/egret-catalog/src crates/egret-catalog/tests crates/egret-metastore/src/lib.rs
git commit -m "feat(catalog): engines table + CatalogStore CRUD"
```

---

## Task 5: Control-plane `EngineProvisioner` trait + routes

**Files:**
- Create: `crates/egret-controlplane/src/engines.rs`
- Modify: `crates/egret-controlplane/src/lib.rs`
- Test: `crates/egret-controlplane/tests/routes_api.rs` (extend with a `FakeEngineProvisioner`)

The control-plane trait keeps kube types out of egret-controlplane (implemented in
egret-bin), exactly like `WarehouseProvisioner`. It takes the *resolved* wiring the
route computed (warehouse name + a freshly-created Catalog Client).

- [ ] **Step 1: `engines.rs` — trait + DTOs + router**

```rust
use axum::extract::{Path, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::auth::require_platform_admin;
use crate::error::CpError;
use crate::CpState;

/// Resolved provisioning request handed to egret-bin (no k8s types here).
pub struct EngineProvisionReq {
    pub name: String,
    pub namespace: String,
    pub image_version: String,
    pub coordinator_replicas: u32,
    pub worker_replicas: u32,
    pub cpu_min: String,
    pub cpu_max: String,
    pub memory: String,
    pub catalog_uri: String,
    pub token_uri: String,
    pub warehouse: String,
    pub s3_endpoint: String,
    pub client_id: String,
    pub client_secret: String,
}
pub struct EnginePod { pub name: String, pub phase: String, pub ready: bool }
pub struct EngineLiveStatus { pub phase: String, pub message: Option<String>, pub pods: Vec<EnginePod> }

#[async_trait::async_trait]
pub trait EngineProvisioner: Send + Sync {
    async fn provision(&self, req: &EngineProvisionReq) -> Result<(), String>;
    async fn status(&self, namespace: &str, name: &str) -> Result<EngineLiveStatus, String>;
    async fn update(&self, req: &EngineProvisionReq) -> Result<(), String>;
    async fn delete(&self, namespace: &str, name: &str) -> Result<(), String>;
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/engines", get(list_engines).post(create_engine))
        .route("/engines/{id}", get(get_engine).put(update_engine).delete(delete_engine))
}
```

The `create_engine` handler:
1. `require_platform_admin`.
2. Look up the warehouse (`st.storage.get_warehouse(warehouse_id)`) → its name +
   `lakekeeper_warehouse_id` (must be provisioned, else 400).
3. Resolve the storage connection endpoint for `s3_endpoint`
   (`st.storage.get_storage_conn(warehouse.storage_conn_id)?.endpoint`).
4. **Auto-wire:** create a Catalog Client for that warehouse via
   `st.storage.create_client(...)` (same call `warehouses.rs` `create_client`
   uses) → `client_id` / `client_secret`.
5. Build `EngineProvisionReq` (catalog_uri = `{EGRET_PUBLIC_URL}/catalog`,
   token_uri = `{EGRET_PUBLIC_URL}/oauth/token`, from a config value `catalog.public_url`).
6. `st.storage.create_engine(...)` (status `provisioning`).
7. `st.engine_provisioner.provision(&req)` → on error, `delete_engine` row + return 400.
8. Return the engine DTO (camelCase).

`get_engine` merges the row with `provisioner.status(namespace, name)` for live
phase + pods. `update_engine` re-resolves + calls `provisioner.update`. `delete_engine`
calls `provisioner.delete` then `delete_engine` row (+ best-effort revoke the client).

Body DTOs (`CreateEngineBody { name, warehouseId, imageVersion?, coordinatorReplicas, workerReplicas, cpuMin, cpuMax, memory, extraProps? }`, `EngineDto`, `EngineStatusDto`) are camelCase; mirror the `warehouses.rs` DTO style.

- [ ] **Step 2: Wire into `CpState` + router**

In `crates/egret-controlplane/src/lib.rs`: add `pub engine_provisioner: Option<Arc<dyn engines::EngineProvisioner>>` to `CpState`; `.merge(engines::router())` in `control_plane_router()`; export the trait + `EngineProvisionReq`/`EngineLiveStatus`/`EnginePod`. Update every `CpState { .. }` construction (tests + egret-bin) to set `engine_provisioner`.

- [ ] **Step 3: Route test with a fake provisioner**

Extend `crates/egret-controlplane/tests/routes_api.rs`: a `FakeEngineProvisioner`
returning `Ok(())` / a `running` status; assert `POST /engines` on a seeded
provisioned warehouse returns 201 and `GET /engines/{id}` returns `phase: "running"`.
(Follow the existing `FakeBrowser` + `seed_provisioned_warehouse` helpers.)

- [ ] **Step 4: Run + commit**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-controlplane
git add crates/egret-controlplane/src/engines.rs crates/egret-controlplane/src/lib.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "feat(controlplane): engines routes + EngineProvisioner trait + auto-wire"
```

---

## Task 6: egret-bin wiring (kube client + provisioner impl)

**Files:**
- Modify: `crates/egret-bin/src/main.rs`
- Modify: `crates/egret-bin/Cargo.toml` (add `egret-orchestrator.workspace = true`)
- Modify: `crates/egret-config/src/...` (add `catalog.public_url` + `engines.namespace` config, default `http://localhost:8080` / `egret-engines`)

- [ ] **Step 1: `EgretEngineProvisioner` impl**

In `main.rs`, add a struct holding `Arc<egret_orchestrator::TrinoProvisioner>` and
implement `egret_controlplane::engines::EngineProvisioner` by mapping
`EngineProvisionReq` → `egret_orchestrator::EngineSpec` (build `CatalogWiring` +
`Resources` from the req) and delegating; map `EngineStatus`/`PodStatus` back to
`EngineLiveStatus`/`EnginePod`; `.map_err(|e| e.to_string())`. Mirror
`impl WarehouseProvisioner for LakekeeperProvisioner`.

- [ ] **Step 2: Construct it in `serve`**

Build the kube client best-effort: `LiveKube::in_cluster().await` else
`LiveKube::from_kubeconfig().await`; if both fail, log a warning and set
`engine_provisioner = None` (the routes then behave like "not configured" → 400
`"kubernetes not configured"`), so egret still runs locally without a cluster. Wire
the `Some(Arc::new(EgretEngineProvisioner::new(...)))` into `CpState`.

- [ ] **Step 3: Build + commit**

```bash
cargo build -p egret-bin
git add crates/egret-bin/src/main.rs crates/egret-bin/Cargo.toml crates/egret-config Cargo.lock
git commit -m "feat(bin): wire TrinoProvisioner (kube-rs) into the control plane"
```

---

## Task 7: Frontend — api client

**Files:**
- Modify: `frontend/src/api/controlPlane.ts`

- [ ] **Step 1: Types + CRUD fns** (mirror the warehouse api style)

```ts
export interface Engine {
  id: string; name: string; type: "trino" | "starrocks" | "risingwave";
  namespace: string; warehouseId: string; status: string;
  spec: { coordinatorReplicas: number; workerReplicas: number; cpuMin: string; cpuMax: string; memory: string };
}
export interface EngineStatus { phase: string; message: string | null; pods: { name: string; phase: string; ready: boolean }[] }
export interface CreateEngineInput {
  name: string; warehouseId: string; imageVersion?: string;
  coordinatorReplicas: number; workerReplicas: number; cpuMin: string; cpuMax: string; memory: string;
}
export function listEngines(): Promise<Engine[]> { return apiJson("/control-plane/engines"); }
export function getEngine(id: string): Promise<Engine & { status: EngineStatus }> { return apiJson(`/control-plane/engines/${id}`); }
export function createEngine(input: CreateEngineInput): Promise<Engine> { return apiJson("/control-plane/engines", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input) }); }
export function updateEngine(id: string, input: CreateEngineInput): Promise<Engine> { return apiJson(`/control-plane/engines/${id}`, { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(input) }); }
export async function deleteEngine(id: string): Promise<void> { await apiFetch(`/control-plane/engines/${id}`, { method: "DELETE" }); }
```

- [ ] **Step 2: `tsc` + commit**

```bash
cd frontend && npx tsc --noEmit -p tsconfig.json
git add frontend/src/api/controlPlane.ts && git commit -m "feat(ui): engines api client"
```

---

## Task 8: Frontend — fill `ControlPlaneEngines` list + Add-engine wizard

**Files:**
- Modify: `frontend/src/routes/control-plane/ControlPlaneEngines.tsx`

- [ ] **Step 1** Replace `ENGINE_TYPES` with `[{value:"trino",label:"Trino"},{value:"starrocks",label:"StarRocks"},{value:"risingwave",label:"RisingWave"}]`.
- [ ] **Step 2** Replace the stub `EngineTable` with a `useQuery(listEngines)` table filtered by the active type tab; columns Name / Type / Endpoint / **Status** (a colored badge from a per-row `useQuery(getEngine)` poll, `refetchInterval: 5000`) / Actions (link to detail). Empty state: "No {type} engines yet."
- [ ] **Step 3** Wire the "Add engine" button (only enabled for the Trino tab in Phase 1) to an `AddEngineDialog`: fields name, warehouse (`useMyWorkspaces`/`listWarehouses` select — provisioned only), coordinator replicas (default 1), worker replicas (default 2), cpuMin/cpuMax/memory (defaults "1"/"2"/"4Gi") → `useMutation(createEngine)` → invalidate the list. StarRocks/RisingWave tabs keep the "later phase" empty state + a disabled Add button.
- [ ] **Step 4** `tsc` + `eslint` + a Vitest that mocks `listEngines` → renders a row; wrap in `MemoryRouter` (the row links). Run `npx vitest run` for the file. Commit.

---

## Task 9: Frontend — `EngineDetail` page (status + config editing + delete)

**Files:**
- Create: `frontend/src/routes/control-plane/EngineDetail.tsx`
- Modify: `frontend/src/App.tsx` (add `<Route path="engines/:id" element={<EngineDetail />} />` + import)

- [ ] **Step 1** Page: back link to `/control-plane/engines`; header (name + type + status badge). A **status card**: phase + a pod table (name / phase / ready) from `getEngine` polled every 5s. A **config card**: editable coordinator/worker replicas + cpu/memory → `useMutation(updateEngine)`. A **danger card**: delete via `ConfirmDeleteDialog` → `useMutation(deleteEngine)` → navigate back.
- [ ] **Step 2** Route + import in `App.tsx`. `tsc`/`eslint`/`build`. Commit.

---

## Task 10: Dev/e2e harness (kind + Stackable operator) — docs + just target

**Files:**
- Create: `deploy/k8s/README.md`, `deploy/k8s/kind.yaml`
- Modify: `justfile`

- [ ] **Step 1** `justfile` target `k8s-up`: create a `kind` cluster, `helm repo add stackable-stable https://repo.stackable.tech/repository/helm-stable/`, install `commons-operator`, `secret-operator`, `listener-operator`, `trino-operator`, create namespace `egret-engines`.
- [ ] **Step 2** `deploy/k8s/README.md`: prerequisites (kind, helm, kubectl), the RBAC egret's ServiceAccount needs (create/get/patch/delete `trinoclusters`/`trinocatalogs`/`secrets`, get/list `pods` in `egret-engines`), and the manual e2e: `just k8s-up` → run egret against the kind kubeconfig → provision a Trino from the UI wired to `demo-catalog` → wait for pods Ready → `SELECT * FROM egret.demo_ns.events` in the Trino CLI. This is opt-in and NOT part of CI (like the existing manual Trino integration test).
- [ ] **Step 3** Commit.

---

## Task 11: Full gate + wrap-up

- [ ] **Step 1** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- [ ] **Step 2** Frontend `npx tsc --noEmit`, `npm run lint`, `npx vitest run`, `npm run build`.
- [ ] **Step 3** Final commit; open PR; per repo convention run the full local gate before an admin squash-merge (GitHub CI is out of quota — see memory `egret-run-full-ci-local-before-merge`).

---

## Self-Review notes

- **Spec coverage:** crate + trait (Task 1,3) ✓; Trino CRs + generic-iceberg-REST auto-wire (Task 2,5) ✓; data model (Task 4) ✓; routes (Task 5) ✓; bin wiring (Task 6) ✓; frontend list/wizard/detail with config-editing (Task 8,9) ✓; status/health (Task 3,5,8,9) ✓; dev/e2e + prereqs (Task 10) ✓; error handling / not-configured / rollback (Task 3,5,6) ✓.
- **Type consistency:** `EngineSpec`/`CatalogWiring`/`Resources` (orchestrator) ↔ `EngineProvisionReq` (controlplane) ↔ `CreateEngineInput` (frontend) — the replica/resource/wiring field names line up; the control-plane trait deliberately re-declares its own types so egret-controlplane never depends on kube.
- **Deferred to later specs (not gaps):** StarRocks + RisingWave provisioners; per-workspace binding; operator install from egret; autoscaling/upgrades.
