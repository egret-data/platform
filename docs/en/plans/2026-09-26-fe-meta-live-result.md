# FE metadata persistence — live result

**Measured:** 2026-09-26, kind context `kind-egret`, branch `fix/starrocks-fe-meta-persistence`
(`414035e2`), worktree `egret-sp4a`.

**Status: the pod-replacement gate was RUN — and it PASSES for new engines. The one-time-migration
claim is FALSIFIED.** The cluster that blocked the first attempt was repaired (Docker VM raised to
23.5 GiB, API server steady, `kube-starrocks-operator` `1/1 Running`, both egret pods `1/1`), the
branch was built from `414035e2` and deployed, and the four checks were run. Checks 1-3 pass: a new
engine renders a bound PVC beside the TLS entry, its catalog / user / grant / root password all
survive deleting the FE pod with nothing else changed, and the *same* PVC is rebound. Check 4 found
something the unit tests could not: an **existing** `emptyDir` engine does not migrate at all —
`volumeClaimTemplates` is immutable on a live StatefulSet, so the operator's patch is rejected, the
engine keeps its `emptyDir`, and its `StarRocksCluster` sits in `phase=failed` retrying forever. §7
has the verbatim operator error. Section 3 is the deploy, verified by the pods; §9 is the scoreboard.
Sections 0-2 are the earlier run's observations, re-confirmed today in §2b.

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
read off the operator's behaviour and the CRD schema when this section was written — **§4 has since
confirmed it live**: the rendered CR produces a bound PVC at that path and the pod carries no
`fe-meta` `emptyDir` beside it.

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

## 2b. The cluster as found on the re-run, 22:08 +07 (OBSERVED)

The memory exhaustion that blocked the first attempt is gone, and — the fact that matters — there
is still **no PVC anywhere in `egret-engines`**, so nothing measured below can be a leftover.

```text
$ kubectl get pods -A | grep starrocks
starrocks   kube-starrocks-operator-66f79d9767-2m54n   1/1   Running   248   40d

$ kubectl -n egret-engines get pvc
No resources found in egret-engines namespace.

$ kubectl -n egret-engines get starrockscluster
NAME            PHASE     FESTATUS   BESTATUS   CNSTATUS   FEPROXYSTATUS
sp3c2-sr-live   running   running               running

$ kubectl -n egret-engines get pod sp3c2-sr-live-fe-0 -o jsonpath='{...volumes...}'
fe-meta	{"emptyDir":{},"name":"fe-meta"}          # still the operator's default

$ docker exec egret-control-plane free -m
               total        used        free      shared  buff/cache   available
Mem:           24031        9664        3483         158       11647       14366

$ kubectl describe node egret-control-plane | sed -n '/Allocated/,/Events/p'
  cpu                3360m (42%)   7100m (88%)
  memory             4524Mi (18%)  12070Mi (50%)
```

## 3. The deploy, verified by the pods and not by helm (OBSERVED)

The `egret-backend:dev` image in the daemon was created `2026-09-26T09:35:33Z` (16:35 +07) —
**before** the fix commit (20:34 +07). So the pods that were running at task start could not have
carried the fix, whatever helm's revision said. Rebuilt from this worktree:

```text
$ docker build -f deploy/docker/backend.Dockerfile -t egret-backend:dev .      # exit 0
$ docker image inspect egret-backend:dev --format '{{.Created}}'
2026-09-26T15:35:35.835409769Z                                  # = 22:35 +07, AFTER 414035e2

$ kind load docker-image egret-backend:dev --name egret
Image: "egret-backend:dev" with ID "sha256:cf279741…" not yet present on node "egret-control-plane", loading...

$ docker exec egret-control-plane crictl inspecti docker.io/library/egret-backend:dev
id: sha256:66d7d0f7a56147d0a045e1ab73f8a07b5051ea8ecebecf2250da10ef5fa8ce4c
```

