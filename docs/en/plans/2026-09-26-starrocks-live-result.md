# StarRocks live result — the gate the four blocker fixes exist to make possible

**Measured:** 2026-09-26, kind context `kind-egret`, branch `fix/starrocks-ranger-blockers`
(`03ec4057`), worktree `egret-sp4a`.

**Why this document exists.** SP4a's permission model shipped (PR #596) with its StarRocks
**masking and row-filter** cases never observed on a real engine: D5's per-column tag expansion had
unit tests and a verified policy layer and nothing else. Four fixes on this branch removed the
obstacles — `665e0020` (the Ranger ConfigMap no longer mounts over `/opt/starrocks/fe/conf`),
`e63e4ca5` (a StarRocks catalog on an egret warehouse carries its S3 config), `ddbc3463`
(`PUT /engines/{id}` honours `authz_service_id`), `03ec4057` (the Ranger service name comes from the
**bound profile row**, not the engine's name). This is the measurement.

Every case below is written **as it finished**. A case that could not be run says so, with what was
tried. Nothing here is inferred from a policy row: the verdicts are engine output, pasted verbatim.

---

## 0. The state found before anything was changed

| fact | value |
|---|---|
| helm revision at task start | 64, all six `--set` values present |
| serving pods | `egret-5b8775647b-869wq` / `-rh9nn`, imageID `sha256:3da6291694f0…` |
| that image's build time | **2026-09-26 08:09:15 +07** — BEFORE all four fix commits (11:07–12:58 +07) |
| StarRocks engine | `sp3c2-sr-live` (`43bc0bf7-…`), `authz_service_id = 54` |
| profile 54 | named `sp3c2-sr-live` — the **same** name as the engine, so `03ec4057`'s bug is invisible against it |
| FE CR | `configMaps: [{mountPath: /opt/starrocks/fe/conf, name: sp3c2-sr-live-ranger-cm}]` — the bare-directory mount, i.e. the pre-`665e0020` render |
| that ConfigMap's keys | `fe.conf`, `ranger-starrocks-audit.xml`, `ranger-starrocks-security.xml` — the third key is SP4a's gate's hand-made workaround for the shadowing |
| FE security xml | `ranger.plugin.starrocks.service.name = sp3c2-sr-live` |
| FE `access_control` | `ranger` — the engine was enforcing, not `allow-all` |
| catalog on the FE | `SHOW CATALOGS` → `default_catalog`, `sp4a10_wh` |
| CN | id `10045`, `Alive: true`, `4.1.3-8a8e186` |
| perm model | 5 grants (principal `analyst`, catalog `sp4a10_wh`), 2 rules — both **path**-selected (mask on `report.item`, row filter `country = 'US'` on `report`) |
| tag `pii` (`2c789387-…`) | assigned to `sp4a.report.item` and `sp4a.report.country` in `catalog_tag_assignments` — SP4a's gate left them there. (`public.tag_assignments`, which is the *BI-object* table and not the one the compiler reads, is empty; reading only that one would have understated the tag's reach.) |
| profile 2 (`demo-trino`) | 15 policies, **0** compiler-owned; coordinator restart count 53 |

So the cluster carried none of the four fixes, and a fresh image was required before any case
could mean anything.

## 1. Instruments, and the ones rejected

