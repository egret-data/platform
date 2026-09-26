# FE metadata persistence — live result

**Measured:** 2026-09-26, kind context `kind-egret`, branch `fix/starrocks-fe-meta-persistence`
(`414035e2`), worktree `egret-sp4a`.

**Status: the pod-replacement gate is FAILED — NOT RUN.** The kind cluster is out of memory: the
API server is being OOM-killed, the scheduler keeps losing its lease, and the kube-starrocks
operator has not been able to start for hours. Nothing about the fix was disproven; nothing about
it was proven live either. Section 3 records exactly what blocked it, verbatim. Sections 0-2 are
measurements that *were* taken.

Every section below is written as it finished. A case that could not be run says so.

---

## 0. The state found before anything was changed (OBSERVED)

The defect, re-confirmed on the live cluster before touching code.

```text
$ kubectl -n egret-engines get pod sp3c2-sr-live-fe-0 -o jsonpath='{range .spec.volumes[*]}{.name}{"\t"}{@}{"\n"}{end}'
ssl-keystore	{"hostPath":{"path":"/var/lib/egret/starrocks/sp3c2-sr-live/ssl-keystore","type":"DirectoryOrCreate"},"name":"ssl-keystore"}
fe-meta	{"emptyDir":{},"name":"fe-meta"}
fe-log	{"emptyDir":{},"name":"fe-log"}
sp3c2-sr-live-fe-cm	{"configMap":{"defaultMode":420,"name":"sp3c2-sr-live-fe-cm"},"name":"sp3c2-sr-live-fe-cm"}
sp3c2-sr-live-ranger-cm-2334	{"configMap":{"defaultMode":420,"name":"sp3c2-sr-live-ranger-cm"},"name":"sp3c2-sr-live-ranger-cm-2334"}
sp3c2-sr-live-ranger-cm-3100	{"configMap":{"defaultMode":420,"name":"sp3c2-sr-live-ranger-cm"},"name":"sp3c2-sr-live-ranger-cm-3100"}

$ kubectl -n egret-engines get pod sp3c2-sr-live-cn-0 -o jsonpath='{range .spec.volumes[*]}{.name}{"\t"}{@}{"\n"}{end}'
cn-log	{"emptyDir":{},"name":"cn-log"}

$ kubectl -n egret-engines get pod sp3c2-sr-live-cn-0 -o jsonpath='{range .spec.containers[*].volumeMounts[*]}{.name}{" -> "}{.mountPath}{"\n"}{end}'
cn-log -> /opt/starrocks/cn/log

$ kubectl -n egret-engines get starrockscluster sp3c2-sr-live -o jsonpath='{.spec.starRocksFeSpec.storageVolumes}'
[{"hostPath":{"path":"/var/lib/egret/starrocks/sp3c2-sr-live/ssl-keystore","type":"DirectoryOrCreate"},"mountPath":"/etc/starrocks/ssl","name":"ssl-keystore"}]

$ kubectl -n egret-engines get starrockscluster sp3c2-sr-live -o jsonpath='{.spec.starRocksCnSpec.storageVolumes}'
                       # empty — the CN role declares no storageVolumes at all

$ kubectl -n egret-engines get pvc
No resources found in egret-engines namespace.

$ kubectl get sc
NAME                 PROVISIONER             RECLAIMPOLICY   VOLUMEBINDINGMODE      ALLOWVOLUMEEXPANSION   AGE
standard (default)   rancher.io/local-path   Delete          WaitForFirstConsumer   false                  70d
```

Three facts to keep: the only `storageVolumes` entry egret rendered was the TLS keystore hostPath;
`fe-meta` was the operator's `emptyDir` default; and **no PVC has ever existed** in
`egret-engines`, so no StarRocks engine egret has provisioned has ever had persistent metadata.

## 1. What egret renders after the fix (OBSERVED, from the renderer)

The FE role's `storageVolumes`, printed from `starrocks_cluster_cr` on the TLS fixture:

```json
[
  {
    "mountPath": "/opt/starrocks/fe/meta",
    "name": "fe-meta",
    "storageSize": "20Gi"
  },
  {
    "hostPath": {
      "path": "/var/lib/egret/starrocks/sr1/ssl-keystore",
      "type": "DirectoryOrCreate"
    },
    "mountPath": "/etc/starrocks/ssl",
    "name": "ssl-keystore"
  }
]
```

`storageClassName` is **absent**, not `""` — an empty string in k8s means "bind a pre-provisioned PV
only", the opposite of the intent, so an unset class must render as absent and let the cluster's
default StorageClass (`standard` here) bind the claim. `storageSize` defaults to `20Gi` and both
knobs are configurable per engine through `starrocks.feMetaStorageClass` /
`starrocks.feMetaStorageSize` on the engine create/update body.