```text
$ helm --kube-context kind-egret upgrade egret deploy/helm/egret -n egret-system --wait --timeout 10m \
    --set oidc.enabled=true --set ingress.enabled=true --set kernel.launcher=kubernetes \
    --set backend.replicas=2 --set sharedStore.enabled=true --set ranger.enabled=true
Release "egret" has been upgraded. Happy Helming!
REVISION: 67

$ kubectl rollout restart deploy/egret -n egret-system && kubectl rollout status …
deployment "egret" successfully rolled out
```

**The two verifications that actually count.** The serving pods run the image just loaded:

```text
$ kubectl -n egret-system get pods -l app=egret -o custom-columns=NAME:…,READY:…,IMAGEID:…
NAME                     READY   IMAGEID
egret-8559cf9b55-brvbl   true    sha256:66d7d0f7a56147d0a045e1ab73f8a07b5051ea8ecebecf2250da10ef5fa8ce4c
egret-8559cf9b55-tbxm7   true    sha256:66d7d0f7a56147d0a045e1ab73f8a07b5051ea8ecebecf2250da10ef5fa8ce4c
```

identical to the node's `egret-backend:dev` id above. And Ranger is on, read from **inside** each
running pod rather than off the values file:

```text
$ kubectl -n egret-system exec egret-8559cf9b55-brvbl -- env | grep -E 'EGRET_RANGER__ENABLED|EGRET_OIDC__ENABLED|EGRET_KERNEL__LAUNCHER'
EGRET_RANGER__ENABLED=true
EGRET_OIDC__ENABLED=true
EGRET_KERNEL__LAUNCHER=kubernetes
$ kubectl -n egret-system exec egret-8559cf9b55-tbxm7 -- env | grep …
EGRET_RANGER__ENABLED=true
EGRET_OIDC__ENABLED=true
EGRET_KERNEL__LAUNCHER=kubernetes
```

## 4. Check 1 — a new StarRocks engine renders a PVC: **PASS** (OBSERVED)

Created through egret's own API, not by hand-editing a CR:

```text
$ curl -b <session> -X POST http://egret.localtest.me/api/control-plane/engines \
    -H 'x-egret-csrf: …' -d @create.json
{"id":"ebca28a8-cfdf-40b0-b53e-5c972bb68793","name":"femeta-live","type":"starrocks",
 "status":"provisioning",
 "spec":{…,"starrocksConfig":{…,"delegation":{"mode":"jwt","tlsEnabled":true},
   "feMetaStorageSize":null,"feMetaStorageClass":null}},
 "catalogAttachErrors":[]}
```

Both new knobs are `null` — the body set neither — so this engine exercises the **default** path,
which is the one every pre-existing engine will also take.

```text
$ kubectl -n egret-engines get starrockscluster femeta-live -o jsonpath='{.spec.starRocksFeSpec.storageVolumes}'
[
    {
        "mountPath": "/opt/starrocks/fe/meta",
        "name": "fe-meta",
        "storageSize": "20Gi"
    },
    {
        "hostPath": {
            "path": "/var/lib/egret/starrocks/femeta-live/ssl-keystore",
            "type": "DirectoryOrCreate"
        },
        "mountPath": "/etc/starrocks/ssl",
        "name": "ssl-keystore"
    }
]
```

Every clause of the check, in that output: the meta entry exists, at the operator's own meta path,
with a `storageSize` and **no `hostPath`** and **no `storageClassName`** (absent, not `""`); and the
TLS keystore entry is still there, **alongside** it as element 2 rather than in place of it — the
append, not an overwrite.

The claim is real and bound, and the operator's `emptyDir` default is gone from the pod:

```text
$ kubectl -n egret-engines get pvc
NAME                       STATUS   VOLUME                                     CAPACITY   ACCESS MODES   STORAGECLASS   AGE
fe-meta-femeta-live-fe-0   Bound    pvc-84ea751b-de7a-4dda-b274-d34a171d605d   20Gi       RWO            standard       13s

$ kubectl -n egret-engines get pod femeta-live-fe-0 -o jsonpath='{range .spec.volumes[*]}{.name}{"\t"}{@}{"\n"}{end}'
fe-meta	{"name":"fe-meta","persistentVolumeClaim":{"claimName":"fe-meta-femeta-live-fe-0"}}
ssl-keystore	{"hostPath":{"path":"/var/lib/egret/starrocks/femeta-live/ssl-keystore","type":"DirectoryOrCreate"},"name":"ssl-keystore"}
fe-log	{"emptyDir":{},"name":"fe-log"}
femeta-live-fe-cm	{"configMap":{"defaultMode":420,"name":"femeta-live-fe-cm"},"name":"femeta-live-fe-cm"}
```

`fe-meta` is a `persistentVolumeClaim`; compare §0, where the same line read `{"emptyDir":{}}`.
`fe-log` is still an `emptyDir`, as §2 says it should be. The class was picked up from the
cluster default (`standard`, `rancher.io/local-path`) because the field was omitted — the
absent-vs-empty-string distinction, confirmed live.

PVC identity recorded here for §6 to compare against:

```text
$ kubectl -n egret-engines get pvc fe-meta-femeta-live-fe-0 \
    -o jsonpath='uid={.metadata.uid} volumeName={.spec.volumeName} created={.metadata.creationTimestamp}'
uid=84ea751b-de7a-4dda-b274-d34a171d605d volumeName=pvc-84ea751b-de7a-4dda-b274-d34a171d605d created=2026-09-26T15:38:51Z
```

## 5. Check 2 — THE HEADLINE: metadata survives a pod replacement: **PASS** (OBSERVED)

### 5.1 The baseline put on the engine

`sp4a10_wh` was attached at create by egret itself, so the catalog was already there; a non-root
user, a database and a grant were added on top (22:41:07 +07):

```text
$ mysql ... -e "CREATE USER 'femetaprobe'@'%' IDENTIFIED BY '…';
               CREATE DATABASE femeta_probe_db;
               GRANT SELECT ON ALL TABLES IN DATABASE femeta_probe_db TO 'femetaprobe'@'%';"   # rc=0

$ mysql ... -e 'SHOW CATALOGS; SHOW ALL AUTHENTICATION; SHOW DATABASES; SHOW GRANTS FOR ...'
Catalog	Type	Comment
default_catalog	Internal	An internal catalog contains this cluster's self-managed tables.
sp4a10_wh	Iceberg	NULL
UserIdentity	Password	AuthPlugin	UserForAuthPlugin
'root'@'%'	Yes	MYSQL_NATIVE_PASSWORD	NULL
'egret_delegator'@'%'	Yes	MYSQL_NATIVE_PASSWORD	NULL
'femetaprobe'@'%'	Yes	MYSQL_NATIVE_PASSWORD	NULL
Database
_statistics_
femeta_probe_db
information_schema
sys
UserIdentity	Catalog	Grants
'femetaprobe'@'%'	default_catalog	GRANT SELECT ON ALL TABLES IN DATABASE femeta_probe_db TO USER 'femetaprobe'@'%'

$ kubectl -n egret-engines get pod femeta-live-fe-0 -o jsonpath='uid=… start=… restarts=…'
uid=2cad3ff8-6881-4aa1-9d89-5a09a67d448e start=2026-09-26T15:38:55Z restarts=0
```

**One nuance recorded before the measurement, because it would otherwise look like a failure.** The
*first* boot of a brand-new engine legitimately says the phrase this check hunts for — the PVC
starts empty, so there is no metadata yet:

```text
$ kubectl logs femeta-live-fe-0 -c fe | grep -n 'first start fe with meta not exist'
2:[Sat Sep 26 15:38:59 UTC 2026] first start fe with meta not exist.
```

So the check is **the replacement pod's own log**, not "any log this engine ever wrote".

```text
$ kubectl exec femeta-live-fe-0 -c fe -- sh -c 'ls /opt/starrocks/fe/meta; du -sh …; df -h … | tail -1'
bdb
image
132K	/opt/starrocks/fe/meta
/dev/vda1       458G   85G  350G  20% /opt/starrocks/fe/meta
```

