# Engine provisioning (Phase 1: Trino on Kubernetes) — design (2026-07-03)

VI reference: `docs/vi/specs/2026-07-03-engine-provisioning-design.md`.

**Goal:** Let a platform admin **provision, configure, and manage query engines
directly from egret's UI**, with egret running on Kubernetes. Phase 1 delivers a
full vertical slice for **Trino** (via the Stackable Trino operator): provision a
cluster, **auto-wire it to egret's embedded Iceberg catalog**, show live status,
edit its config (scale / resources / catalogs), and delete it.

Later phases add **StarRocks** and **RisingWave** behind the same
`EngineProvisioner` trait (each its own spec).

**Non-goals (Phase 1):** installing the operators (they are a pre-installed
platform prerequisite); per-workspace engine isolation (engines are shared
platform-wide, like warehouses/storage); multi-cloud; autoscaling; upgrades.

## Deployment model (decided)

egret creates **Custom Resources**; the engine **operators are pre-installed** in
the cluster (a platform prerequisite, like the DB/MinIO). Trino uses the
**Stackable** operator (`stackabletech/trino-operator`): a `TrinoCluster` +
`TrinoCatalog` CR. StarRocks/RisingWave (later) use their own operators. egret
never installs operators or CRDs in Phase 1; it only applies CRs, so its
Kubernetes RBAC stays narrow.

## Architecture

### New crate `egret-orchestrator`
Holds one `kube::Client` (in-cluster `ServiceAccount` in prod; a kubeconfig in
dev/kind) and the provisioner abstraction:

```rust
pub struct EngineSpec { /* name, warehouse, replicas, resources, catalogs, … */ }
pub struct EngineStatus { pub phase: String, pub message: Option<String>, pub endpoint: Option<String>, pub pods: Vec<PodStatus> }

#[async_trait]
pub trait EngineProvisioner: Send + Sync {
    async fn provision(&self, spec: &EngineSpec) -> Result<(), String>;
    async fn status(&self, name: &str) -> Result<EngineStatus, String>;
    async fn update(&self, spec: &EngineSpec) -> Result<(), String>;
    async fn delete(&self, name: &str) -> Result<(), String>;
}
```

Phase 1 implements `TrinoProvisioner` over kube-rs `Api<DynamicObject>` with the
Stackable GVKs. A `KubeClient` trait wraps the raw kube calls so tests can inject
a fake (no real cluster needed for unit tests).

### Provisioning Trino = apply 3 resources (idempotent, server-side apply)
1. **`Secret`** — the credentials of an auto-created **Catalog Client** (see
   auto-wire) — `client_id` / `client_secret`.
2. **`TrinoCatalog` CR** — an Iceberg REST catalog pointing at **egret's embedded
   catalog**: `uri` = `{egret}/catalog`, `warehouse` = the warehouse name, OAuth2
   `credential` / `server-uri` from the Secret. Reuses exactly the config the
   Connect tab already generates.
3. **`TrinoCluster` CR** — coordinator + N workers; resources/replicas from the
   spec; references the `TrinoCatalog`.

### Auto-wire (the key feature)
On provision, egret creates a **Catalog Client** for the chosen warehouse
(reusing the existing `create_client` machinery), takes its `client_id`/`secret`,
and injects them into the `TrinoCatalog`. The provisioned Trino therefore
**connects to egret's catalog on startup with no manual config** — the
Connect-tab values, delivered automatically. If auto-wire fails, the CR apply is
rolled back (delete any partially-applied resources).

## Data model (new migration)

`engines` table (control-plane, platform-global):

| column | type | notes |
|---|---|---|
| id | uuid pk | |
| name | text unique | k8s resource name (DNS-1123) |
| type | text | `'trino'` (CHECK in `trino`,`starrocks`,`risingwave`) |
| namespace | text | target k8s namespace |
| warehouse_id | uuid | the wired warehouse (FK `warehouses`) |
| spec | jsonb | replicas, resources (cpu/mem), extra catalogs |
| status | text | `provisioning`\|`running`\|`failed`\|`deleting` (last-known; live status read from the CR) |
| created_at | timestamptz | |

egret's row is the source of truth for *intent*; live health is read from the CR
+ pods at request time.

## Backend

- `egret-controlplane`: a new `engines` router (platform-admin gated):
  - `GET /control-plane/engines` — list (+ merge live status).
  - `POST /control-plane/engines` — provision (create row → auto-wire → apply CRs).
  - `GET /control-plane/engines/{id}` — detail + live status/pods.
  - `PUT /control-plane/engines/{id}` — update spec → patch the `TrinoCluster` CR.
  - `DELETE /control-plane/engines/{id}` — delete CRs + Secret + Catalog Client + row.
  - `EngineProvisioner` lives in `CpState` (like `WarehouseProvisioner`), impl'd in
    egret-bin over `egret-orchestrator`.
- Engine type/status DTOs are camelCase on the wire.

## Frontend (fill the `ControlPlaneEngines.tsx` stub)

- Engine types → **Trino / StarRocks / RisingWave** (replace the Spark-Connect stub tab).
- **List**: name, type, endpoint, **live status/health**, actions. Live status via
  a polling query.
- **Add-engine wizard**: name · warehouse to wire · size (coordinator + workers) ·
  resources (cpu/mem) · optional extra catalogs.
- **Engine detail page** (`/control-plane/engines/:id`): live status + pod list +
  **config editing** (scale, resources, catalogs) + delete.

## Dev / test

- **Unit**: the `KubeClient` trait gets a fake impl; tests cover CR templating +
  the auto-wire flow (Catalog Client creation + config injection) without a cluster.
- **E2e (opt-in, not CI)**: `just k8s-up` spins a `kind` cluster and installs the
  Stackable Trino operator; a manual test provisions a Trino, waits for readiness,
  and runs `SELECT` against the wired egret catalog. Documented like the existing
  manual Trino integration test.

## Error handling / prerequisites

- Operator/CRD missing → a clear `"Stackable Trino operator not installed"` error
  (detected when the `TrinoCluster` GVK is absent).
- Provision failure → `status = failed` + surfaced message; **auto-wire failure
  rolls back** any applied resources.
- Config-update validation errors → 400 with the message.
- **Prerequisite (documented, not enforced by egret):** the Stackable Trino
  operator is installed, and egret's `ServiceAccount` has RBAC to
  create/get/patch/delete `TrinoCluster` + `TrinoCatalog` + `Secret` and read
  `Pods` in the target namespace (default `egret-engines`, configurable).

## Future phases (own specs, same trait)
- **StarRocks** — `StarRocksCluster` CR (FE + BE/CN).
- **RisingWave** — `RisingWave` CR (meta + compute + frontend + connector).
- Per-workspace binding; scale-to-zero / autoscaling; operator install from egret.