The `mountPath` is the operator's own default meta path **on purpose**: kube-starrocks injects its
`emptyDir` default only for a path that no `storageVolumes` entry already mounts, so matching the
path is what suppresses the `emptyDir` instead of adding a second volume beside it. That claim is
read off the operator's behaviour and the CRD schema — **it is the part section 3 was supposed to
verify live, and did not.**

## 2. Every `emptyDir` the operator defaults for this cluster, with a verdict (enumerated from §0)

| volume | path | default | needs persistence? | why |
|---|---|---|---|---|
| `fe-meta` | `/opt/starrocks/fe/meta` | `emptyDir` | **YES — fixed here** | The BDBJE journal + image: catalogs, StarRocks-side users, roles, privileges, schemas. It is the only copy; the object store holds table DATA, not this. Losing it is `first start fe with meta not exist` and an engine that has forgotten everything egret configured on it. |
| `fe-log` | `/opt/starrocks/fe/log` | `emptyDir` | **No — deliberately left ephemeral** | `fe.log`/`fe.warn.log` are already read through `kubectl logs` and the pod's stdout. The one thing here with a compliance flavour is `fe.audit.log`, and egret does not rely on it: audit for governed access is Ranger's, shipped by the plugin. Consequence, stated so nobody is surprised: **a pod replacement loses the FE's log history.** If audit retention ever becomes a requirement, this row changes — a PVC, or a log shipper, not silence. |
| `cn-log` | `/opt/starrocks/cn/log` | `emptyDir` | **No — same reasoning as `fe-log`** | Compute-node logs, scraped out of the pod. |
| CN data / block cache | (no volume at all) | container filesystem | **No** | In shared-data mode the CN is stateless: its local cache is rebuilt from the object store. The cost of losing it is a cold cache, i.e. slower first queries, never lost state. Note the CN carries **no** `storageVolumes` and **no** data `emptyDir` — the cache lives in the container's writable layer, which is as ephemeral as an `emptyDir` and equally fine. |
| BE anything | — | — | **N/A** | egret provisions shared-data StarRocks: the CR has `starRocksFeSpec` + `starRocksCnSpec` and no `starRocksBeSpec` (pinned by the unit test `cr_has_fe_and_cn_shared_data`). There is no BE role, so no BE volume. |
| `ssl-keystore` (egret's own) | `/etc/starrocks/ssl` | `hostPath` (egret) | **No** | Rebuilt from the `{name}-ssl-secret` Secret by the `build-keystore` init-container on every boot. Losing it costs nothing, which is why a `hostPath` is correct *here* and wrong for metadata. |

Exactly one volume in this CR holds state that exists nowhere else, and that is the one that got a
PVC. The other four are ephemeral by decision, not by oversight.

## 3. The pod-replacement gate — FAILED, NOT RUN

**What was to be measured:** deploy the branch to kind with all six helm values, create a StarRocks
engine, give it a catalog and a non-root user, then `kubectl delete pod <fe>` **changing nothing
else**, and confirm afterwards that the catalog is still listed, the user still exists, the FE log
does not say `first start fe with meta not exist`, and the same PVC was rebound rather than a fresh
empty one created.

**Why it could not be run:** the cluster's node is out of memory. The API server is OOM-killed
(exit 137) and restarts; the scheduler loses its lease and restarts with it; and the operator that
would have to reconcile the new CR cannot start at all. In that state a `kubectl delete pod` would
not have produced a replacement to measure, and the backend image build (a `cargo build --release`
inside Docker, in the same 16 GiB VM) had nowhere to run.

```text
### free -m + uptime (inside egret-control-plane), 2026-09-26 20:39:41 +0700
               total        used        free      shared  buff/cache   available
Mem:           15991       15647         143         239         973         343
 13:39:41 up 3 days,  6:42,  0 user,  load average: 5.46, 50.87, 87.95

### kube-apiserver container list
CONTAINER      IMAGE          CREATED         STATE     NAME             ATTEMPT   POD
4a44045dad364  c6ab243b29f82  4 minutes ago   Running   kube-apiserver   27        kube-apiserver-egret-control-plane
0dcba039e2569  c6ab243b29f82  5 hours ago     Exited    kube-apiserver   26        kube-apiserver-egret-control-plane

### previous kube-apiserver exit
exitCode 137 reason Error finishedAt 2026-09-26T13:35:02.886495372Z

### operator
NAME                                       READY   STATUS             RESTARTS         AGE
kube-starrocks-operator-66f79d9767-2m54n   0/1     CrashLoopBackOff   232 (5m8s ago)   40d

### operator log tail
ERROR Failed to get API Group-Resources {"error": "Get \"https://10.96.0.1:443/api?timeout=32s\":
  dial tcp 10.96.0.1:443: connect: connection refused"}
ERROR main unable to start manager {"error": "Get \"https://10.96.0.1:443/api?timeout=32s\":
  dial tcp 10.96.0.1:443: connect: connection refused"}

### kube-scheduler, previous attempt (284 of 285)
E leaderelection.go:436] error retrieving resource lock kube-system/kube-scheduler: Get
  "https://172.23.0.2:6443/apis/coordination.k8s.io/v1/namespaces/kube-system/leases/kube-scheduler?timeout=5s":
  context deadline exceeded
I leaderelection.go:297] failed to renew lease kube-system/kube-scheduler: context deadline exceeded
E server.go:303] "Leaderelection lost"

### allocated on the single node
cpu     5860m (73%)    limits 13100m (163%)
memory  14764Mi (92%)  limits 22310Mi (139%)
```

The obvious remedy — freeing memory by scaling down the two idle Trino engines from already-merged
gates (`sp3c-live-trino`, `sp3c2-trino-live`, six JVM containers between them) — was **refused by
the sandbox** as interference with other workloads, and `demo-trino` is off limits by standing
project constraint in any case. So this needs a human: give the Docker VM more memory, or stop the
idle engines, then run section 5.

**Also not measured, for the same reason:** the one-time migration loss on an existing `emptyDir`
engine (`sp3c2-sr-live`). What that engine loses on its first roll after the upgrade is documented
in `deploy/helm/egret/README.md` (the page an operator reads before deploying or upgrading this
stack) as the **prediction** it currently is — catalogs,
StarRocks-side users and roles — and section 5 step 6 is how to turn it into an observation.

## 4. Confirmed untouched

```text
$ kubectl -n egret-engines get pod demo-trino-coordinator-c54f46974-xwgbt
NAME                                     READY   STATUS    RESTARTS        AGE
demo-trino-coordinator-c54f46974-xwgbt   1/1     Running   54 (106m ago)   5d23h
```

No `kubectl apply`, `patch`, `scale`, `delete` or `helm` write was issued against this cluster by
this task — every command run was a read. `demo-trino` was neither used nor modified.

## 5. The recipe the next run must follow

1. Free memory first (Docker VM ≥ 24 GiB, or stop the idle Trino engines) and confirm the API
   server stops restarting and `kube-starrocks-operator` is `1/1 Running`.
2. Deploy with **all six** values — dropping `ranger.enabled` makes every engine deny every query,
   which reads as "the policies work" while measuring nothing:

   ```bash
   just images-build
   kind load docker-image egret-backend:dev egret-frontend:dev egret-kernel:dev --name egret
   helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 10m \
     --set oidc.enabled=true --set ingress.enabled=true --set ranger.enabled=true \
     --set kernel.launcher=kubernetes --set backend.replicas=2 --set sharedStore.enabled=true
   ```
3. Verify the deploy **by the pod**, never by helm's report (which has claimed success here while
   pods stayed hours old): the serving pods' `imageID` equals the image just loaded, and
   `EGRET_RANGER__ENABLED=true` read with `kubectl exec … -- env` from inside a running pod.