- **Subject: `srgate`, a non-admin.** Auto-bind seeds `egret-admin` policies granting `*.*.*.*`, so
  every case run as `admin` passes vacuously. `analyst` (the previous gates' subject) exists on the
  FE but its password is recorded nowhere, and this run does not rewrite a shared credential, so a
  new StarRocks user `srgate` was created additively and every grant below is authored for it.
- **`DESC` rejected.** It returns the column list for denied tables too (SP4a's gate measured this),
  so it cannot discriminate. Every case below is an actual `SELECT`.
- **`demo-trino` untouched** — OOMKilled against its own 1Gi limit, and out of bounds by standing
  project constraint. Re-checked at the end.
- **Engine-level enforcement confirmed before trusting any pass** — `access_control = ranger` read
  from the FE, and the plugin's own evaluator count for the bound service name read from `fe.log`.
- **helm's report is not evidence.** The deploy is verified by the pods' `imageID` and by
  `EGRET_RANGER__ENABLED` read from inside a running pod.
- Engines see a **bare username**, never an email. Query polling terminal state is `finished`.
  Ranger `MASK` is **per character**, so a 4-letter value masks to `xxxx` — correct, not a defect.

## 2. The deploy, verified by the pod and not by helm

The image was rebuilt from `03ec4057` and `kind load`ed: the node's `egret-backend:dev` became
`0167b952681d2` (`crictl images`), built **16:35:33 +07** — after every fix commit.

`helm upgrade` with all six `--set` values **failed the first time** and the failure is worth
recording, because helm's report was right and the cluster was wrong: `Error: UPGRADE FAILED:
context deadline exceeded` after 10 minutes of `--wait`. The cause was **`minio` in
`ImagePullBackOff`** — the dev-stack MinIO container had been OOMKilled during the release build and
its pod carries `imagePullPolicy: Always`, so the kubelet tried to re-pull
`quay.io/minio/minio:RELEASE.2025-09-07T16-13-09Z.hotfix.7aa24e772` and got
`401 UNAUTHORIZED` from quay. The image was **already on the node** (`crictl images` →
`1cc6cb042b265`), so the repair was one patch to the deployment
(`imagePullPolicy: IfNotPresent`); MinIO came back 1/1 and its PVC-backed `/data/warehouse/sp4a10`
was intact. Recorded because a tag whose only copy is on the node plus `Always` is a landmine: any
MinIO restart wedges the whole stack, and with it every Iceberg read.

| check | result |
|---|---|
| helm revision (second attempt) | **66**, `STATUS: deployed` |
| `helm get values` after the upgrade | all **six** user-supplied values, `ranger.enabled: true` among them |
| serving pods' `imageID` | `sha256:0167b952681d236edea123829f4461f6c5039b7e353afe0ca830458fbafd84b2` on **both** `egret-5b949848d6-69rcp` and `-qzk6c`, identical to the node's freshly loaded `egret-backend:dev` |
| `EGRET_RANGER__ENABLED` read from inside both running pods | `true`, `true` |
| `_sqlx_migrations` max | 157 |
| the new code is really serving | `PUT /engines/{id}` with `authzServiceId: 999999` → **400** `authz_service_id 999999 is not a policy profile; list them with GET /api/control-plane/policy/services`. The previous image returned **200** and dropped the field — this is `ddbc3463`, answering live. |

## 3. Case 5 — `03ec4057`: a profile whose name differs from the engine's is the one enforced

A **new** profile `sp4a-sr-crosscheck` (id **55**) was created — deliberately NOT named after the
engine, which is `sp3c2-sr-live`. A same-named bind passes against the old bug and proves nothing;
profile 54 is named `sp3c2-sr-live` and is exactly what the old code would have configured.

The bind itself went through the route `ddbc3463` added:

```text
PUT /api/control-plane/engines/43bc0bf7-…  {"authzServiceId": 55, …}  → 200
select name, authz_service_id from engine_mgmt.engines where id='43bc0bf7-…'
sp3c2-sr-live|55            (was 54)
```

What the provisioner then rendered into the FE, verbatim:

```xml
<property><name>ranger.plugin.starrocks.service.name</name><value>sp4a-sr-crosscheck</value></property>
```

and the plugin's own refresher, from `fe.log` — the only one running:

```text
PolicyRefresher(serviceName=sp4a-sr-crosscheck)
[RangerPolicyRepository.init():930] This policy engine contains 20 policy evaluators
```

The **functional** half, which no rendering check can substitute for: every grant authored below
compiled into `{"services":[52,55]}` — profile 55 and the Trino profile, and **never 54**. So a
`srgate` policy exists only in 55, and §5's `SELECT` returning rows can only come from 55 being the
service the engine evaluates. **PASS.**

## 4. Case 1 — the FE survives a Ranger bind (`665e0020`), but it does not keep its metadata

**The mount is fixed and the FE boots.** The re-rendered CR, verbatim:

```json
[{"mountPath":"/opt/starrocks/fe/conf/ranger-starrocks-security.xml",
  "name":"sp3c2-sr-live-ranger-cm","subPath":"ranger-starrocks-security.xml"},
 {"mountPath":"/opt/starrocks/fe/conf/ranger-starrocks-audit.xml",
  "name":"sp3c2-sr-live-ranger-cm","subPath":"ranger-starrocks-audit.xml"}]
```

No mount on the bare `/opt/starrocks/fe/conf` any more, and the FE reached `1/1 Running` in **64
seconds with 0 restarts** while bound. The pre-fix signature — `ln: failed to create symbolic link
'/opt/starrocks/fe/conf/fe.conf': Read-only file system`, which this same pod printed on its
previous start at 09:54 — **is gone from the new start log**. `665e0020`: **PASS**.

**The metadata did not survive**, and the cause is not the ConfigMap. The new FE pod's first log
line is `first start fe with meta not exist`, and:

```text
SHOW CATALOGS        → default_catalog only        (sp4a10_wh gone)
SHOW ALL AUTHENTICATION → 'root'@'%' | Password: No  (analyst gone, root's password gone)
SHOW COMPUTE NODES   → empty                       (the CN's registration gone)
```

Before the bind the same FE had `default_catalog` + `sp4a10_wh` and users `root` + `analyst`.
The reason is in the pod spec: **`fe-meta` is an `emptyDir`** and there is no PVC in
`egret-engines` at all —

```json
{"emptyDir": {}, "name": "fe-meta"}
{"emptyDir": {}, "name": "fe-log"}
```

so *any* pod replacement wipes the FE's metadata, and a profile bind is a pod replacement. SP4a's
gate saw the same loss and attributed it to the shadowing mount; this run separates the two: the
mount is fixed, the FE boots, and the metadata is still lost. **Case 1: PASS on "the FE starts",
FAIL on "keeps its metadata" — with a cause (`emptyDir` FE meta) that is a provisioning defect of
its own, not `665e0020`'s.**

Recovery, all additive and recorded for reproducibility: root's password was set back to the value
in `sp3c2-sr-live-initpwd-secret` (the FE came up with root having **no** password, which also
breaks the CN's `MYSQL_PWD` self-registration), the CN pod was deleted so it re-registered
(`ComputeNodeId: 10031, Alive: true, 4.1.3-8a8e186`), and the catalog was re-applied through
`POST /engines/{id}/catalogs/{cid}/apply` → `{"status":"applied","error":null}`.

### `e63e4ca5` at the same time — the catalog now carries its S3 config

`SHOW CREATE CATALOG sp4a10_wh`, verbatim properties:

```text
"aws.s3.endpoint" = "http://minio.egret-system.svc.cluster.local:9000"
"aws.s3.enable_ssl" = "false"   "aws.s3.enable_path_style_access" = "true"
"aws.s3.region" = "us-east-1"   "aws.s3.use_instance_profile" = "false"
"iceberg.catalog.vended-credentials-enabled" = "true"
"iceberg.catalog.type" = "rest"  "iceberg.catalog.uri" = "http://egret.egret-system.svc.cluster.local:8080/catalog"
```

No `aws.s3.access_key`, no `aws.s3.secret_key`. Before the fix the catalog carried no `aws.s3.*` at
all and fell back to the FE's own `aws_s3_endpoint = http://minio:9000`, which does not resolve from
`egret-engines` — every object read came back `S3, Status Code: 400`. It reads now, which is what
made every case below possible.

## 5. S1 — a grant on a StarRocks table returns real rows

Subject `srgate`; `select current_user()` → **`'srgate'@'%'`** (the bare username, D7 again).

The control first — the same query with **no** grant:

```text
SELECT id,item,country FROM sp4a10_wh.sp4a.srcases ORDER BY id
ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s)
                              on COLUMN country for this operation.
```

Then `{effect: allow, actions:[read], scope:{catalog: sp4a10_wh, schema: sp4a, table: srcases,
column: "*"}, principals:[user srgate]}` (`compile → {"services":[52,55],"created":2}`), and after
the plugin's poll:

```text
id	item	country
1	book	US
2	pen	VN
3	lamp	US
4	desk	VN
```

The literal values, matching what Trino reads as admin from the same table. **PASS.**

## 6. S2 — deny on one table, the sibling still reads

`allow read` **and** `deny read` on `sp4a.secret.*` for `srgate`:

```text
SELECT id,note FROM sp4a10_wh.sp4a.secret ORDER BY id
ERROR 5204 (42000) at line 1: Access denied; you need (at least one of) the SELECT privilege(s)
                              on COLUMN note for this operation.

SELECT count(*) FROM sp4a10_wh.sp4a.srcases
4
```

Deny wins over allow, and it is scoped to the table it names. **PASS.**

## 7. S3 — the tag mask, at the read path, per column

`pii` is on **five** columns across **three** tables: `report.item`, `report.country` (SP4a's gate
left those), plus `sibling.v`, `srcases.item`, `srcases.country` added here. One rule was written:
`{name: sr-tagmask-pii, kind: {mask, mask_type: MASK}, selector: {tag, key: pii}}`.

The compile, and D5's asymmetry in one response: `{"services":[2,52,55],"created":6}` with a
`skipped` entry for the profile whose `tagService` names no row. Six = **five** StarRocks policies +
**one** Trino tag policy. In profile 55, verbatim names:

```text
perm-rule-47ab3a6a-…-sp4a10_wh.sp4a.report.country   policyType 1
perm-rule-47ab3a6a-…-sp4a10_wh.sp4a.report.item      policyType 1
perm-rule-47ab3a6a-…-sp4a10_wh.sp4a.sibling.v        policyType 1
perm-rule-47ab3a6a-…-sp4a10_wh.sp4a.srcases.country  policyType 1
perm-rule-47ab3a6a-…-sp4a10_wh.sp4a.srcases.item     policyType 1
```

and for the same rule, service 51 (`sp3c2-trino-live_tag`) holds exactly **1** tag-keyed policy.
**One policy per column on StarRocks, one policy in total on Trino** — and nothing in profile 2
(`demo-trino`) or 54.

Then the engine, as `srgate`, after the plugin's poll — the measurement this whole branch exists for:

```text
SELECT id,item,country FROM sp4a10_wh.sp4a.srcases ORDER BY id
id	item	country
1	xxxx	XX
2	xxx	XX
3	xxxx	XX
4	xxxx	XX

SELECT id,v FROM sp4a10_wh.sp4a.sibling ORDER BY id
id	v
1	xxxxx
2	xxxx
```

The literal values. `book`→`xxxx`, **`pen`→`xxx`** (three characters, not four), `lamp`→`xxxx`,
`desk`→`xxxx`; `alpha`→`xxxxx`, `beta`→`xxxx`; and `US`/`VN`→**`XX`** — Ranger's `MASK` is
per character *and* case-preserving (uppercase → `X`, lowercase → `x`). `id` is untouched, so the
mask is scoped to the tagged columns and not to the row. No real value survives anywhere in the
tagged columns. **PASS** — the first time D5's per-column tag expansion has been observed at the
read path on any engine.

## 8. S4 — the tag row filter, per TABLE, and the excluded rows are absent

Same tag, one more rule: `{name: sr-tagfilter-id, kind: {row_filter, filter_expr: "id > 2"},
selector: {tag, key: pii}}`. `{"services":[2,52,55],"created":4}` — **three** StarRocks policies +
one Trino tag policy. The three, verbatim:

```text
perm-rule-570d7e80-…-sp4a10_wh.sp4a.report     policyType 2
perm-rule-570d7e80-…-sp4a10_wh.sp4a.sibling    policyType 2
perm-rule-570d7e80-…-sp4a10_wh.sp4a.srcases    policyType 2
```

**Five mask policies and three row-filter policies from the same tag on the same five columns.**
The row-filter names carry no column, because `rowFilterDef` has no column resource — two tagged
columns of one table are one and the same filter. The counts cannot coincide, which is what makes
this a measurement and not a restatement.

At the read path, as `srgate` (`id > 2` on top of S3's masks):

```text
SELECT id,item,country FROM sp4a10_wh.sp4a.srcases ORDER BY id
id	item	country
3	xxxx	XX
4	xxxx	XX

SELECT count(*) FROM sp4a10_wh.sp4a.srcases          → 2      (was 4)
SELECT id FROM sp4a10_wh.sp4a.srcases WHERE id <= 2  → (no rows, rc=0)
SELECT count(*) FROM sp4a10_wh.sp4a.sibling          → 0      (its ids are 1 and 2)
SELECT id,v FROM sp4a10_wh.sp4a.sibling              → (no rows, rc=0)
```

4 rows → 2, and the direct probe for the excluded rows returns **zero** — they are genuinely absent,
not merely fewer. On `sibling`, whose only ids are 1 and 2, the filter removes the table's entire
contents, which is the same fact from the other side. **PASS.**

## 9. Cross-checks repeated on purpose at the end

| check | at task start | at the end |
|---|---|---|
| profile 2 (`demo-trino`) total / compiler-owned policies | 15 / **0** | 15 / **0** |
| `demo-trino` coordinator restarts | 53 | **53** |
| `demo-trino` used as an instrument or its data touched | — | **no**, never queried |
| profile 54 (`sp3c2-sr-live`, the same-named one, now unbound) | 20 / 7 | **20 / 7** — nothing written to it after the re-point |
| profile 55 (`sp4a-sr-crosscheck`, bound) | did not exist | 32 / 19 (13 seeded `egret-admin` + 19 compiler-owned) |
| service 51 (`sp3c2-trino-live_tag`) | 0 | **2** — one tag policy per tag rule, against StarRocks' 5 + 3 |
| FE `access_control` | `ranger` | `ranger`, and `PolicyRefresher(serviceName=sp4a-sr-crosscheck)` is the only refresher |

Every tag-rule compile response carried an explicit `skipped` entry for the profile whose
`tagService` names no `policy.authz_services` row, which is what keeps `demo-trino` out of it.

## 10. Verdicts

| # | case | verdict | the actual observed value |
|---|---|---|---|
| 1 | the FE survives a Ranger bind | **PASS** (boot) / **FAIL** (metadata) | `1/1 Running`, 0 restarts, 64 s, bound; no `Read-only file system` line. But `first start fe with meta not exist`: `SHOW CATALOGS` → `default_catalog` only, `SHOW ALL AUTHENTICATION` → `root` with no password, `SHOW COMPUTE NODES` → empty. Cause: `fe-meta` is an `emptyDir`. |
| S1 | grant `read` on a StarRocks table | **PASS** | `1 book US / 2 pen VN / 3 lamp US / 4 desk VN`; the same query before the grant: `ERROR 5204 … SELECT privilege(s) on COLUMN country` |
| S2 | `deny` on one table, sibling still reads | **PASS** | `secret`: `ERROR 5204 (42000) … on COLUMN note`; `srcases`: `count(*) = 4` |
| S3 | **tag mask** on StarRocks columns | **PASS** | `item`: `xxxx`, **`xxx`**, `xxxx`, `xxxx`; `country`: `XX`; `sibling.v`: `xxxxx`, `xxxx`; `id` untouched. **5 policies, one per tagged column** (vs 1 on Trino's tag service) |
| S4 | **tag row filter** on StarRocks tables | **PASS** | `srcases` 4 rows → **2** (ids 3, 4); `count(*)` = 2; `WHERE id <= 2` → **no rows**; `sibling` `count(*)` = **0**. **3 policies, one per TABLE** |
| 5 | a differently-named bound profile is enforced | **PASS** | engine `sp3c2-sr-live` bound to profile **`sp4a-sr-crosscheck`**; xml + refresher both name the profile; every `srgate` policy compiled to `[52,55]`, never 54, and the engine honoured them |

**5 mask policies and 3 row-filter policies from one tag on five columns of three tables** is D5's
central claim, measured at the read path for the first time.

## 11. Findings this run produced (none is SP4a's, none was fixed here)

1. **The FE's metadata volume is an `emptyDir`.** `{"emptyDir": {}, "name": "fe-meta"}`, and
   `egret-engines` holds no PVC at all. Any FE pod replacement — and a profile bind is one — destroys
   every catalog, every user, root's password, and the CN's registration. SP4a's gate saw this loss
   and read it as a symptom of the shadowing mount; with the mount fixed, the loss is unchanged, so
   the two are independent. Today a bind therefore costs a re-`apply` of every catalog and a CN
   restart.
2. **The FE's JVM heap is hardcoded `-Xmx8192m`** in `starrocks_fe_configmap`, with no relation to
   `feResources` (which is `null` here). On this host the FE was OOMKilled repeatedly — 72 restarts,
   `exitCode 137` — and could not stay up at all while anything else wanted memory. Worked around
   from the API, not by editing egret: `feConfig: {"JAVA_OPTS": "\"… -Xmx2048m -XX:+UseG1GC\""}`
   (`JAVA_OPTS` is not in `SR_PROTECTED_FE_KEYS`, and StarRocks' own `fe.conf` parser normalises the
   spaces `merge_fe_conf` renders around `=`). The FE has been stable since.
3. **A ConfigMap key deleted from the render is not deleted from the cluster.**
   `sp3c2-sr-live-ranger-cm` still carries the `fe.conf` key SP4a's gate added by hand, because the
   provisioner's `apply` merges `data` and never prunes. Harmless now — only the two XML keys are
   mounted — but a stale key in the very ConfigMap that used to shadow the FE's conf directory is
   worth knowing about.
4. **`root` is not a valid subject for StarRocks access cases, but it *is* subject to masking and row
   filters.** Before any `srgate` grant existed, `root` read `sp4a.report` and got rows — while
   `item` came back `xxxx` and only the `country = 'US'` rows appeared, i.e. the compiled path rules
   applied to root. So "root could read it" says nothing about grants, and every access case here was
   run as `srgate`.
5. **MinIO's pinned tag is a landmine.** `imagePullPolicy: Always` plus
   `quay.io/minio/minio:RELEASE.2025-09-07T16-13-09Z.hotfix.7aa24e772`, which now answers `401
   UNAUTHORIZED` on quay, means any MinIO restart wedges the dev stack even though the image is on
   the node. It cost this run a failed `helm upgrade` and a repair patch.

## 12. What this run did NOT settle

- **`write` on StarRocks.** SP4a maps `write` → `insert,update,delete` there; only `read` was
  exercised. No `INSERT` was attempted as `srgate`.
- **The Trino side of the two tag rules** was counted at the policy layer (1 tag policy each, on
  service 51) but not read back through Trino — this run's subject exists only on StarRocks.
- **`analyst` and the SP4a gate's FE-side state** were destroyed by the metadata loss of §4 and were
  not recreated; `sp4a10_wh` was.
- **Group principals** — unchanged, no group provider exists.
- The engine is left bound to **profile 55**, with the fixture (`sp4a.srcases`, user `srgate`, three
  tag assignments, five grants, two tag rules) in place: all additive, and the state the verdicts
  above were measured in.
