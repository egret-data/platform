# egret Helm chart (local kind)

Deploys the full egret stack into a local **kind** cluster for development.

> **Dev only — NOT for production.** This chart bundles Postgres and MinIO,
> ships an insecure local-admin login, and targets NodePorts on `localhost`. It
> is meant for a single-developer kind cluster, not a real deployment.

## Prerequisites

- **Docker** running.
- A **kind cluster named `egret`** with the target namespace and engine
  operators. Create it with `just k8s-up` (see [`deploy/k8s/README.md`](../../k8s/README.md)),
  which also maps the NodePorts used below in [`deploy/k8s/kind.yaml`](../../k8s/kind.yaml).
- The **kube-starrocks operator** installed (see [`deploy/starrocks-operator/`](../../starrocks-operator/)),
  so the backend can provision StarRocks engines.

## Deploy

```bash
just deploy-kind
```

This runs `just images-build` (builds `egret-backend:dev` and
`egret-frontend:dev`), then:

```bash
kind load docker-image egret-backend:dev egret-frontend:dev --name egret
helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m
```

> The backend image is a **full Rust release build** — the first build is slow.
> Subsequent builds reuse the Docker layer cache and are much faster.

## Access

- **SPA:** <http://localhost:30080> — the frontend is exposed on NodePort
  `30080`, mapped to the host in [`deploy/k8s/kind.yaml`](../../k8s/kind.yaml).
- **Login:** local-admin `admin@egret.local` / `Admin12345!`.
  OIDC is **off** in this milestone, so the local-admin account is the only way in.

## What runs

- **egret backend** (in-cluster) — provisions engines into the `egret-engines`
  namespace.
- **nginx frontend** — serves the built SPA.
- **Bundled Postgres** — the control-plane metastore.
- **Bundled MinIO** — object storage, pre-created bucket `warehouse`.

## Configuration

Everything is driven by `EGRET_*` environment variables, sourced from the
ConfigMap `egret-env` and the Secret `egret-secrets`. Edit the values in
[`values.yaml`](./values.yaml).

To use an **external Postgres/MinIO** instead of the bundled ones, set
`deps.enabled=false` and point the `EGRET_*` connection values at your
services.

## Upgrade notes

### EXISTING StarRocks engines need a manual step, and lose their FE metadata when you take it

Before this release egret rendered no `storageVolumes` entry for the StarRocks FE's metadata
directory, so the kube-starrocks operator applied its own default — `fe-meta: {emptyDir: {}}`.
Metadata therefore lived and died with the pod. This release renders a PersistentVolumeClaim at
`/opt/starrocks/fe/meta` for **every** StarRocks engine egret provisions.

**New engines get it automatically and need nothing from you** (measured:
`docs/en/plans/2026-09-26-fe-meta-live-result.md` §4-§6 — a bound PVC, and a catalog, a user, its
grant and the root password all surviving `kubectl delete pod` on the FE).

**Existing engines do NOT pick it up on their own.** A `storageSize` entry becomes a StatefulSet
`spec.volumeClaimTemplates` entry, and `volumeClaimTemplates` is **immutable** on a StatefulSet that
already exists. Measured on an engine created before this release (§7 of the same document): egret
updates the `StarRocksCluster` correctly, then every operator reconcile is rejected by the API
server —

```text
ERROR StarRocksClusterReconciler.feController deploy statefulset failed
  error: failed to apply merge patch: StatefulSet.apps "<engine>-fe" is invalid:
    spec: Forbidden: updates to statefulset spec for fields other than 'replicas', 'ordinals',
    'template', 'updateStrategy', 'revisionHistoryLimit', 'persistentVolumeClaimRetentionPolicy'
    and 'minReadySeconds' are forbidden
```

— and it keeps retrying. The engine's FE keeps its `emptyDir` (so it is no safer than before), and
the `StarRocksCluster` sits in `phase=failed`, which means **no other change to that engine
reconciles either** until this is cleared.

**The step, per existing StarRocks engine.** Recreating the FE StatefulSet is what lets the operator
build it with the claim template. It replaces the FE pod, and that is the moment the old `emptyDir`
is discarded — so do the "before upgrading" work below first:

```bash
kubectl -n egret-engines delete statefulset <engine>-fe --cascade=orphan   # keeps the FE pod alive
kubectl -n egret-engines delete pod <engine>-fe-0                          # FE metadata is lost here
```

