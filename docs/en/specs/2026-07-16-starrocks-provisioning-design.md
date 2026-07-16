# StarRocks Provisioning — Operator + `StarRocksCluster` CR (shared-data) — Design

**Date:** 2026-07-16 · **Status:** Design — awaiting approval · **Branch:** `feat/starrocks-provisioning` (off main)
**Program:** StarRocks-parity-with-Trino, **slice 2 of 3**. Slice 1 (Ranger enforcement) shipped (#259) + follow-up (#260). Slice 3 (delegation/security parity) is a later spec. This spec covers **control-plane provisioning of StarRocks clusters onto Kubernetes**.

## Goal

Let a platform admin provision, manage, and delete **StarRocks** query engines from egret's control plane, at parity with Trino provisioning — a provisioned StarRocks cluster is **auto-wired to egret's embedded Iceberg catalog**, **enforces Ranger policies** (slice 1, now templated by the provisioner instead of deploy-side), and is **assignable to workspaces**. Proven live by running a real query through a provisioned cluster.

**Topology:** FE + CN in StarRocks **shared-data mode** — the lakehouse-native fit for egret (data lives in the object store, CN is stateless). Chosen over FE+BE (local PVs) with the user.

## Non-goals (scope boundaries)

- **egret does not manage the operator lifecycle.** The `kube-starrocks` operator + CRD are **pre-installed once** into the cluster (documented in `deploy/`), exactly as the Stackable Trino operator is pre-installed. The provisioner assumes the `StarRocksCluster` CRD is present.
- **MVP topology:** single FE (no HA) + 1 CN. HA-FE (3 FE), CN autoscaling, and a fuller config surface (heap tuning, resource groups, custom volumes) are follow-ups.
- **BE / classic shared-nothing mode is out** — shared-data only (this slice). A FE+BE variant can be added later behind the same provisioner.
- **No new authz.** Ranger enforcement is slice 1; this slice only relocates its FE config from `deploy/` into the provisioner.

## Background — current state (verified)

- **Trino provisioning templates raw k8s resources**, NOT an operator CR: `TrinoProvisioner` (`egret-orchestrator/src/trino.rs`) applies `Deployment` + `ConfigMap` + `Secret` + `Service` + `Ingress` per role. Confirmed live: the running `rgr2` engine is plain `apps/v1` Deployments (`rgr2-coordinator`/`rgr2-worker`), not a Stackable `trinocluster` CR. The "Stackable operator" comment in `egret-orchestrator/src/lib.rs` is stale. **StarRocks deliberately diverges** — it uses the operator + CR, because the operator exists locally and StarRocks (FE leader election, CN registration, shared-data storage volume) is impractical to hand-roll as raw Deployments.
- **`KubeClient` already applies CRs.** `egret-orchestrator/src/kube_client.rs`: `apply(obj: &serde_json::Value)` derives an `ApiResource` from the object's `apiVersion`+`kind` and does a server-side apply as a `DynamicObject`; `delete(apiVersion, kind, ns, name)` likewise. So applying a `StarRocksCluster` CR + ConfigMaps needs **no KubeClient change**. (kube-rs 0.99, k8s-openapi 0.24.)
- **`EngineProvisioner` seam.** `provision/status/update/delete` (`egret-orchestrator/src/provisioner.rs`). `TrinoProvisioner` is constructed once in `egret-bin/src/main.rs:1300`. `egret-engine-mgmt/src/engines.rs` `create_engine(name, "trino", ns, wh_id, spec)` (line ~1634) hardcodes `"trino"` — provisioning currently supports one engine type.
- **`RangerBinding`** (`egret-engine-mgmt-api`) already models the Ranger wiring (service_name, egret_url, poll_interval); slice 1 delivered the StarRocks FE config in `deploy/starrocks/ranger/`. This slice moves that into the provisioner's FE ConfigMap.
- **Engine→workspace assignment** (#247/#248) is engine-type-agnostic — reused as-is.
- **The operator repo** is checked out at `data-portal/starrocks-kubernetes-operator/`: CRD `deploy/starrocks.com_starrocksclusters.yaml`, installer `deploy/operator.yaml`, examples in `examples/starrocks/`.

## `StarRocksCluster` CR (authoritative shape, from the operator)

`apiVersion: starrocks.com/v1`, `kind: StarRocksCluster`. Relevant `spec` fields:

- `starRocksFeSpec`: `image` (`starrocks/fe-ubuntu:<ver>`), `replicas`, `limits`/`requests`, `storageVolumes` (fe-meta/fe-log PVs), `configMapInfo: {configMapName, resolveKey: fe.conf}` (mounts the FE config), and **`configMaps: [{name, mountPath}]`** (extra ConfigMap mounts — the Ranger XML goes here, mounted into the FE conf dir).
- `starRocksCnSpec`: `image` (`starrocks/cn-ubuntu:<ver>`), `replicas`, `limits`/`requests` (stateless — no local storage in shared-data).

**Shared-data `fe.conf`** (in the FE ConfigMap): query ports (`query_port=9030`, `http_port=8030`, `rpc_port=9020`, `edit_log_port=9010`), `run_mode = shared_data`, `cloud_native_meta_port = 6090`, `enable_load_volume_from_conf = true` + the object-store params so a **builtin default storage volume** is created at FE bootstrap:

```
cloud_native_storage_type = S3
aws_s3_endpoint = http://host.docker.internal:9000      # egret MinIO, reachable from kind pods
aws_s3_path = <bucket>/<starrocks-prefix>
aws_s3_access_key = <egret storage key>
aws_s3_secret_key = <egret storage secret>
aws_s3_use_aws_sdk_default_behavior = false
aws_s3_enable_path_style_access = true
```

…plus `access_control = ranger` (slice-1 Ranger enforcement) when the engine is bound to a policy profile.

## Architecture — components

### 1. `StarRocksProvisioner` (`egret-orchestrator/src/starrocks.rs`)

Implements `EngineProvisioner`:
- **`provision(spec)`** → apply (a) the **FE ConfigMap** (`fe.conf` templated from the spec's storage/ports/ranger), (b) the **Ranger XML ConfigMap** (`ranger-starrocks-security.xml` + `ranger-starrocks-audit.xml`, when `spec.ranger.is_some()`), (c) the **`StarRocksCluster` CR** referencing both. All via `KubeClient::apply(&Value)`.
- **`status(ns, name)`** → derive `EngineStatus` from the CR's pods (FE + CN) via the existing pod-list path.
- **`update(spec)`** → re-apply the ConfigMaps + CR (server-side apply is idempotent).
- **`delete(ns, name)`** → delete the `StarRocksCluster` CR (operator garbage-collects pods) + the ConfigMaps.

A new `spec.rs` carrier (`StarRocksConfig`: fe/cn image versions, replicas, resources, storage params) mirrors `TrinoConfig`. The CR/ConfigMap JSON is built by pure templating fns (`starrocks_cluster_cr`, `starrocks_fe_configmap`, `starrocks_ranger_configmap`) with unit tests over the emitted JSON — mirroring `trino.rs`'s `trino_config_map`/`trino_deployment` test pattern.

### 2. Provisioner registry (`egret-engine-mgmt` + `main.rs`)

`main.rs` builds a **type-keyed** provisioner map (`{"trino": TrinoProvisioner, "starrocks": StarRocksProvisioner}`) behind `Arc<dyn EngineProvisioner>`. `create_engine`/status/update/delete look up the provisioner by the engine's `type`. `create_engine(name, "starrocks", …)` is the StarRocks entry point.

### 3. Config surface (`egret-engine-mgmt-api` + `egret-engine-mgmt`)

`StarRocksEngineConfig` (analogue of `TrinoConfig`): fe/cn `image_version`, `replicas`, `resources`; storage params defaulted from egret's `config.storage` (endpoint/bucket/keys → the `aws_s3_*` block) so the admin usually only picks image + sizes. `EngineSpec` gains a StarRocks variant (or a `kind` discriminant) carrying this config; the create wizard seeds sensible defaults.

### 4. Auto-wire Iceberg (runtime)

Unlike Trino (catalog *files*), StarRocks registers an external catalog via SQL. After the FE is Ready, the provisioner (or the create flow) runs, via the FE mysql endpoint:

```sql
CREATE EXTERNAL CATALOG IF NOT EXISTS <egret_wh>
PROPERTIES ("type"="iceberg", "iceberg.catalog.type"="rest",
            "iceberg.catalog.uri"="<engine_egret_url>/catalog",
            "iceberg.catalog.warehouse"="<warehouse>", ... token ...);
```

This reuses the same `engine_egret_url` + warehouse the Trino auto-wire uses (`engines.rs`), and the per-engine catalog token minting already in place.

### 5. Ranger auto-wire

When the engine is bound to a policy profile, the FE `fe.conf` gets `access_control = ranger` and the Ranger XML ConfigMap is mounted (`ranger.plugin.starrocks.service.name = <profile>`, `policy.rest.url = <engine_egret_url>`). This is slice-1's config, now templated by the provisioner from `spec.ranger` (`RangerBinding`) instead of the manual `deploy/` files.

### 6. Workspace assignment

No change — the existing engine→workspace assignment (#247/#248) works on any provisioned engine.

## Data flow

Admin creates a StarRocks engine (control-plane UI) bound to the egret warehouse → `create_engine(…, "starrocks", …)` mints a catalog token + builds `EngineSpec` → `StarRocksProvisioner.provision` applies the FE/Ranger ConfigMaps + `StarRocksCluster` CR → operator materializes FE + CN pods → FE bootstraps the builtin MinIO storage volume + (if bound) loads Ranger policies from egret → auto-wire runs `CREATE EXTERNAL CATALOG` → the engine is queryable (external Iceberg reads) and, if bound, Ranger-enforced.

## Testing strategy

- **Unit (CI):** `starrocks_cluster_cr` / `starrocks_fe_configmap` / `starrocks_ranger_configmap` emit the expected JSON — FE `run_mode=shared_data` + storage params, CN spec, `configMapInfo` + `configMaps` mounts, `access_control=ranger` only when `spec.ranger.is_some()`, no ranger keys otherwise. Provisioner registry routes `"starrocks"` to the right provisioner. Mirrors `trino.rs`'s templating tests with a fake `KubeClient`.
- **DB/HTTP (CI):** `create_engine` for `"starrocks"` persists the engine + primary catalog row (mirror the Trino create tests).
- **Live-only (controller-driven, not CI):** operator installed → provision a StarRocks engine via the control plane → FE+CN pods Ready → external Iceberg catalog created → a real query through the provisioned FE returns rows from a seeded Iceberg table; if bound to a profile, a masked/denied query confirms Ranger still enforces on the *provisioned* cluster. Report exactly what ran.

## Live-only risks (validated at smoke)

1. **Shared-data storage bootstrap** — `enable_load_volume_from_conf=true` + the `aws_s3_*` block must create a working builtin volume against egret's MinIO (path-style, `host.docker.internal:9000`). If FE rejects it, fall back to a runtime `CREATE STORAGE VOLUME`. FE log surfaces it.
2. **Ranger XML mount path** — the `configMaps` mount must land the XML on the FE classpath / conf dir the StarRocks Ranger plugin reads (slice 1 found `/data/deploy/starrocks/fe/conf` on the allin1 image; the `fe-ubuntu` operator image path must be confirmed). One-line mountPath fix if wrong.
3. **Operator image availability** in kind (pull `starrocks/fe-ubuntu`/`cn-ubuntu` + the operator image). Pre-pull if the cluster is offline.
4. **CR ↔ pod status mapping** — `status()` must read FE+CN pod readiness correctly for the control-plane health UI.

## Work breakdown (for the plan)

1. **Operator install** — `deploy/starrocks-operator/` (vendored `operator.yaml` + CRD) + a `deploy/starrocks/README` section; install into kind once.
2. **`StarRocksProvisioner` + templating fns + `StarRocksConfig`** in `egret-orchestrator` (unit-tested JSON).
3. **Provisioner registry** — type-keyed dispatch in `main.rs` + `egret-engine-mgmt`.
4. **Config surface + `create_engine("starrocks")`** in `egret-engine-mgmt(-api)` + create-wizard defaults.
5. **Auto-wire Iceberg** (runtime `CREATE EXTERNAL CATALOG`) + **Ranger** templating from `spec.ranger`.
6. **FE control-plane** — StarRocks appears in the engine create/list/detail surfaces (largely engine-type-agnostic; add the StarRocks config form).
7. **Gate + LIVE SMOKE** — full CI locally + provision-and-query proof.

## Self-review

- **Scope focused:** provisioning only; shared-data FE+CN MVP; operator pre-installed; Ranger relocated-not-reinvented; tags/BE/HA/autoscaling excluded with reasons. ✔
- **No placeholders:** CR shape, shared-data `fe.conf` keys, mount fields (`configMapInfo`/`configMaps`), dispatch site (`engines.rs:1634`), provisioner construction (`main.rs:1300`), and KubeClient CR-apply are concrete/verified. ✔
- **Consistency:** reuses `EngineProvisioner`, `KubeClient`, `RangerBinding`, `engine_egret_url`, catalog-token minting, and #247/#248 assignment — StarRocks slots into the existing seams; the only new dispatch concept is the type-keyed registry. ✔
- **Ambiguity:** "provisioning parity" is pinned to shared-data FE+CN via the operator (not raw Deployments, not BE, not HA) so it can't be read as the full Trino config surface. ✔
