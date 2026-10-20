# egret Helm chart (local kind)

Deploys the full egret stack into a local **kind** cluster for development.

> **Dev only — NOT for production.** This chart bundles Postgres and RustFS,
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
- **Bundled RustFS** — S3-compatible object storage (Service `rustfs`), pre-created
  bucket `warehouse`.

## Configuration

Everything is driven by `EGRET_*` environment variables, sourced from the
ConfigMap `egret-env` and the Secret `egret-secrets`. Edit the values in
[`values.yaml`](./values.yaml).

To use an **external Postgres/S3 store** instead of the bundled ones, set
`deps.enabled=false` and point the `EGRET_*` connection values at your
services.

## Upgrade notes

### The Ranger surface moved to `/ranger` and requires a credential

With `ranger.enabled=true`, engine plugins poll egret at `<egret>/ranger` (the root `/service`
prefix is gone, with no alias), and `ranger.authMode` (rendered as `EGRET_RANGER__AUTH_MODE`)
defaults to `required`: every plugin must present its engine's own HTTP Basic credential, or it
gets 401. Provisioned engines get their credential on their next apply, which rolls their pods;
apply each bound engine once (save its settings, or call
`POST /api/control-plane/engines/{id}/rotate-ranger-credential`). `just deploy-kind-governance` no longer
sets `ranger.authMode=anonymous`. Set `--set ranger.authMode=anonymous` only for a throwaway
cluster: anyone who can reach the URL can then download every profile's policies. With
`ingress.enabled=true` the ingress routes `/ranger` to the backend, so an engine outside the
cluster can use `https://<ingress host>/ranger`. Trino engines from before this release printed
their Ranger password in their pod logs; rotate each bound Trino engine's credential once (the
rotation also renders the fixed `log.properties`). Details,
external engines and troubleshooting: [`docs/en/guides/ranger-surface.md`](../../../docs/en/guides/ranger-surface.md).

### The bundled object store is RustFS (was MinIO)

MinIO withdrew its public images, so the bundled S3 server is now RustFS. Everything that named
MinIO was renamed with it:

| Before | Now |
|---|---|
| Service / Deployment `minio` (`http://minio:9000`) | `rustfs` (`http://rustfs:9000`) |
| values `deps.minio.*` | `deps.rustfs.*` |
| `deps.minio.mcImage` | `deps.rustfs.clientImage` (a shell plus the `aws` CLI; creates the bucket once) |
| PersistentVolumeClaim `minio-data` | `rustfs-data` |

**Upgrading an existing dev release starts with an empty bucket.** RustFS cannot read MinIO's
on-disk format, so it gets a new volume. Whatever the old MinIO held (Iceberg data files, query
results, workflow files, Data Apps sources) is not carried over, and the catalog rows in Postgres
that point at those objects are left dangling. Reinstall, or reseed, rather than upgrade in place.

**Engines provisioned before the upgrade keep `http://minio…:9000` in their rendered catalog
properties and storage connections**, and that name no longer resolves. Re-create the storage
connection with the new endpoint and re-apply the catalogs, or re-provision the engines.

Helm leaves the old `minio-data` claim in place (it does not delete PVCs it no longer renders);
delete it by hand when you no longer need it.

### StarRocks FE metadata is persistent for engines created from this release on

Before this release egret rendered no `storageVolumes` entry for the StarRocks FE's metadata
directory, so the kube-starrocks operator applied its own default — `fe-meta: {emptyDir: {}}`.
Metadata therefore lived and died with the pod. This release renders a PersistentVolumeClaim at
`/opt/starrocks/fe/meta` for every StarRocks engine **created after the upgrade**.

**New engines get it automatically and need nothing from you** (measured:
`docs/en/plans/2026-09-26-fe-meta-live-result.md` §4-§6 — a bound PVC, and a catalog, a user, its
grant and the root password all surviving `kubectl delete pod` on the FE).

**Existing engines are left exactly as they are: upgrading changes nothing for them.** They keep the
operator's `emptyDir`, so their FE metadata is no safer than before — and their clusters keep
reconciling, which is the point. egret decides this from a marker (`starrocksConfig.feMetaPersistent`)
stamped into the stored spec at create time, so it is only ever set on engines created after this
release; a later `PUT /engines/{id}` preserves whatever is stored and never adds it.

**Why it is not applied to existing engines** (measured, §7 of the same document; this is a
constraint, not caution). A `storageSize` entry becomes a StatefulSet `spec.volumeClaimTemplates`
entry, and `volumeClaimTemplates` is **immutable** on a StatefulSet that already exists. When egret
rendered it unconditionally, the `StarRocksCluster` updated correctly and then every operator
reconcile was rejected by the API server —

