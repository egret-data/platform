# Engine config surface + multi-catalog, Trino on plain manifests — design (Phase 1.5, 2026-07-03)

VI reference: `docs/vi/specs/2026-07-03-engine-config-catalog-design.md`.
Builds on Phase 1: `docs/en/specs/2026-07-03-engine-provisioning-design.md`.

**Goal:** Two things, plus a mechanism pivot that makes both simple:
1. **Pivot Trino provisioning off the Stackable operator onto plain, stateless
   Kubernetes manifests** that egret templates itself (the shape the official
   `trinodb/trino` Helm chart produces) — Deployments, no StatefulSet, no PVC, no
   operator prerequisite.
2. **Expose a fuller Trino config surface** (curated fields + a raw escape hatch).
3. **Let a cluster have multiple catalogs** beyond the single auto-wired egret
   catalog (egret warehouses + Postgres + tpch/tpcds + a generic escape hatch).

**Why the pivot:** the Stackable operator models Trino as a StatefulSet with
per-pod TLS + a persistent `data-` PVC. Trino is essentially stateless, so those
PVCs bought nothing but operational pain — a crashed worker left a stale launcher
PID lock in its persistent data dir that survived restarts and self-perpetuated a
crashloop. Plain Deployments (the official Helm model) are stateless: restart
freely, no PVC, no stale lock, no operator to install.

**Non-goals (Phase 1.5):** the egret→Trino reverse proxy / external exposure
(that is **Phase 1.6**, its own spec — both path and subdomain modes, selectable
per engine); StarRocks/RisingWave (still their own operators later); credential
rotation; autoscaling.

## Mechanism: egret templates plain manifests (rewrite `trino.rs`)

egret keeps everything from Phase 1 (the `EngineProvisioner` trait, the
`KubeClient` seam, the `engines` data model, the control-plane routes, the
auto-wire, the frontend). Only `crates/egret-orchestrator/src/trino.rs` is
rewritten: instead of `TrinoCluster`/`TrinoCatalog` CRs, `TrinoProvisioner` applies
plain objects via the existing `KubeClient.apply`:

- **Deployment** `<name>-coordinator` (1 replica) — the Trino coordinator.
- **Deployment** `<name>-worker` (N replicas) — the workers.
- **Service** `<name>-trino` (ClusterIP, port 8080) — targets the coordinator.
- **ConfigMap** `<name>-config` — `config.properties`, `jvm.config`, `node.properties`.
- **ConfigMap** `<name>-catalogs` — one `<catalog>.properties` per catalog.
- **Secret** `<name>-catalog-<cat>` (when a catalog needs credentials) — mounted as
  env on both Deployments; catalog properties reference `${ENV:VAR}`.

Both Deployments mount the two ConfigMaps (config at `/etc/trino`, catalogs at
`/etc/trino/catalog`) and any catalog Secrets as env. No TLS/PVC/operator. The
Stackable operators are no longer a prerequisite for Trino.

The Deployments carry a `checksum/config` pod-template annotation set to a hash of
the rendered config+catalogs; changing it triggers a rolling restart (see below).

### Trino does not hot-reload catalogs
Trino uses static catalog management — changing `config.properties` or the catalogs
ConfigMap requires a **pod restart** to take effect. On any config/catalog change,
egret re-renders the ConfigMaps and updates the `checksum/config` annotation on both
Deployments, which rolls the pods. The API returns a "restarting to apply" status;
the UI surfaces it. This is cheap and safe with stateless Deployments.

## Config surface (curated + raw escape hatch)

`EngineSpec` (orchestrator) and the engine `spec` jsonb gain a `TrinoConfig`:

- **Curated** (typed, validated) → written to `config.properties` / `jvm.config`:
  - `jvm_heap` (e.g. `2G`) → `jvm.config` `-Xmx`/`-Xms`.
  - `query_max_memory` → `query.max-memory`.
  - `query_max_memory_per_node` → `query.max-memory-per-node`.
- **Raw escape hatch** (free key→value):
  - `config_properties: Map<String,String>` → appended to `config.properties`.
  - `env: Map<String,String>` → env on the Deployments.
  - Curated keys win over raw for the same key (documented precedence).

The existing replicas/resources/image-version fields stay.