Then confirm `kubectl -n egret-engines get pvc` shows `fe-meta-<engine>-fe-0` as `Bound`, and the new
pod's log says `start fe with exist meta.` on its *next* replacement. From then on the metadata
survives pod replacement.

**What that step loses** — observed on PR #597's live gate when a pod replacement wiped an `emptyDir`
(`first start fe with meta not exist` in `fe.log`). This list has not been re-measured through the
StatefulSet recreation above; treat it as the expectation to plan around:

- **every catalog on that engine** — `SHOW CATALOGS` came back with `default_catalog` alone,
  including the `egret-warehouse` Iceberg catalog egret wires up at create time;
- **every StarRocks-side user, role and grant** — the `analyst` user was simply gone;
- **the `root` password** — `root` came back with no password at all when the pod was replaced
  outside egret. Note, measured: an engine **update through egret** (`PUT /engines/{id}`) re-templates
  the `initpwd` Job and it runs again within seconds, so the password is restored on that path — but a
  bare `kubectl delete pod` does not go through egret and does not re-run it;
- **anything created directly on the FE** — databases, views, materialized views. Table DATA in the
  object store is untouched; what is lost is the FE's record of it.

**What recovers it, and what does not:**

| lost | recovered by |
|---|---|
| catalogs | `POST /api/control-plane/catalogs/{id}/reapply` per registry catalog (needs `catalog_registry:update`), which re-applies the stored configuration to every attached engine |
| StarRocks users, roles, grants | **nothing automatic — recreate them by hand.** egret does not keep a copy of StarRocks-side principals to replay |
| the `root` password | **not restored by reapply.** Update the engine so the initpwd resources are re-applied, or set it on the FE yourself |
| databases / views created directly on the FE | **nothing — recreate them.** Only the object-store data survives |

**So before upgrading:** write down the StarRocks-side users, roles and grants of every existing
StarRocks engine (`SHOW GRANTS`), and expect to re-run `reapply` for its catalogs afterwards. If an
engine's FE state matters and cannot be rebuilt, snapshot
`/opt/starrocks/fe/meta` out of the running pod first (`kubectl cp`).

Per-engine knobs, both optional, on the engine create/update body under `starrocks`:

| field | default | meaning |
|---|---|---|
| `feMetaStorageClass` | absent ⇒ the cluster's **default** StorageClass (`standard` on kind) | StorageClass backing the claim |
| `feMetaStorageSize` | `20Gi` | claim size; FE metadata is small, so this is generous |

### Logs still do not survive a pod replacement

Only the FE metadata volume was made persistent. `fe-log` (`/opt/starrocks/fe/log`) and `cn-log`
(`/opt/starrocks/cn/log`) stay on `emptyDir` **by design**, and the CN's shared-data block cache has
no volume at all (it is rebuilt from the object store). The consequence is deliberate and worth
knowing: **a replaced pod starts with empty logs, and the previous pod's `fe.log` / `fe.audit.log`
are gone.** Read them with `kubectl logs` while the pod lives, or ship them off the pod, if you need
them after a restart. Governed-access audit does not depend on this: that is Ranger's, written by
the plugin.

## Teardown

```bash
just kind-undeploy   # uninstall the egret release only (keeps cluster + operators)
just k8s-down        # delete the whole kind cluster
```

## Roadmap

OIDC / Keycloak login is **Milestone B** — see the deployment plan/spec. This
milestone (A) ships local-admin auth only.

## Troubleshooting

- **`http://localhost:30080` refused / SPA unreachable.** kind `extraPortMappings`
  (the `30080` host map in `deploy/k8s/kind.yaml`) only take effect when the
  cluster is **created**. On a cluster that predates that mapping, either recreate
  it (`just k8s-down && just k8s-up`) or port-forward the frontend instead:
  `kubectl port-forward -n egret-system svc/egret-frontend 8090:80` → browse
  `http://localhost:8090`.
- **"Invalid email or password" on a fresh install.** The `egret-init` post-install
  hook Job bootstraps the admin; if it hasn't finished, wait for it
  (`kubectl get job -n egret-system egret-init -w`) or run it manually:
  `kubectl exec -n egret-system deploy/egret -- egret init`.
- **Backend restarts once on first boot.** Expected — it races Postgres becoming
  ready, times out once, then reconnects (`restartPolicy: Always`).