```text
ERROR StarRocksClusterReconciler.feController deploy statefulset failed
  error: failed to apply merge patch: StatefulSet.apps "<engine>-fe" is invalid:
    spec: Forbidden: updates to statefulset spec for fields other than 'replicas', 'ordinals',
    'template', 'updateStrategy', 'revisionHistoryLimit', 'persistentVolumeClaimRetentionPolicy'
    and 'minReadySeconds' are forbidden
```

— forever. The FE kept its `emptyDir` (so it gained nothing), and the `StarRocksCluster` sat at
`phase=failed`, which means **no other change to that engine reconciled either** — not an image
bump, not a rescale, not a Ranger rebind. That outcome is strictly worse than leaving the engine
alone, which is why the marker exists.

**Migrating an existing engine is a deliberate operator action, never a side effect of upgrading.**
Nothing below happens on its own, and none of it is required. Recreating the FE StatefulSet is what
lets the operator build it with a claim template; it replaces the FE pod, and that is the moment the
old `emptyDir` — and with it that FE's metadata — is destroyed. Read the loss list below first.
There is also no egret-side way to set the marker on an existing engine today, so an engine migrated
this way renders no claim template until it is recreated through egret; if the metadata matters, the
supported route is to create a NEW engine and repoint at it.

```bash
kubectl -n egret-engines delete statefulset <engine>-fe --cascade=orphan   # keeps the FE pod alive
kubectl -n egret-engines delete pod <engine>-fe-0                          # FE metadata is destroyed here
```

**What that loses** — observed on PR #597's live gate when a pod replacement wiped an `emptyDir`
(`first start fe with meta not exist` in `fe.log`):

- **every catalog on that engine** — `SHOW CATALOGS` came back with `default_catalog` alone,
  including the `egret-warehouse` Iceberg catalog egret wires up at create time;
- **every StarRocks-side user, role and grant** — the `analyst` user was simply gone;
- **anything created directly on the FE** — databases, views, materialized views. Table DATA in the
  object store is untouched; what is lost is the FE's record of it.

**The `root` password is a special case, and the honest answer is "sometimes".** The claim that it is
simply lost is wrong: an engine update through egret (`PUT /engines/{id}`) deletes and re-applies the
`initpwd` Job whenever a root password is on record, and on 2026-09-26 that Job re-ran within
seconds and restored the password (§7.4 of the live-result document). It did **not** reproduce on
2026-09-27: a PUT on the same engine templated no `initpwd` Job at all and `root` came back with no
password (§10). So treat a restored root password as likely on the egret path and not guaranteed, and
check `SHOW ALL AUTHENTICATION` after any FE roll. A bare `kubectl delete pod` never restores it —
that path does not go through egret at all.

**What recovers the rest, and what does not:**

| lost | recovered by |
|---|---|
| catalogs | `POST /api/control-plane/catalogs/{id}/reapply` per registry catalog (needs `catalog_registry:update`), which re-applies the stored configuration to every attached engine |
| StarRocks users, roles, grants | **nothing automatic — recreate them by hand.** egret does not keep a copy of StarRocks-side principals to replay |
| databases / views created directly on the FE | **nothing — recreate them.** Only the object-store data survives |

**Before migrating an engine by hand:** write down its StarRocks-side users, roles and grants
(`SHOW GRANTS`), expect to re-run `reapply` for its catalogs afterwards, and if its FE state matters
and cannot be rebuilt, snapshot `/opt/starrocks/fe/meta` out of the running pod first
(`kubectl cp`). None of this is needed to upgrade egret.

Per-engine knobs, both optional, **CREATE-ONLY**, and only meaningful for an engine created after
this release:

| field | default | meaning |
|---|---|---|
| `feMetaStorageClass` | absent ⇒ the cluster's **default** StorageClass (`standard` on kind) | StorageClass backing the claim |
| `feMetaStorageSize` | `20Gi` | claim size; FE metadata is small, so this is generous |

`PUT /engines/{id}` **refuses** both fields — it does not accept and ignore them. They render inside
the FE StatefulSet's `spec.volumeClaimTemplates`, which Kubernetes makes immutable once the
StatefulSet exists, so on exactly the engines they are meaningful for they are also **destructive**:
changing either would not resize anything, it would park that engine's `StarRocksCluster` at
`phase=failed` with nothing reconciling again. Pick the class and size when you create the engine.
There is no bypass flag; resizing a running engine means recreating its FE StatefulSet by hand, which
destroys that FE's metadata (see above), or creating a new engine.

Two values are refused at CREATE time as well, because the operator reads them as "render no claim"
and would silently leave you on ephemeral storage: `feMetaStorageClass: emptyDir` or `hostPath` (they
are special to the operator, not StorageClass names) and any zero `feMetaStorageSize`. A
`feMetaStorageSize` that is not a positive binary quantity (`20Gi`, `1.5Ti`, `512Mi` — `Mi/Gi/Ti/Ei/Pi`
only) is a 400 at create rather than a CR rejection minutes later.

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