## Multi-catalog

New table **`engine_catalogs`**:

| column | type | notes |
|---|---|---|
| id | uuid pk | |
| engine_id | uuid | FK `engines(id)` ON DELETE CASCADE |
| name | text | Trino catalog name (per-engine unique) |
| kind | text | `egret-warehouse` \| `postgresql` \| `tpch` \| `tpcds` \| `generic` |
| config | jsonb | kind-specific (warehouse id, pg host/port/db/user, connectorName+props) |
| secret_ref | uuid null | egret Secrets id for credentials (pg password, generic secret props) |
| is_primary | bool | the auto-wired egret catalog created at provision |
| created_at | timestamptz | |

- **Primary catalog** = the auto-wired egret warehouse from Phase 1, now stored as
  the first `engine_catalogs` row (`is_primary = true`, `kind = egret-warehouse`)
  so the whole set is uniform. Provision creates the engine + this row.
- Each row renders one `<name>.properties` file in the `<name>-catalogs` ConfigMap:
  - `egret-warehouse` → iceberg REST connector to egret (same properties Phase 1
    already generates), OAuth credential via `${ENV:...}` from a per-catalog Secret
    holding a freshly-minted Catalog Client (reuses Phase 1's auto-wire).
  - `postgresql` → `connector.name=postgresql`, `connection-url`, `connection-user`,
    `connection-password=${ENV:...}` from the egret-sourced Secret.
  - `tpch` / `tpcds` → `connector.name=tpch|tpcds` (no config, no secret).
  - `generic` → `connector.name=<connectorName>` + the raw properties map (values or
    `${ENV:...}` for secret-backed ones).
- Connector credentials are sourced from **egret's Secrets module** (`resolve_plaintext`),
  consistent with storage secrets — egret writes the resolved value into the k8s
  Secret it mounts. No plaintext passwords in the engine spec or the ConfigMap.

## Backend

- `EngineProvisioner` (orchestrator) gains nothing structurally — `provision`/`update`
  now render manifests; catalog add/remove flows through `update` (re-render the
  catalogs ConfigMap + roll). Provision renders the primary catalog; `update` renders
  the full catalog set from the DB.
- Control-plane `engines.rs`:
  - `PUT /engines/{id}` — accepts the `TrinoConfig` fields; re-renders + rolls.
  - `GET/POST /engines/{id}/catalogs` — list / add a catalog (creates the row +
    mints/creates any Secret + triggers a re-render+roll).
  - `DELETE /engines/{id}/catalogs/{cid}` — remove (deletes row + Secret + client +
    re-render+roll); refuses to delete `is_primary`.
- egret-bin adapter maps the richer `EngineProvisionReq` (now carrying the catalog
  set + config) to the orchestrator `EngineSpec`.

## Frontend (EngineDetail)

- **Config section** — curated fields (heap, max-memory) + a raw key→value editor
  for `config.properties` and env. Save → PUT → "restarting to apply" banner.
- **Catalogs section** — a list (name, kind, status) + an **Add catalog** dialog
  whose form switches on kind (egret-warehouse → warehouse picker; postgresql →
  host/port/db/user + a Secret picker for the password; tpch/tpcds → nothing;
  generic → connectorName + a properties key→value editor). Remove per row (primary
  is not removable).

## Dev / test

- **Unit**: the fake `KubeClient` verifies the rendered Deployment/Service/ConfigMap/
  Secret objects (names, replicas, config.properties contents, per-catalog files,
  env references) — no cluster.
- **Live (opt-in)**: a `kind` cluster with **no operators** — egret just applies the
  Deployments. Provision → pods Ready → query via the coordinator; add a `tpch`
  catalog → roll → `SELECT` across egret + tpch (cross-catalog join). Simpler than
  Phase 1's harness (drop the Stackable operator install).

## Migration / cleanup from Phase 1
- `trino.rs` CR templating is replaced by manifest templating.
- The Stackable-operator prerequisite and the `just k8s-up` operator install are
  removed from the docs/harness (kept only if StarRocks/RisingWave need operators).
- `reset-testdata.sql` already deletes engines; the PVC-cleanup concern disappears
  (no PVCs). `engine_catalogs` cascades on engine delete.
