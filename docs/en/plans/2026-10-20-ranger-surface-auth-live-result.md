# Ranger surface authentication: live result on kind

Task 9 Steps 1 and 4 of [the plan](2026-10-20-ranger-surface-auth.md). Run on 2026-10-20 (UTC) against branch
`feat/ranger-surface-auth-b` at `254684b4`, on a throwaway kind cluster with egret's own Postgres. Real
Trino 483 (Ranger plugin 2.8.0) and StarRocks 4.1.6 (plugin 2.9.0) engines were provisioned by egret.

Every block below is copied from the terminal. Passwords are never printed: where a password would
appear, only its length and first two characters are shown, as `5T…`. Engine and credential ids are not
secrets and are shown in full.

## Summary

| # | Scenario | Verdict |
|---|---|---|
| 1 | Both engines poll `/ranger/service/plugins/secure/policies/download/<profile>` with Basic; a policy change reaches each | **PASS** |
| 2 | 401 / 401 / 403 / 404 / roles 401 then 404 | **PASS** |
| 3 | Password only in `{name}-rc-{tag}`; volume names; operator accepts the Secret; pod annotation | **PASS** for the Kubernetes objects. **FAIL** on "nowhere else": the Trino plugin writes the password into the pod log (finding 1) |
| 4 | egret down, engine pod restarted: does it serve from a policy cache? | **Risk confirmed.** The cache does not survive a pod restart; a restarted StarRocks FE denies everything until egret is back. A JWT Trino coordinator does not start at all without egret |
| 5 | Rotation of a provisioned engine: 204, roll, polls continue, retire after rollout, old 401, new 200, second rotate 409 | **PASS** (Trino and StarRocks) |
| 6 | External engine: one-time credential, rotate, stale retire 409, retire 204, old 401; `engine_auth_failing` raised and cleared | **PASS** |
| 7 | Browser: one-time credential dialog and the Ranger credential section | **PASS** (5 screenshots, kept out of git) |
| 8 | Surprises | 4 items, see [Surprises](#surprises) |

## Findings requiring action

Ranked by impact.

1. **The Trino Ranger plugin logs the engine's Ranger password in plain text.** On every start, Ranger's
   `AuditProviderFactory` logs each `ranger.plugin.trino.*` property at INFO, including
   `policy.rest.client.password`. The line is in the stdout of every Trino coordinator and worker, so it
   is readable with `kubectl logs` and through egret's own
   `GET /api/control-plane/engines/{id}/pods/{pod}/logs`, which requires only `engine:read`
   (`crates/egret-engine-mgmt/src/engines.rs`, `pod_logs`). A user with `engine:read` can therefore read
   the credential and download that profile's policies, and can also raise or clear the engine's
   `engine_auth_failing` item. StarRocks is not affected: the password is in none of the FE log files
   and not in the FE stdout. Evidence: [scenario 3](#3-where-the-password-lives) and
   [scenario 8](#8-secret-sweep). Possible fix (not made here): set
   `org.apache.ranger.audit.provider.AuditProviderFactory=WARN` in the Trino `log.properties` that egret
   renders, and add a provisioner test asserting it. A credential that has already been logged needs a
   rotation once the fix ships.
2. **The policy cache does not survive a pod restart, and a cold plugin with egret unreachable denies
   everything** (the open availability risk). Measured on StarRocks: after the FE pod was deleted while
   egret was scaled to 0, `/tmp/ranger/live-sr/policycache/` held only the roles file and the plugin logged
   `cache file does not exist or not readable '/tmp/ranger/live-sr/policycache/starrocks_live-sr.json'`.
   A native user covered by the `egret-admin` policies got
   `Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN v` and `SHOW DATABASES`
   returned no rows. Access came back 16 s after egret did. Any reschedule, node drain, OOM kill or
   rotation roll that happens while egret is down therefore takes the engine to deny-all. A Trino engine
   with JWT authentication (the default) is worse off: its coordinator does not start without egret's
   JWKS (`Error reading JWK keys from http://egret.egret-system.svc.cluster.local:8080/.well-known/jwks.json`),
   so the cache question does not arise for it. Options for the user to decide: mount an `emptyDir`
   (survives container restarts but not pod deletion) or a small PVC at the cache directory, or accept
   and document the risk. The guide's "Not measured yet" bullet can now be replaced with this result.
3. **The Helm ingress does not route `/ranger`, and the guide's check `curl` then gives a false 200.**
   The chart's ingress routes `/auth`, `/data-api`, `/oauth`, `/.well-known/*` and `/catalog` to egret,
   and everything else to the frontend. `http://egret.localtest.me:28080/ranger/service/plugins/secure/policies/download/live-trino`
   with no credentials returned `HTTP 200 content-type=text/html` (the SPA). An external engine that
   follows the guide and sets `policy.rest.url` to `https://<egret host>/ranger` through this ingress
   reaches the frontend, not egret. The guide's check command ("`200` means the credential and the
   binding are right") then reports success for any credential. In-cluster engines are unaffected:
   they use `http://egret.egret-system.svc.cluster.local:8080/ranger`. The old root `/service` path was
   not routed either, so this is not a regression of this branch, but the new guide promises the
   external URL. Fix: add `/ranger` to `deploy/helm/egret/templates/ingress.yaml`, and make the guide's
   check also test the content type or the body.
4. **A StarRocks engine created without `authzServiceId` is left unbound, so it gets no Ranger credential.**
   Creating `live-sr` without `authzServiceId` returned 201 with `rangerCredentialId: null`, and its
   namespace had no `-rc-` Secret and no `-ranger-cm`. Trino auto-binds a new profile named after the
   engine, but `create_starrocks_engine` only binds a profile given by id. The late bind in the apply
   path calls `plan_trino_profile` and did not bind it either. This behaviour predates this branch, but
   it means a StarRocks engine is unprotected by default while Ranger is on. Record it for the
   follow-up list. In this run the engine was deleted and recreated with `authzServiceId: 3`.

## Addressed after the run

Added on 2026-10-21. The results above are unchanged; none of these fixes has been checked on a
cluster yet.

- **Finding 1, fixed.** `f6adf09a` renders
  `org.apache.ranger.audit.provider.AuditProviderFactory=WARN` as the last line of every bound Trino
  engine's `log.properties` (a user entry for that logger is dropped; the user's other lines are
  kept). `log.properties` is in the pod-template checksum, so bound engines roll on their next apply.
  `5c8be53a` also masks `…password=`, `…secret=` style values in egret's pod-logs response and its
  follow stream. A credential that was already logged still needs a rotation; the guide's upgrade
  notes say so.
- **Finding 3, fixed.** `44a8fb2f` adds a `/ranger` Prefix rule to the egret backend in the Helm
  ingress, and a `location /ranger/` to `frontend/nginx-prod.conf`. The guide's check now reads the
  content type and the body, and names `200 text/html` as "not egret".
- **Finding 2, open.** The guide now records the measured behaviour in place of "Not measured yet".
  Whether to mount an `emptyDir` or a PVC at the cache directory, or to accept the risk, is the
  user's decision.
- **Finding 4, open.** A StarRocks engine created without `authzServiceId` is still left unbound.

## Environment

| Item | Value |
|---|---|
| Branch / commit | `feat/ranger-surface-auth-b` at `254684b4` |
| kind | v0.29.0, node `kindest/node:v1.33.1@sha256:050072256b9a903bd914c0b2866828150cb229cea0efe5892e2b644d5dd3b34f` |
| Cluster | `egret-ranger-live`, single node, host ports 28080 (ingress :80) and 28081 (:30080), bound to 127.0.0.1 |
| Docker | 28.4.0, 8 CPUs, 23.5 GiB |
| kubectl / helm | v1.34.1 / v3.18.6 |
| ingress-nginx | `registry.k8s.io/ingress-nginx/controller:v1.15.1` (the kind manifest from the `ingress-install` recipe) |
| StarRocks operator | `starrocks/operator:v1.11.6` from `deploy/starrocks-operator` (`--volume-name-with-hash=true`) |
| egret images | `egret-backend:live9c`, `egret-frontend:live9c`, built from the worktree |
| Engines | `trinodb/trino:483@sha256:db58cc93…` (plugin 2.8.0), `starrocks/fe-ubuntu:4.1.6@sha256:d07aa750…` + `cn-ubuntu:4.1.6` (plugin 2.9.0) |
| egret config | Helm `oidc.enabled=true`, `ingress.enabled=true`, `ranger.enabled=true`, `ranger.authMode` left at its default `required` |

### Commands to reproduce

```bash
# cluster: the repo's deploy/k8s/kind.yaml with hostPorts moved to 28080/28081
kind create cluster --name egret-ranger-live --config live9c-kind.yaml
kubectl apply -f https://raw.githubusercontent.com/kubernetes/ingress-nginx/main/deploy/static/provider/kind/deploy.yaml
kubectl create namespace egret-engines
kubectl apply --server-side -f deploy/starrocks-operator/crd.yaml
kubectl apply -f deploy/starrocks-operator/operator.yaml
./deploy/k8s/coredns-rewrite.sh

# images: the backend Dockerfile with its target cache mount given its own id (see Surprises, item 1)
docker build -f live9c-backend.Dockerfile -t egret-backend:live9c .
docker build -f frontend/Dockerfile -t egret-frontend:live9c frontend
kind load docker-image egret-backend:live9c egret-frontend:live9c --name egret-ranger-live
# engine images: `kind load` failed on the multi-platform images (Surprises, item 2), so pulled in the node
docker exec egret-ranger-live-control-plane crictl pull docker.io/trinodb/trino:483   # and fe/cn-ubuntu:4.1.6

# egret: deploy-kind-governance's values, plus the image tag and no renderer
helm upgrade --install egret deploy/helm/egret -n egret-system --create-namespace --wait --timeout 15m \
  --set oidc.enabled=true --set ingress.enabled=true --set ranger.enabled=true \
  --set backend.image.tag=live9c --set frontend.image.tag=live9c --set renderer.replicas=0
```

Setup through the API, in order: login as `admin@egret.local`; `POST /api/control-plane/storage`
(RustFS in the cluster, bucket `warehouse`); `POST /api/control-plane/warehouses` (registry catalog
`live_wh`, iceberg, id `4bf6329d-79d0-41ae-8645-ec87f68161ff`); `POST /api/control-plane/engines` for
`live-trino` (`c4c38045-5b82-4b48-9824-ab6273086c54`, 1 coordinator and 1 worker) and, after
`POST /api/control-plane/policies/services {"name":"live-sr","type":"starrocks"}` (id 3), for `live-sr`
with `authzServiceId: 3` (`d7f7d3c3-9d5a-470a-a7b9-945670f16c86`, 1 FE, 1 CN, 4Gi). The Trino engine
was created with `memory: 1Gi`, which the coordinator does not survive (Surprises, item 3); one `PUT`
raised it to 2Gi.

The download URL was reached through `kubectl port-forward svc/egret 28090:8080`, because the ingress
does not route `/ranger` (finding 3).

## 1. Both engines poll with Basic, and a policy change reaches each

Profiles and bindings after both engines were up:

```text
{"id":3,"name":"live-sr","type":"starrocks","tagService":null,"engines":["live-sr"]}
{"id":2,"name":"live-trino","type":"trino","tagService":null,"engines":["live-trino"]}
{"id":1,"name":"live-trino_tag","type":"tag","tagService":null,"engines":[]}
```

`policy.ranger_plugin_downloads` at 21:04:30 (egret is in `required` mode, so every row is an
authenticated download):

```text
 service_id |    name    |                        plugin_id                         |   kind   | last_known_version | served_version |              at
------------+------------+----------------------------------------------------------+----------+--------------------+----------------+-------------------------------
          2 | live-trino | trino@live-trino-worker-5bc95b69ff-vwv2q-live-trino      | policies |                 19 |             35 | 2026-10-20 21:03:16.736563+00
          2 | live-trino | trino@live-trino-coordinator-787b66c94f-trnzf-live-trino | policies |                 35 |             35 | 2026-10-20 21:04:02.51083+00
          3 | live-sr    | starrocks@live-sr-fe-0-live-sr                           | policies |                 16 |             16 | 2026-10-20 21:04:27.791366+00
          2 | live-trino | trino@live-trino-worker-698fff79cf-4zfxh-live-trino      | policies |                 35 |             35 | 2026-10-20 21:04:28.464983+00
```

The engines' own logs. Trino coordinator (the password line is redacted here; see finding 1):

```text
INFO main org.apache.ranger.audit.provider.AuditProviderFactory AUDIT PROPERTY: ranger.plugin.trino.policy.rest.client.username=engine-c4c38045-5b82-4b48-9824-ab6273086c54
INFO main org.apache.ranger.audit.provider.AuditProviderFactory AUDIT PROPERTY: ranger.plugin.trino.policy.rest.url=http://egret.egret-system.svc.cluster.local:8080/ranger
INFO main org.apache.ranger.audit.provider.AuditProviderFactory AUDIT PROPERTY: ranger.plugin.trino.policy.rest.client.password=5T…(redacted)
INFO main org.apache.ranger.plugin.util.PolicyRefresher PolicyRefresher(serviceName=live-trino): found updated version. lastKnownVersion=-1; newVersion=35
```

StarRocks FE `fe.log` (`secureMode=true` is the plugin choosing the `/secure/` path because a username
and password are set; the roles 404 is expected and ignored, as the measurements predicted):

```text
2026-10-20 21:03:57.009Z ERROR (main|1) [RangerAdminRESTClient.getRolesIfUpdated():254] Error getting Roles; service not found. secureMode=true, response=404, serviceName=live-sr, lastKnownRoleVersion=-1, lastActivationTimeInMillis=0
2026-10-20 21:03:57.013Z WARN (main|1) [RangerAdminRESTClient.getRolesIfUpdated():262] Received 404 error code with body:[null], Ignoring
2026-10-20 21:03:57.612Z INFO (main|1) [PolicyRefresher.loadPolicyfromPolicyAdmin():298] PolicyRefresher(serviceName=live-sr): found updated version. lastKnownVersion=-1; newVersion=16
2026-10-20 21:03:57.779Z INFO (main|1) [RangerAccessController.<init>():54] Start Ranger plugin (starrocks - live-sr) success
```

### A policy change, observed as a query result

StarRocks: an Iceberg table `live_wh.live9c.t` (2 rows) created as `root`; a native StarRocks user
`live9cprobe` and an egret user with the username `live9cprobe`.

```text
21:07:30  probe: ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN v for this operation.
21:07:59  POST /api/control-plane/perm/grants {"effect":"allow","actions":["read"],"scope":{"catalog_id":"4bf6…","schema":"live9c"},"principals":{"include":[{"kind":"user","ref":"live9cprobe"}],"exclude":[]}}
          {"id":"fe3c3a38-f8f5-4855-a917-12619acfea73","compile":{"services":[2,3],"created":2,"updated":0,"deleted":0,"unchanged":2,"skipped":[]}}  201
21:08:08  probe: ERROR 5204 (42000) ... Access denied ...
21:08:13  probe allowed:
id	v
1	a
2	b
```

```text
 live-sr    | starrocks@live-sr-fe-0-live-sr                           |   16 |   17 | 2026-10-20 21:08:13.462799+00
 live-trino | trino@live-trino-coordinator-787b66c94f-trnzf-live-trino |   35 |   36 | 2026-10-20 21:08:02.519737+00
2026-10-20 21:08:13.487Z INFO (PolicyRefresher(serviceName=live-sr)-271|271) [PolicyRefresher.loadPolicyfromPolicyAdmin():298] PolicyRefresher(serviceName=live-sr): found updated version. lastKnownVer…
2026-10-20T21:08:02.555Z INFO PolicyRefresher(serviceName=live-trino)-144 ... found updated version. lastKnownVersion=35; newVersion=36
```

Then a deny grant on `live9c.t` for `live9cprobe` (21:08:23): still allowed at 21:08:24, :29, :34, :39;
`21:08:45 probe denied: ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN v`.
Ledger: `live-sr ... 17 | 18 | 21:08:43`, Trino coordinator `36 | 37 | 21:08:32`, worker `35 | 37 | 21:08:28`.

Trino, through egret's SQL Lab as `admin` (egret mints the engine JWT): `SELECT id, v, current_user AS u FROM live_wh.live9c.t ORDER BY id`
returned `[[1,"a","admin"],[2,"b","admin"]]`. After a deny grant for `admin` on `live9c.t` at 21:09:34:

```text
21:09:35 state=finished error=   [[1,"a"],[2,"b"]]
21:09:42 state=finished error=   [[1,"a"],[2,"b"]]
...
21:10:01 state=finished error=   [[1,"a"],[2,"b"]]
21:10:06 state=failed error=engine: Access Denied: Cannot select from columns [v, id] in table or view t
```

The grant was then deleted. **Verdict: PASS** for both engines (a 30 s poll interval; changes took
effect in 14 to 32 s).

## 2. The download URL's answers

`/private/…/live9c-s2.sh`, credentials read from the engines' Secrets inside the script. Output as printed:

```text
no credentials, /ranger/service/plugins/secure/policies/download/live-trino => HTTP 401 | www-authenticate: Basic realm="egret-ranger" | body:
wrong password (engine-c4c38045-5b82-4b48-9824-ab6273086c54:wrong), /ranger/service/plugins/secure/policies/download/live-trino => HTTP 401 | www-authenticate: Basic realm="egret-ranger" | body:
unknown engine (engine-00000000-0000-0000-0000-000000000000), /ranger/service/plugins/secure/policies/download/live-trino => HTTP 401 | www-authenticate: Basic realm="egret-ranger" | body:
trino credential, /ranger/service/plugins/secure/policies/download/live-trino => HTTP 200 | www-authenticate:  | body: {"serviceName":"live-trino","serviceId":2,"policyVersion":39,"serviceDef":{"id":203,…
starrocks credential, /ranger/service/plugins/secure/policies/download/live-sr => HTTP 200 | www-authenticate:  | body: {"serviceName":"live-sr","serviceId":3,"policyVersion":20,"serviceDef":{"id":205,…
trino credential for starrocks profile, /ranger/service/plugins/secure/policies/download/live-sr => HTTP 403 | www-authenticate:  | body: {"msg":"forbidden","statusCode":403}
starrocks credential for trino profile, /ranger/service/plugins/secure/policies/download/live-trino => HTTP 403 | www-authenticate:  | body: {"msg":"forbidden","statusCode":403}
trino credential, plain path /ranger/service/plugins/policies/download/live-trino => HTTP 200 | www-authenticate:  | body: {"serviceName":"live-trino","serviceId":2,"policyVersion":39,…
trino credential, tags /ranger/service/tags/download/live-trino_tag => HTTP 200 | www-authenticate:  | body: {"op":"add","serviceName":"live-trino_tag","tagVersion":1,"tags":{},"serviceResources":[],"resourceToTagIds":{}}
trino credential, nonexistent profile /ranger/service/plugins/secure/policies/download/nope => HTTP 404 | www-authenticate:  | body: {"msg":"service nope","statusCode":404}
root /service/plugins/policies/download/live-trino, no creds => HTTP 404 | www-authenticate:  | body:
root /service/plugins/policies/download/live-trino, valid trino creds => HTTP 404 | www-authenticate:  | body:
roles /ranger/service/roles/secure/download/live-trino, no creds => HTTP 401 | www-authenticate: Basic realm="egret-ranger" | body:
roles /ranger/service/roles/secure/download/live-trino, valid trino creds => HTTP 404 | www-authenticate:  | body:
roles /ranger/service/roles/secure/download/live-sr, valid sr creds => HTTP 404 | www-authenticate:  | body:
operator route /ranger/service/plugins/services, trino creds => HTTP 403 | www-authenticate:  | body:
operator route /ranger/service/plugins/services, empty bearer => HTTP 401 | www-authenticate: Basic realm="egret-ranger" | body:
operator route /ranger/service/plugins/services, no creds => HTTP 401 | www-authenticate: Basic realm="egret-ranger" | body:
```

egret's log for the same run (ANSI colour removed):

```text
2026-10-20T21:10:44.689877Z  WARN egret_policy::auth_failures: ranger credential rejected engine_id=c4c38045-5b82-4b48-9824-ab6273086c54 rejected=1
2026-10-20T21:10:44.725444Z DEBUG egret_policy::auth_failures: ranger credential rejected for no engine engine_id=00000000-0000-0000-0000-000000000000
2026-10-20T21:10:44.851341Z  WARN egret_policy::ranger::api::download: engine credential asked for a service it is not bound to engine_id=c4c38045-5b82-4b48-9824-ab6273086c54 requested=live-sr
2026-10-20T21:10:44.891316Z  WARN egret_policy::ranger::api::download: engine credential asked for a service it is not bound to engine_id=d7f7d3c3-9d5a-470a-a7b9-945670f16c86 requested=live-trino
```

**Verdict: PASS.** Two observations: an engine credential gets 404, not 403, for a profile name that
does not exist, so a holder of any engine credential can test which profile names exist (spec A5
records this oracle); and the wrong-password probe raised `engine_auth_failing` for `live-trino`, which
the engine's next real poll cleared 18 s later (see scenario 6).

## 3. Where the password lives

```text
KIND        NAME                     LABELS                                                                                              ANN
Secret      live-sr-initpwd-secret   map[app.kubernetes.io/managed-by:egret egret/engine:live-sr]                                        <none>
Secret      live-sr-rc-0516ad        map[app.kubernetes.io/managed-by:egret egret/component:ranger-credential egret/engine:live-sr]      0516ade4-4fd3-4bbc-b690-73f1d6d598f0
Secret      live-sr-ssl-secret       map[app.kubernetes.io/managed-by:egret egret/engine:live-sr]                                        <none>
Secret      live-trino-env           map[app.kubernetes.io/managed-by:egret egret/engine:live-trino]                                     <none>
Secret      live-trino-rc-8f99e7     map[app.kubernetes.io/managed-by:egret egret/component:ranger-credential egret/engine:live-trino]   8f99e754-222d-4d71-bbb3-9d3af28ee1d7
Secret      live-trino-tls           map[app.kubernetes.io/managed-by:egret egret/engine:live-trino]                                     <none>
ConfigMap   kube-root-ca.crt         <none>                                                                                              <none>
ConfigMap   live-sr-fe-cm            map[app.kubernetes.io/managed-by:egret egret/engine:live-sr]                                        <none>
ConfigMap   live-sr-initpwd-shell    map[app.kubernetes.io/managed-by:egret egret/engine:live-sr]                                        <none>
ConfigMap   live-sr-ranger-cm        map[app.kubernetes.io/managed-by:egret egret/engine:live-sr]                                        <none>
ConfigMap   live-sr-ssl-cm           map[app.kubernetes.io/managed-by:egret egret/engine:live-sr]                                        <none>
ConfigMap   live-trino-catalogs      map[app.kubernetes.io/managed-by:egret egret/engine:live-trino]                                     <none>
ConfigMap   live-trino-config        map[app.kubernetes.io/managed-by:egret egret/engine:live-trino]                                     <none>
```

No `{name}-ranger-secret` exists (`kubectl get secret -n egret-engines | grep -c ranger-secret` → `0`).

A script (`live9c-pwcheck.sh`) extracted each password from its Secret into a shell variable and
counted exact matches everywhere else, printing only counts:

```text
secret=live-trino-rc-8f99e7 keys=["ranger-trino-security.xml"] xml_len=1162 pw_len=43 pw_first2=5T
  hits in all ConfigMaps yaml:        0
  hits in all Secrets yaml (raw b64): 0
  decoded hit in secret/live-trino-rc-8f99e7: 1
  hits in pods yaml (env/spec):       0
  hits in deploy/sts yaml:            0
  hits in starrocksclusters yaml:     0
  hits in logs pod/live-sr-cn-0/cn: 0
  hits in logs pod/live-sr-fe-0/fe: 0
  hits in logs pod/live-sr-initpwd-dtb98/initpwd: 0
  hits in logs pod/live-trino-coordinator-787b66c94f-trnzf/trino: 1
  hits in logs pod/live-trino-worker-698fff79cf-4zfxh/trino: 1
  egret-system logs checked
secret=live-sr-rc-0516ad keys=["ranger-starrocks-security.xml"] xml_len=1192 pw_len=43 pw_first2=cx
  hits in all ConfigMaps yaml:        0
  hits in all Secrets yaml (raw b64): 0
  decoded hit in secret/live-sr-rc-0516ad: 1
  hits in pods yaml (env/spec):       0
  hits in deploy/sts yaml:            0
  hits in starrocksclusters yaml:     0
  hits in logs pod/live-sr-cn-0/cn: 0
  hits in logs pod/live-sr-fe-0/fe: 0
  hits in logs pod/live-sr-initpwd-dtb98/initpwd: 0
  hits in logs pod/live-trino-coordinator-787b66c94f-trnzf/trino: 0
  hits in logs pod/live-trino-worker-698fff79cf-4zfxh/trino: 0
  egret-system logs checked
```

(egret-system lines are printed only on a hit; none were.) The two Trino log hits are the
`AUDIT PROPERTY: …password=` line (finding 1). The StarRocks FE log files were searched separately:
`grep -c -r -F` over `/opt/starrocks/fe/log/` gave 0 for every file; in `/opt/starrocks/fe/conf/` the
password is only in `ranger-starrocks-security.xml`, the mounted Secret.

StarRocks: the operator accepted the per-credential Secret in `starRocksFeSpec.secrets` with `subPath`
and mounts it:

```text
== SR CR feSpec.secrets/configMaps
{"secrets":[{"mountPath":"/opt/starrocks/fe/conf/ranger-starrocks-security.xml","name":"live-sr-rc-0516ad","subPath":"ranger-starrocks-security.xml"}],"configMaps":[{"mountPath":"/opt/starrocks/fe/conf/ranger-starrocks-audit.xml","name":"live-sr-ranger-cm","subPath":"ranger-starrocks-audit.xml"}],"annotations":{"egret/config-checksum":"6f23e2642c94efe2","egret/ranger-credential":"0516ade4-4fd3-4bbc-b690-73f1d6d598f0"}}
== SR FE sts pod template annotation + volumes
"0516ade4-4fd3-4bbc-b690-73f1d6d598f0"
[{"name":"live-sr-fe-cm","len":13,"secret":null,"cm":"live-sr-fe-cm"},{"name":"live-sr-ranger-cm-1204","len":22,"secret":null,"cm":"live-sr-ranger-cm"},{"name":"live-sr-rc-0516ad-4057","len":22,"secret":"live-sr-rc-0516ad","cm":null}]
[{"mountPath":"/opt/starrocks/fe/conf/ranger-starrocks-audit.xml","name":"live-sr-ranger-cm-1204","subPath":"ranger-starrocks-audit.xml"},{"mountPath":"/opt/starrocks/fe/conf/ranger-starrocks-security.xml","name":"live-sr-rc-0516ad-4057","subPath":"ranger-starrocks-security.xml"}]
== FE pod
"0516ade4-4fd3-4bbc-b690-73f1d6d598f0"
1193
<name>ranger.plugin.starrocks.service.name</name>
<name>ranger.plugin.starrocks.policy.rest.url</name>
<name>ranger.plugin.starrocks.policy.source.impl</name>
<name>ranger.plugin.starrocks.policy.cache.dir</name>
<name>ranger.plugin.starrocks.policy.pollIntervalMs</name>
<name>ranger.plugin.starrocks.policy.rest.client.connection.timeoutMs</name>
<name>ranger.plugin.starrocks.policy.rest.client.read.timeoutMs</name>
<name>ranger.plugin.starrocks.policy.rest.client.username</name>
<name>ranger.plugin.starrocks.policy.rest.client.password</name>
<property><name>ranger.plugin.starrocks.policy.rest.url</name><value>http://egret.egret-system.svc.cluster.local:8080/ranger</value></property>
<property><name>ranger.plugin.starrocks.policy.rest.client.username</name><value>engine-d7f7d3c3-9d5a-470a-a7b9-945670f16c86</value></property>
<property><name>ranger.plugin.starrocks.policy.rest.client.password</name><value>cx…REDACTED</value></property>
```

The operator names the volume `{secret}-{4-char hash}`: `live-sr-rc-0516ad-4057`, 22 characters for a
7-character engine name. The suffix is 15 characters, so the 48-character name bound gives at most
63, as designed.

Trino, coordinator and worker Deployments:

```text
== live-trino-coordinator
"8f99e754-222d-4d71-bbb3-9d3af28ee1d7"
[{"name":"config",…,"cm":"live-trino-config"},{"name":"catalogs",…},{"name":"data",…},{"name":"ranger-secret","len":13,"secret":"live-trino-rc-8f99e7","cm":null},{"name":"tls","len":3,"secret":"live-trino-tls","cm":null}]
[{"mountPath":"/etc/trino/ranger-trino-security.xml","name":"ranger-secret","subPath":"ranger-trino-security.xml"}]
[]
== live-trino-worker
"8f99e754-222d-4d71-bbb3-9d3af28ee1d7"
[… {"name":"ranger-secret","len":13,"secret":"live-trino-rc-8f99e7","cm":null}]
[{"mountPath":"/etc/trino/ranger-trino-security.xml","name":"ranger-secret","subPath":"ranger-trino-security.xml"}]
[]
1163
-rw-r--r-- 1 root  root  1163 Oct 20 21:02 ranger-trino-security.xml
```

Pod annotations equal the credential id:

```text
live-sr-cn-0 -
live-sr-fe-0 0516ade4-4fd3-4bbc-b690-73f1d6d598f0
live-trino-coordinator-787b66c94f-trnzf 8f99e754-222d-4d71-bbb3-9d3af28ee1d7
live-trino-worker-698fff79cf-4zfxh 8f99e754-222d-4d71-bbb3-9d3af28ee1d7
```

The same check after rotation (`live-trino-rc-a64f37`, `live-sr-rc-456611`) gave the same result: 0 in
every ConfigMap, pod, Deployment, StatefulSet and `StarRocksCluster`, and 1 in each Trino pod's log.

**Verdict:** PASS for the objects (Secret only, per-credential name, ≤ 63-character volume names,
operator accepted and mounted, annotation equal to the credential id). FAIL on "nowhere else" because of
the Trino log line (finding 1).

## 4. egret stopped, engine pod restarted

Cache directory in the running pods before the test:

```text
# live-sr-fe-0
-rw-r--r-- 1 root root 20930 Oct 20 21:18 starrocks_live-sr.json
-rw-r--r-- 1 root root    89 Oct 20 21:13 starrocks_live-sr_roles.json
overlay         458G   81G  354G  19% /
# live-trino coordinator
-rw-r--r-- 1 trino trino 41108 Oct 20 21:12 trino_live-trino.json
-rw-r--r-- 1 trino trino    92 Oct 20 21:12 trino_live-trino_roles.json
```

`/tmp` is the container's overlay root; no volume is mounted at `/tmp/ranger`. The FE pod's mounts are
`fe-meta`, `ssl-keystore`, `fe-log`, `live-sr-fe-cm`, the audit ConfigMap and the credential Secret.

The probe user was first given read on `live9c.t` (deny grant deleted; `21:18:17 probe allowed`). Then:

```text
FE pod uid before: 6feee3d5-cf8f-4bea-9755-64b4b3cafd04
21:18:26
deployment.apps/egret scaled
pod/egret-c6666f98b-9xhqd condition met
21:18:27
download from FE pod: 000 (curl exit 0)
```

**Warm** (egret down 47 s, more than one poll interval):

```text
21:19:14
warm FE, egret down:
id	v
1	a
2	b
2026-10-20 21:19:13.917Z WARN (PolicyRefresher(serviceName=live-sr)-65|65) [RangerRESTClient.shouldRetry():869] Failed to communicate with Ranger Admin. URL: http://egret.egret-system.svc.cluster.local:8080/ranger. Error: java.net.ConnectException: Connection
```

**Cold** (`kubectl delete pod live-sr-fe-0` at 21:19:22, Ready again at 21:19:55):

```text
FE pod uid after: 66257706-5917-497d-9448-ad51815495c4
/tmp/ranger/live-sr/policycache/:
total 12
drwxr-xr-x 2 root root 4096 Oct 20 21:19 .
drwxr-xr-x 3 root root 4096 Oct 20 21:19 ..
-rw-r--r-- 1 root root   89 Oct 20 21:19 starrocks_live-sr_roles.json
2026-10-20 21:20:19.830Z ERROR (PolicyRefresher(serviceName=live-sr)-65|65) [RangerAdminRESTClient.getServicePoliciesIfUpdated():157] Error getting policies; Received NULL response!!. secureMode=true, serviceName=live-sr
2026-10-20 21:20:19.830Z WARN (PolicyRefresher(serviceName=live-sr)-65|65) [PolicyRefresher.loadFromCache():370] cache file does not exist or not readable '/tmp/ranger/live-sr/policycache/starrocks_live-sr.json'
```

The Iceberg catalog cannot be used at all with egret down, because egret also serves the Iceberg REST
catalog: both the probe and `root` got
`ERROR 1064 (HY000) at line 1: Failed to init connector [type: iceberg, name: live_wh]. msg: Failed to load rest catalog`.
So the Ranger decision was tested on an internal table, `default_catalog.live9c.t`, as a native user
`admin` (created by `root` with egret down). The `egret-admin - *` policies grant user `admin` full access:

```text
21:20:35
cold FE, egret down, as native user 'admin' (egret-admin policy holder):
ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN v for this operation.
# SHOW DATABASES as admin: no rows
```

egret back (`kubectl scale deploy/egret --replicas=1` at 21:31:15, rolled out at 21:31:24):

```text
21:31:40 native admin allowed again:
id	v
1	a
2	b
-rw-r--r-- 1 root root 20930 Oct 20 21:31 starrocks_live-sr.json
2026-10-20 21:31:40.891Z INFO (starrocks-mysql-nio-pool-7|768) [PolicyRefresher.loadPolicyfromPolicyAdmin():298] PolicyRefresher(serviceName=live-sr): found updated version. lastKnownVersion=-1; newVersion=47
```

**Trino.** The coordinator pod was deleted at 21:20:43 with egret down. The new pod never reached the
Ranger plugin: it went to `CrashLoopBackOff` (exit code 100) with

```text
2026-10-20T21:28:58.668Z	ERROR	main	io.trino.server.Server	Unable to create injector, see the following errors:
1) [Guice/ErrorInCustomProvider]: RuntimeException: Error reading JWK keys from http://egret.egret-system.svc.cluster.local:8080/.well-known/jwks.json
Caused by: ConnectException: Connection refused
```

The worker, which was not restarted, still had its cache file (`trino_live-trino.json`, 40371 bytes,
21:18). After egret returned, the coordinator started on its next restart; at 21:34:45 its plugin
logged `found updated version. lastKnownVersion=-1; newVersion=75` and
`cache file does not exist or not readable '/tmp/ranger/live-trino/policycache/trino_live-trino_tag.json'`,
and the SQL Lab query returned `[[1,"a"],[2,"b"]]`.

**Verdict: the open availability risk is real.** The plugin cache at `/tmp/ranger/<profile>/policycache`
does **not** survive a pod restart (there is no volume). A warm plugin keeps enforcing its policies with
egret down; a restarted StarRocks FE denies everything, including to the `egret-admin` policy holder,
until egret is reachable again. A JWT Trino coordinator does not start at all without egret. Finding 2.

## 5. Rotation of a provisioned engine

Trino. The watcher (`live9c-watch.sh`, one line every ~4 s) prints the engine row, the
`ranger-credential` Secrets, the pods with their `egret/ranger-credential` annotation, the last download
per plugin in the last 5 minutes, and the engine's `policy.ranger_auth_failures` row. Credential ids are
abbreviated as `OLD(8f99e754)` and `NEW(a64f372e)`.

```text
21:11:46
HTTP/1.1 204 No Content
21:11:46
{"error":"conflict: a ranger credential rotation is pending: the previous credential stays valid until the engine's pods are observed to have rolled out with the current one; retry once that rollout has completed"}
HTTP 409
{"rangerCredentialId":"a64f372e-0f9a-4b49-a848-70860d9b9769","rangerCredentialPending":true}
```

```text
21:11:44 cur=OLD prev=- | secrets: live-trino-rc-8f99e7 | pods: coordinator-787b66c94f-trnzf(Running,OLD) worker-698fff79cf-4zfxh(Running,OLD) | downloads: coordinator-787b…@21:11:32 worker-698f…@21:11:28 | authfail: fail_count=1 last=21:10:44 ok=21:11:02
21:11:48 cur=NEW prev=OLD | secrets: live-trino-rc-8f99e7 live-trino-rc-a64f37 | pods: coordinator-7465c9d667-l9nnx(Running,NEW) coordinator-787b66c94f-trnzf(Running,OLD) worker-5f6cdbb7df-tdwhx(Running,NEW) worker-698fff79cf-4zfxh(Running,OLD) | downloads: coordinator-787b…@21:11:32 worker-698f…@21:11:28 | authfail: fail_count=1 last=21:10:44 ok=21:11:02
21:12:01 cur=NEW prev=OLD | … | downloads: coordinator-787b…@21:11:32 worker-698f…@21:11:58 | …
21:12:05 cur=NEW prev=OLD | … | downloads: coordinator-787b…@21:12:02 worker-698f…@21:11:58 | …
21:12:12 cur=NEW prev=OLD | secrets: live-trino-rc-8f99e7 live-trino-rc-a64f37 | pods: coordinator-7465c9d667-l9nnx(Running,NEW) worker-5f6cdbb7df-tdwhx(Running,NEW) | …
21:12:20 cur=NEW prev=OLD | … | downloads: … worker-5f6cdbb7df-tdwhx@21:12:19 … | authfail: fail_count=1 last=21:10:44 ok=21:11:02
21:12:24 cur=NEW prev=OLD | … | downloads: coordinator-7465c9d667-l9nnx@21:12:22 … worker-5f6cdbb7df-tdwhx@21:12:19 … | authfail: fail_count=1 last=21:10:44 ok=21:11:02
21:12:28 cur=NEW prev=- | secrets: live-trino-rc-a64f37 | pods: coordinator-7465c9d667-l9nnx(Running,NEW) worker-5f6cdbb7df-tdwhx(Running,NEW) | … | authfail: fail_count=2 last=21:12:28 ok=21:11:02
```

The old credential, probed from the host every 5 s:

```text
21:11:53 old-cred=200
… (200 at every probe through 21:12:23)
21:12:28 old-cred=401
```

egret's log (DEBUG lines are on in this chart):

```text
2026-10-20T21:11:57.980252Z DEBUG egret_engine_mgmt::ranger_credential: the previous ranger credential is kept: rollout not observed complete engine=live-trino reason=Deployment live-trino-coordinator: rollout in progress (1 of 1 updated, 2 replicas, 1 ready, 1 ava…
2026-10-20T21:12:27.985931Z  INFO egret_engine_mgmt::ranger_credential: the rollout of the current ranger credential is complete; the previous one is retired engine=live-trino secret_id=8f99e754-222d-4d71-bbb3-9d3af28ee1d7
2026-10-20T21:12:28.676988Z  WARN egret_policy::auth_failures: ranger credential rejected engine_id=c4c38045-5b82-4b48-9824-ab6273086c54 rejected=1
2026-10-20T21:12:50.645356Z  WARN egret_policy::auth_failures: ranger credential rejected engine_id=c4c38045-5b82-4b48-9824-ab6273086c54 rejected=1
```

The only `ranger credential rejected` lines after the rotation are 21:12:28 and 21:12:50, which are this
check's own old-password probes; no engine pod got a 401. Afterwards: `old => 401`, `new => 200`.

StarRocks, the same way (`OLD(0516ade4)`, `NEW(4566116e)`):

```text
21:12:59
HTTP 204
{"error":"conflict: a ranger credential rotation is pending: …"}
HTTP 409
21:12:56 cur=OLD prev=- | secrets: live-sr-rc-0516ad | pods: live-sr-cn-0(Running,) live-sr-fe-0(Running,OLD) | downloads: live-sr-fe-0-live-sr@21:12:43
21:12:59 cur=NEW prev=OLD | secrets: live-sr-rc-0516ad live-sr-rc-456611 | pods: live-sr-cn-0(Running,) live-sr-fe-0(Running,OLD) | downloads: live-sr-fe-0-live-sr@21:12:57
21:13:03 cur=NEW prev=OLD | … | pods: live-sr-cn-0(Running,) live-sr-fe-0(Pending,NEW) | downloads: live-sr-fe-0-live-sr@21:12:57
21:13:11 cur=NEW prev=OLD | … | pods: … live-sr-fe-0(Running,NEW) | downloads: live-sr-fe-0-live-sr@21:13:09
21:13:27 cur=NEW prev=- | secrets: live-sr-rc-456611 | … | downloads: live-sr-fe-0-live-sr@21:13:14
21:13:00 … 21:13:24 old-cred=200;  21:13:30 old-cred=401
2026-10-20T21:13:27.978384Z  INFO egret_engine_mgmt::ranger_credential: the rollout of the current ranger credential is complete; the previous one is retired engine=live-sr secret_id=0516ade4-4fd3-4bbc-b690-73f1d6d598f0
old => 401
new => 200
[{"name":"live-sr-rc-456611-3229","len":22,"secret":"live-sr-rc-456611"}]
```

The StarRocks operator rolled the FE on the Secret name change. Retirement came 28 to 41 s after the
rotation, and in both cases only after the new pods had downloaded with the new credential.

**Verdict: PASS.** One point to read correctly: during the Trino roll the old pods were gone by 21:12:12
and the new ones first downloaded at 21:12:19 and 21:12:22. In those 7 to 10 s there was no running
plugin with policies, but there was also no serving pod, so no query could run. That is the normal
Deployment roll, not a credential gap. The single-FE StarRocks engine is unavailable while its one FE
restarts.

## 6. External engine and `engine_auth_failing`

Registration (`live-ext`, StarRocks, host `fake-sr.example.invalid:9030`, plugin 2.9.0, profile 4):

```text
HTTP 201
Content-Type: application/json
cache-control: no-store
{"id":"763e444a-b56c-468c-82ec-2950c2877389","name":"live-ext","origin":"external","rangerCredentialId":"09036b40-b2d3-41c8-a1bc-79c6f637204f","rangerCredentialPending":false,"rangerCredential":{"username":"engine-763e444a-b56c-468c-82ec-2950c2877389","credentialId":"09036b40-b2d3-41c8-a1bc-79c6f637204f","password_len":43,"password_first2":"y7"}}
```

(The body was summarised with `jq`; it carries the password itself.) Reads afterwards:

```text
GET /api/control-plane/engines/763e444a-…: contains password=0 has_rangerCredential_key=0
GET /api/control-plane/engines: contains password=0 has_rangerCredential_key=0
GET /api/control-plane/engines/763e444a-…/ranger-plugin-config: contains password=0 has_rangerCredential_key=0
{"rangerCredentialId":"09036b40-b2d3-41c8-a1bc-79c6f637204f","rangerCredentialPending":false}
{"authMode":"engine-credential","has_placeholder":true,"has_username":true}
cred1 on own profile => 200
cred1 on live-sr => 403
```

Rotation and retirement:

```text
rotate HTTP 200
cache-control: no-store
{"username":"engine-763e444a-b56c-468c-82ec-2950c2877389","credentialId":"8258d8cf-e709-40a5-abed-d91768d3f1cc","password_len":43,"password_first2":"ij"}
{"rangerCredentialId":"8258d8cf-e709-40a5-abed-d91768d3f1cc","rangerCredentialPending":true}
old(cred1) => 200  new(cred2) => 200
{"error":"conflict: a ranger credential rotation is pending: the previous credential is still valid until you confirm the current one is installed on the engine with POST /api/control-plane/engines/763e444a-b56c-468c-82ec-2950c2877389/retire-previous-ranger-credential"}
second rotate HTTP 409
retire with stale token installed=CID1(09036b40-b2d3-41c8-a1bc-79c6f637204f):
{"error":"conflict: the credential named as installed is not this engine's current ranger credential (a later rotation, or a concurrent change): nothing was retired; install the current credential and confirm it"}
HTTP 409
old(cred1) => 200
retire with right token installed=CID2(8258d8cf-e709-40a5-abed-d91768d3f1cc):
HTTP 204
repeat retire:
HTTP 204
{"rangerCredentialId":"8258d8cf-e709-40a5-abed-d91768d3f1cc","rangerCredentialPending":false}
old(cred1) => 401  new(cred2) => 200
provisioned retire:
{"error":"conflict: engine live-trino is provisioned: egret retires its previous ranger credential itself once the rollout of the current one is observed complete"}
HTTP 409
```

Broken password, then the Overview attention API (`GET /api/control-plane/governance/attention`):

```text
21:14:44 wrong pw as engine-763e444a-b56c-468c-82ec-2950c2877389 => 401
21:14:48 wrong pw as engine-763e444a-b56c-468c-82ec-2950c2877389 => 401
21:14:52 wrong pw as engine-763e444a-b56c-468c-82ec-2950c2877389 => 401
21:14:56 wrong pw as engine-763e444a-b56c-468c-82ec-2950c2877389 => 401
{"kind":"engine_auth_failing","severity":"critical","count":1,"sentence":"The Ranger credential of engine live-ext is being rejected: 5 rejected credentials, the last at 2026-10-20 21:14:56 UTC; no successful download since. Its plugin cannot download policies and runs on its last cached copy, if any.","sample":[{"label":"live-ext","detail":null,"ref":{"type":"engine","id":"763e444a-b56c-468c-82ec-2950c2877389","name":"live-ext"}}],"ref":{"type":"engine","id":"763e444a-b56c-468c-82ec-2950c2877389","name":"live-ext"},"source":null}
```

(5 = these 4 plus the retired-password probe just before.) Clearing with good downloads:

```text
21:15:08 good download => 200; engine_auth_failing findings=1
21:15:16 good download => 200; engine_auth_failing findings=0
   name   | count |           last_at            |             ok_at
----------+-------+------------------------------+-------------------------------
 live-ext |     5 | 2026-10-20 21:14:56.45661+00 | 2026-10-20 21:15:16.834064+00
```

The first good download did not clear it: successful downloads are written at most once a minute per
engine, and this engine's last write was the 200 checks a minute earlier. That matches the guide.

The same item for the provisioned `live-trino`, raised by scenario 2's wrong password and cleared by the
engine's own poll:

```text
21:10:57 {"kind":"engine_auth_failing","severity":"critical","count":1,"sentence":"The Ranger credential of engine live-trino is being rejected: 1 rejected credential, the last at 2026-10-20 21:10:44 UTC; no successful download since. …"}
21:11:09 engine_auth_failing findings=0
 c4c38045-5b82-4b48-9824-ab6273086c54 |     1 | 2026-10-20 21:10:44.688438+00 | 2026-10-20 21:11:02.559874+00
```

**Verdict: PASS.**

## 7. Browser

Driven with the Playwright MCP browser against `http://egret.localtest.me:28080`, logged in once as the
break-glass admin. Screenshots are in the session scratchpad (`live9c/screens/`), not in git:

1. `live9c-ui-1-register-dialog.png`: after **Register engine** for `live-ui` (StarRocks, 2.9.0, profile
   `live-ui`), the alert dialog "Engine registered: save its Ranger credential" with username, a masked
   password with **Show** and **Copy**, the credential id, the install steps naming
   `ranger.plugin.starrocks.policy.rest.client.username` and `.password`, and **Open plugin configuration**
   disabled until "I have saved the password and the credential id" is ticked.
2. `live9c-ui-2-plugin-config.png`: the Plugin config tab the dialog leads to.
3. `live9c-ui-3-external-pending.png`: after **Rotate credential** (confirmation "Rotate the Ranger
   credential?", then the "New Ranger credential" dialog with **Done** disabled until ticked), the
   section shows "Previous credential pending", **Rotate credential** disabled, **Retire previous
   credential**, and "Rotation is blocked until you retire the previous credential". The retire
   confirmation said "You confirm the engine runs the credential e89ff81d-9cac-4f68-9136-af9764d7b897,
   which the rotation on this page returned." After it the status was "Active".
4. `live9c-ui-4-provisioned-credential.png`: `live-trino`'s Overview: "Active", current credential id
   `a64f372e-0f9a-4b49-a848-70860d9b9769`, **Rotate credential**.
5. `live9c-ui-5-overview-auth-failing.png`: Governance Overview, "Needs attention / Critical: The Ranger
   credential of engine live-ui is being rejected: 1 rejected credential, the last at 2026-10-20
   21:17:20 UTC; …" after two wrong-password requests 0.3 s apart (the count is a lower bound, as
   documented).

Console during the run: one `401 (Unauthorized) @ /api/auth/me` before login, and the browser's
"Password field is not contained in a form" notice on both dialogs. **Verdict: PASS.**

## 8. Secret sweep

Every password known to this run (the two retired provisioned ones, both external ones, the two current
provisioned ones) was searched for in egret's log, both Trino pods' logs, and the StarRocks FE stdout and
log files, at the end of the run. Labels as checked against first characters:

```text
trino-old   (len 43, 5T…): egret-log=0 trino-coord-log=0 trino-worker-log=0 sr-fe-logfiles=0 sr-fe-stdout=0
sr-old      (len 43, cx…): egret-log=0 trino-coord-log=0 trino-worker-log=0 sr-fe-logfiles=0 sr-fe-stdout=0
ext-cred1   (len 43, y7…): egret-log=0 trino-coord-log=0 trino-worker-log=0 sr-fe-logfiles=0 sr-fe-stdout=0
ext-cred2   (len 43, ij…): egret-log=0 trino-coord-log=0 trino-worker-log=0 sr-fe-logfiles=0 sr-fe-stdout=0
trino-cur   (len 43, JW…): egret-log=0 trino-coord-log=1 trino-worker-log=1 sr-fe-logfiles=0 sr-fe-stdout=0
sr-cur      (len 43, 9F…): egret-log=0 trino-coord-log=0 trino-worker-log=0 sr-fe-logfiles=0 sr-fe-stdout=0
```

```text
2026-10-20T21:12:18.873Z	INFO	main	org.apache.ranger.audit.provider.AuditProviderFactory	AUDIT PROPERTY: ranger.plugin.trino.policy.rest.client.password=JW…(REDACTED, 43 chars)
does egret's pod-log API expose it? (engine:read users)
HTTP 200
hits of current password in egret API pod-log response: 1
```

egret's log was clean for all six. Its pod was replaced in scenario 4, so the egret log searched here
starts at 21:31; the earlier pod's log was searched at 21:05 (scenario 3) with no hit. The old Trino
password is not in the current Trino logs only because those pods were replaced; it was logged the
same way while they ran.

## Surprises

1. **The documented image build failed on a shared BuildKit cache.** `docker build -f deploy/docker/backend.Dockerfile`
   failed with `error[E0432]: unresolved import egret_engine_mgmt_api::credential` and similar errors,
   although the worktree has `crates/egret-engine-mgmt-api/src/credential.rs`. The Dockerfile's
   `--mount=type=cache,target=/src/target` is shared by every checkout that builds on this Docker
   daemon. Here another checkout's newer artifacts made cargo treat this branch's crates as fresh. A
   copy of the Dockerfile with `id=live9c-egret-target` on that mount (outside the repo) built cleanly.
   Any session building `egret-backend` from a branch can ship another branch's code this way, or fail
   as here. Worth a separate fix (a per-checkout cache id).
2. **`kind load docker-image` failed for the engine images**:
   `ctr: rpc error: code = NotFound desc = content digest sha256:aa18e61b…: not found` (kind's
   `--all-platforms` import of a multi-platform image from Docker's containerd store). `crictl pull`
   inside the node worked.
3. **A Trino engine with `memory: 1Gi`** (the value the e2e helpers use) **is OOM-killed**: the
   coordinator showed `"reason":"OOMKilled","exitCode":137` and `CrashLoopBackOff`. A PUT to 2Gi fixed
   it. Before it was killed, the worker had already polled with the credential.
4. **Leftovers after deleting a StarRocks engine**: after `DELETE /engines/{id}` (204) the
   `live-sr-initpwd-*` Job pod (Completed) and the PVC `fe-meta-live-sr-fe-0` (20Gi) stayed. Recreating
   an engine with the same name would have reused the old FE metadata (and its root password). They
   were deleted by hand. This predates the branch.

Not investigated: `live-trino`'s profile version went from 39 (21:10) to 75 across egret's restart in
scenario 4 with no authoring in between. It was stable afterwards (75 and 75, 45 s apart), and the
restart logged `per-user Ranger baseline pass seeded=1 failed=0`. The StarRocks operator logged
`Access denied for user 'root' (using password: NO)` on `SHOW COMPUTE NODES` throughout. That is the
existing initpwd design, unrelated to Ranger.

## Not covered

- An Overview item for a provisioned engine with **no** credential (pre-change engine) was not tested,
  because every engine here was created on this branch.
- The plugin's response to **401** (not connection refused) on a cold start was not repeated live. The
  measurements doc covers it with the real 2.8.0 jar. Scenario 4 used connection refused, which is
  what "egret down" produces.
- `ranger.authMode=anonymous` was not exercised: the brief forbids it.