### 5.2 The pod was deleted and NOTHING else was changed

```text
2026-09-26 22:41:29 +0700
$ kubectl -n egret-engines delete pod femeta-live-fe-0
pod "femeta-live-fe-0" deleted from egret-engines namespace
```

That single command is the whole of the step. Proof that nothing else moved the pod — this is the
rule PR #597's gate broke, so it is measured rather than asserted:

```text
$ kubectl -n egret-engines get sts femeta-live-fe -o jsonpath='generation=… observed=… currentRevision=… updateRevision=…'
generation=1 observed=1 currentRevision=femeta-live-fe-9d8c98ff5 updateRevision=femeta-live-fe-9d8c98ff5
$ kubectl -n egret-engines get starrockscluster femeta-live -o jsonpath='generation=…'
generation=1
$ helm list -n egret-system
egret	egret-system	67	2026-09-26 22:36:25 +0700	deployed	egret-0.1.0	dev
```

StatefulSet generation still `1` with `currentRevision == updateRevision` (no pod-template change),
CR generation still `1`, helm still on the revision from §3. The pod rolled because it was deleted,
and for no other reason.

### 5.3 What came back

```text
$ kubectl -n egret-engines get pod femeta-live-fe-0 -o jsonpath='uid=… start=… restarts=…'
uid=7103b049-f507-42ce-be8b-37829374608e start=2026-09-26T15:41:31Z restarts=0
```

A different `uid` from §5.1 and `restarts=0` — a genuinely new pod, not a restarted container (a
container restart would not have exercised the mechanism at all, since an `emptyDir` survives that).

**The three verdicts, verbatim (22:41:57 +07):**

```text
$ mysql ... -e 'SHOW CATALOGS; SHOW ALL AUTHENTICATION; SHOW DATABASES; SHOW GRANTS FOR ...'
Catalog	Type	Comment
default_catalog	Internal	An internal catalog contains this cluster's self-managed tables.
sp4a10_wh	Iceberg	NULL
UserIdentity	Password	AuthPlugin	UserForAuthPlugin
'root'@'%'	Yes	MYSQL_NATIVE_PASSWORD	NULL
'egret_delegator'@'%'	Yes	MYSQL_NATIVE_PASSWORD	NULL
'femetaprobe'@'%'	Yes	MYSQL_NATIVE_PASSWORD	NULL
Database
_statistics_
femeta_probe_db
information_schema
sys
UserIdentity	Catalog	Grants
'femetaprobe'@'%'	default_catalog	GRANT SELECT ON ALL TABLES IN DATABASE femeta_probe_db TO USER 'femetaprobe'@'%'
```

1. **`SHOW CATALOGS` still lists `sp4a10_wh`.** PR #597's gate saw `default_catalog` alone.
2. **`femetaprobe` still exists, with its password and its grant.** #597's gate lost `analyst`
   entirely; here even the grant came back.
3. **`root` still has a password** (`Password: Yes`) — and the query above only succeeded *because*
   it does: the password came from `femeta-live-initpwd-secret` and authenticated. #597's gate found
   `root` reverted to no password. The `initpwd` Job is `Complete` and did **not** re-run.
4. **The replacement pod's log does not contain the failure phrase**, and says the opposite:

```text
$ kubectl logs femeta-live-fe-0 -c fe | grep -c 'first start fe with meta not exist'
0
$ kubectl logs femeta-live-fe-0 -c fe | head -6
[Sat Sep 26 15:41:34 UTC 2026] Process conf file fe.conf ...
[Sat Sep 26 15:41:34 UTC 2026] start fe with exist meta.
[Sat Sep 26 15:41:34 UTC 2026] start with meta run start_fe.sh with additional options: ' --host_type FQDN'
```

`start fe with exist meta.` is the exact inverse of the signature that opened this whole
investigation.

## 6. Check 3 — the PVC was retained and rebound, not recreated: **PASS** (OBSERVED)

The same three fields, before (§4) and after (§5.3) the replacement:

| field | before | after | identical? |
|---|---|---|---|
| `.metadata.uid` | `84ea751b-de7a-4dda-b274-d34a171d605d` | `84ea751b-de7a-4dda-b274-d34a171d605d` | **yes** |
| `.spec.volumeName` | `pvc-84ea751b-de7a-4dda-b274-d34a171d605d` | `pvc-84ea751b-de7a-4dda-b274-d34a171d605d` | **yes** |
| `.metadata.creationTimestamp` | `2026-09-26T15:38:51Z` | `2026-09-26T15:38:51Z` | **yes** |

```text
$ kubectl -n egret-engines get pvc fe-meta-femeta-live-fe-0 -o jsonpath='uid=… volumeName=… created=…'
uid=84ea751b-de7a-4dda-b274-d34a171d605d volumeName=pvc-84ea751b-de7a-4dda-b274-d34a171d605d created=2026-09-26T15:38:51Z

$ kubectl -n egret-engines get pvc
NAME                       STATUS   VOLUME                                     CAPACITY   ACCESS MODES   STORAGECLASS   AGE
fe-meta-femeta-live-fe-0   Bound    pvc-84ea751b-de7a-4dda-b274-d34a171d605d   20Gi       RWO            standard       6m5s
```

`AGE 6m5s` spans the deletion — a new claim would read seconds. One PVC exists, not two, and the
replacement pod mounts it by the same name:

```text
$ kubectl -n egret-engines get pod femeta-live-fe-0 -o jsonpath='{range .spec.volumes[*]}…'
fe-meta	{"name":"fe-meta","persistentVolumeClaim":{"claimName":"fe-meta-femeta-live-fe-0"}}
ssl-keystore	{"hostPath":{"path":"/var/lib/egret/starrocks/femeta-live/ssl-keystore","type":"DirectoryOrCreate"},"name":"ssl-keystore"}
fe-log	{"emptyDir":{},"name":"fe-log"}
femeta-live-fe-cm	{"configMap":{"defaultMode":420,"name":"femeta-live-fe-cm"},"name":"femeta-live-fe-cm"}
```

So the data survived because the *same* volume was rebound, which is the thing a naive "the pod came
back" check cannot distinguish.

## 7. Check 4 — the one-time migration loss: **the prediction is FALSIFIED, and the real behaviour is worse** (OBSERVED)

This section previously said the loss was a prediction. It is now a measurement — and it does not
say what was predicted. **An existing `emptyDir` engine does not migrate at all: the operator cannot
apply the change, and the cluster goes to `phase=failed` and stays there.**

### 7.1 Baseline of `sp3c2-sr-live`, an engine provisioned before the fix

Captured before any write (22:12-22:45 +07). It is the `emptyDir` engine from §0:

```text
$ mysql ... -e 'SHOW CATALOGS;'
default_catalog	Internal	…
sp4a10_wh	Iceberg	NULL
$ mysql ... -e 'SHOW ALL AUTHENTICATION;'
'srgate'@'%'	Yes	MYSQL_NATIVE_PASSWORD	NULL
'root'@'%'	Yes	MYSQL_NATIVE_PASSWORD	NULL
$ mysql ... -e 'SHOW DATABASES;'      → _statistics_, information_schema, sys
$ mysql ... -e "SHOW GRANTS FOR 'srgate'@'%'"   → empty (authz is Ranger's, not StarRocks grants)
$ mysql ... -e 'SHOW ROLES;'          → the six builtins only
$ du -sh /opt/starrocks/fe/meta       → 26M
$ mysql -uroot -e "SELECT 'root-login-ok'"      → root-login-ok   (root's password works)

$ kubectl get pod sp3c2-sr-live-fe-0 -o jsonpath='poduid=… start=… restarts=…'
poduid=fc307d0e-5664-4861-82e9-bce2e36330fd start=2026-09-26T10:02:03Z restarts=33
$ kubectl get sts sp3c2-sr-live-fe  -o jsonpath='stsgen=… currentRevision=…'
stsgen=3 currentRevision=sp3c2-sr-live-fe-f56bbdc67
$ kubectl get starrockscluster sp3c2-sr-live -o jsonpath='crgen=…'
crgen=4
```

