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

### StarRocks engines lose their FE metadata ONCE on this upgrade

Before this release egret rendered no `storageVolumes` entry for the StarRocks FE's metadata
directory, so the kube-starrocks operator applied its own default — `fe-meta: {emptyDir: {}}`.
Metadata therefore lived and died with the pod. This release gives **every** StarRocks engine, new
and existing alike, a PersistentVolumeClaim at `/opt/starrocks/fe/meta`.

Changing the volume replaces the FE pod, and the pod being replaced is what discards the old
`emptyDir`. So **each existing StarRocks engine loses its FE metadata exactly once**, on its first
FE roll after the upgrade. From then on the metadata survives pod replacement. New engines never
see this.

**What is lost**, observed on PR #597's live gate when a pod replacement wiped an `emptyDir`
(`first start fe with meta not exist` in `fe.log`):

- **every catalog on that engine** — `SHOW CATALOGS` came back with `default_catalog` alone,
  including the `egret-warehouse` Iceberg catalog egret wires up at create time;
- **every StarRocks-side user, role and grant** — the `analyst` user was simply gone;
- **the `root` password** — `root` came back with no password at all. The initpwd Job that sets it
  has already `Completed` and does **not** re-run by itself;
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