4. Create a StarRocks engine, wait for the FE to be Ready, then add a catalog and a non-root user.
   Record `SHOW CATALOGS` and `SELECT user FROM information_schema.user_privileges` (or
   `SHOW GRANTS FOR <user>`) as the baseline.
5. **Delete the FE pod and change nothing else.** PR #597's `C1` hid for a whole live gate because
   the apply under test also changed a mount list, so the pod rolled for an unrelated reason and the
   mechanism under test was never exercised. Then check, verbatim:
   - `SHOW CATALOGS` still lists the catalog;
   - the non-root user still exists;
   - `kubectl logs` does **not** contain `first start fe with meta not exist`;
   - the pod's `fe-meta` volume is a `persistentVolumeClaim`, not an `emptyDir`;
   - the PVC was **retained and rebound**: capture `kubectl get pvc fe-meta-<sts>-0 -o
     jsonpath='{.metadata.uid} {.spec.volumeName} {.metadata.creationTimestamp}'` before and after
     and require all three to be **identical**. A fresh empty PVC each time passes a naive "the pod
     came back" check while losing the data anyway.
6. Take `sp3c2-sr-live` (an engine provisioned with the `emptyDir` default) through an update so it
   picks up the PVC, and record what it **actually** loses: `SHOW CATALOGS` before and after, the
   non-root users before and after, and whether `POST /catalogs/{id}/reapply` restores the catalogs.
   That turns section 3's prediction into a measurement.