### 7.2 Taking it through the upgrade

`PUT /api/control-plane/engines/43bc0bf7-…` re-sending its own stored config unchanged (same images,
replicas, `feConfig`, `pod.env`, `delegationMode: jwt`, `tlsEnabled: true`, `authzServiceId: 55`) —
so the *only* difference in the render is the new code's unconditional `fe-meta` entry. The response
confirms the stored spec keeps `feMetaStorageSize: null` / `feMetaStorageClass: null`, i.e. an engine
whose spec predates the surface takes the default path.

egret's half worked. The CR now carries the PVC entry (`crgen` 4 → 5):

```text
$ kubectl -n egret-engines get starrockscluster sp3c2-sr-live -o jsonpath='{.spec.starRocksFeSpec.storageVolumes}'
[
    {"mountPath": "/opt/starrocks/fe/meta", "name": "fe-meta", "storageSize": "20Gi"},
    {"hostPath": {"path": "/var/lib/egret/starrocks/sp3c2-sr-live/ssl-keystore", "type": "DirectoryOrCreate"},
     "mountPath": "/etc/starrocks/ssl", "name": "ssl-keystore"}
]
```

### 7.3 What the operator did with it — verbatim

Nothing, forever. Every reconcile of this cluster now fails, retried roughly every 25s:

```text
$ kubectl -n starrocks logs kube-starrocks-operator-66f79d9767-2m54n --since=8m | grep sp3c2
ERROR StarRocksClusterReconciler.feController deploy statefulset failed
  {"name": "sp3c2-sr-live", "namespace": "egret-engines", "action": "SyncCluster",
   "error": "failed to apply merge patch: StatefulSet.apps \"sp3c2-sr-live-fe\" is invalid:
     spec: Forbidden: updates to statefulset spec for fields other than 'replicas', 'ordinals',
     'template', 'updateStrategy', 'revisionHistoryLimit', 'persistentVolumeClaimRetentionPolicy'
     and 'minReadySeconds' are forbidden"}
ERROR StarRocksClusterReconciler sub controller reconciles spec failed
  {"name": "sp3c2-sr-live", "subController": "feController", "error": "failed to apply merge patch: …"}
ERROR Reconciler error {"controller": "starrockscluster", … "error": "failed to apply merge patch: …"}
```

The k8s event, `Count: 24` and climbing, says the same: `Reason:"SyncFeFailed"`.

**Why.** A `storageVolumes` entry with `storageSize` becomes a StatefulSet
`spec.volumeClaimTemplates` entry, and `volumeClaimTemplates` is **immutable** on an existing
StatefulSet. Adding one to a StatefulSet that has none is not a permitted update, so the operator's
merge patch is rejected by the API server, not by the operator's own logic. This has nothing to do
with how egret renders the CR — egret rendered exactly what §4 proves correct — and everything to do
with the fact that existing engines already have a StatefulSet without that claim template.

### 7.4 What was actually lost: nothing, because nothing happened

Five minutes after the update, and after the operator had retried 24 times:

```text
$ kubectl -n egret-engines get pod sp3c2-sr-live-fe-0 -o jsonpath='poduid=… start=… restarts=…'
poduid=fc307d0e-5664-4861-82e9-bce2e36330fd start=2026-09-26T10:02:03Z restarts=33
        # byte-identical to §7.1 — the pod was never replaced

$ kubectl -n egret-engines get pod sp3c2-sr-live-fe-0 -o jsonpath='…volumes…' | grep fe-meta
fe-meta	{"emptyDir":{},"name":"fe-meta"}
        # STILL the operator's emptyDir

$ kubectl -n egret-engines get sts sp3c2-sr-live-fe -o jsonpath='{.spec.volumeClaimTemplates}'
        # empty — no claim template was ever added

$ kubectl -n egret-engines get pvc
NAME                       STATUS   VOLUME                                  CAPACITY   STORAGECLASS   AGE
fe-meta-femeta-live-fe-0   Bound    pvc-84ea751b-…                          20Gi       standard       14m
        # only the NEW engine's claim; sp3c2-sr-live has none

$ kubectl -n egret-engines get starrockscluster sp3c2-sr-live -o jsonpath='phase=… fe=…'
phase=failed fe=running

$ mysql ... -e 'SHOW CATALOGS; SHOW ALL AUTHENTICATION;'
default_catalog	Internal	…
sp4a10_wh	Iceberg	NULL
'srgate'@'%'	Yes	MYSQL_NATIVE_PASSWORD	NULL
'root'@'%'	Yes	MYSQL_NATIVE_PASSWORD	NULL
```

So the loss inventory in `deploy/helm/egret/README.md` **could not be measured, because the event it
describes never occurs on this path.** It stays labelled a prediction. What replaces it as an
observation is worse and belongs in that README instead:

| the README currently predicts | measured |
|---|---|
| "each existing StarRocks engine loses its FE metadata exactly once, on its first FE roll after the upgrade" | The engine does **not** roll and does **not** get the PVC. Its FE keeps the `emptyDir`, so it is still as fragile as before the fix. |
| "From then on the metadata survives pod replacement." | Not reached. For an existing engine nothing changes, except that its `StarRocksCluster` goes to `phase=failed` and the operator retries a rejected patch indefinitely — so **no further CR change to that engine reconciles either**, fix-related or not. |
| "the `root` password — the initpwd Job … does **not** re-run by itself" | **Wrong for an egret-driven update.** The PUT re-templated the Job and a fresh `sp3c2-sr-live-initpwd-89zmr` ran to `Completed` within 10s. (The claim may still hold for a bare `kubectl delete pod`, which does not go through egret at all.) |

**One further caveat, stated rather than left implied:** §5's PASS is for an engine **created** by
this code. The fix is proven for new engines. For engines that already exist, this branch as it
stands does not deliver persistence and leaves their operator reconcile wedged — that needs a
follow-up (recreate the FE StatefulSet, e.g. `kubectl delete sts <name>-fe --cascade=orphan` so the
operator rebuilds it with the claim template, which is also the moment the metadata is lost), and it
needs to be in the README as a required operator step, not as a passive warning.

### 7.5 What was deliberately NOT done

The remedy above — deleting `sp3c2-sr-live-fe`'s StatefulSet so the operator rebuilds it with a
claim template — **was not run.** It is a manual destructive step outside the upgrade path this
check was asked to measure, and executing it would have destroyed that engine's FE metadata
(catalogs, `srgate`, the root password) as a side effect of measuring. The loss inventory therefore
remains a prediction, as the task instructed when the measurement cannot be taken cleanly.

The cluster is left with `sp3c2-sr-live` in `phase=failed` and the operator retrying. That state is
the finding, not damage from the measurement: reverting it is impossible through egret, since this
branch renders the `fe-meta` entry unconditionally on every provision.

## 8. Confirmed untouched

```text
$ kubectl -n egret-engines get pods | grep demo-trino
demo-trino-coordinator-c54f46974-xwgbt   1/1   Running   59 (50m ago)   6d1h
```

`demo-trino` was neither used nor modified. `sp3c-live-trino` / `sp3c2-trino-live` were left at the
0 replicas they were found at.

## 9. Scoreboard

| check | verdict |
|---|---|
| 1. a new engine renders a PVC, bound, with the TLS entry alongside | **PASS** (§4) |
| 2. metadata survives a pod replacement, nothing else changed | **PASS** (§5) |
| 3. the PVC is retained and rebound, same uid/volumeName/creationTimestamp | **PASS** (§6) |
| 4. the one-time migration loss, measured | **the prediction is FALSIFIED** — the migration does not happen at all and the operator wedges (§7). The loss inventory itself stays a prediction. |

The headline is proven for new engines. The existing-engine path is broken in a way the unit tests
could not see, and the README's upgrade note is wrong about it.
