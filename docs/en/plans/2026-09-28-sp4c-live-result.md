# SP4c live result — the intersection property, measured

**Measured:** 2026-09-28, kind context `kind-egret`, branch
`feat/sp4c-classification-clearance`, worktree `egret-sp4a`.

**The acceptance test is one sentence: a user without clearance cannot read a
tagged column, by any path.** Not that a policy was written. Every verdict below
is engine output, pasted verbatim. Each case was written into this document as it
finished; a case that could not be run says so, with what was tried and what came
back. Nothing here is inferred from a policy row.

`demo-trino` was not used and not modified.

## Verdict

**Four PASS, one FAILED, one BLOCKED. The acceptance sentence holds for the query
paths SP4c set out to close and does NOT hold through a view.**

| # | case | verdict | the actual value observed |
|---|---|---|---|
| 1 | intersection property — uncleared read refused, SQL Lab **and** notebook, both engines | **PASS** | Trino `Access Denied: Cannot select from columns [id, secret] in table or view gate`; StarRocks `ERROR 5204 (42000): Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN secret`. Same two denials from the notebook kernel. The untagged column `plain` still returns 3 rows. |
| 2 | cleared user still reads — **first observation of `denyExceptions` on StarRocks** | **PASS** | `d3member` gets `[[1,'classified-one'],[2,'classified-two'],[3,'classified-three']]` on both engines, SQL Lab and notebook, while `analyst` is refused in the same minute. |
| 3 | the clearance lifts and the policy is reaped | **PASS** | profile 51: 4 → 3 policies, profile 55: 35 → 34, `classification_policies` row gone; every read returns 4 rows again. |
| 4 | the view bypass, on both engines | **FAILED (measured Critical)** | StarRocks: `SELECT id, secret FROM sp4cgate.gate_v` → all three classified values to `analyst`. Trino: same view read succeeds in full once the view's **owner** is cleared. Both MVs **BLOCKED** (see 4c). |
| 5 | StarRocks withholds reads only | **PASS as stated — and Trino does too, which the UI denies** | `analyst` INSERTs row 91 on StarRocks (`finished`; `d3member` reads it back) and still cannot SELECT it. The same subject also INSERTed **and DELETEd** on Trino, whose deny carries `insert, delete`. |
| 6 | a rename does not break it | **PASS** | `PATCH {"name":"sp4cgate2"}` → policy 8840 `resources.tag.values` = `["sp4cgate2"]`, tag download `tagVersion 10 / type sp4cgate2`; deny and exception both still bite. |

Two defects were measured that the six cases did not ask for, and both are
recorded in full below: **Trino's clearance withholds reads only** (its `insert` /
`delete` accesses match nothing, because the tag sits on a *column* resource while
the write checks present a *table* one), and **`explain` disagrees with the engine
about a cleared user** because `ExplainSubject.roles` is caller-supplied and
defaults to empty.

---

## 1. Instruments, and the ones rejected

- **Subjects are non-admin.** Auto-bind seeds `egret-admin` policies granting
  `*.*.*.*` on both profiles, so every case run as `admin` passes vacuously.
  - `analyst` (`analyst@egret.local`, `users.username = analyst`) — the
    **uncleared** subject. Holds no platform role.
  - `d3member` (`d3member@egret.local`, `users.username = d3member`) — the
    **cleared** subject. Holds exactly one platform role, `sp4c_cleared`, created
    for this run.
- **`root` rejected on StarRocks.** It bypasses the access check before Ranger is
  consulted (`RangerAccessController.java:115-117`), so "root could read it"
  proves nothing about a deny. It is used below only for DDL and for reading FE
  state.
- **`DESC` rejected.** It returns the column list for denied tables too, so it
  cannot discriminate. Every case below is an actual `SELECT`.
- **Engines see a bare username, never an email.** Measured, not assumed:
  `SELECT current_user` on Trino → `admin`; `SELECT current_user()` on StarRocks
  → `'admin'@'10.244.0.41'`.
- **Query polling's terminal state is `finished`**, not `succeeded`.
- **helm's report is not evidence.** The deploy is verified by the serving pods'
  `imageID` and by `EGRET_RANGER__ENABLED` read from *inside* a running pod.

### The fixture

A new table, so no SP4a rule already in the store can mask or filter the column
under test (the pre-existing `sr-tagmask-pii` / `sr-tagfilter-id` rules select on
tag `pii`, which this fixture does not use):

```sql
CREATE TABLE sp4a10_wh.sp4a.gate (id BIGINT, secret VARCHAR, plain VARCHAR)
INSERT VALUES (1,'classified-one','open-one'),(2,'classified-two','open-two'),(3,'classified-three','open-three')
```

Created through Trino into the Iceberg REST warehouse `sp4a10_wh`
(`registry.catalogs.id = df341757-…`), which **both** engines are attached to, so
one table is read by both. Governance objects, all created through the egret API:

| object | value |
|---|---|
| tag | `sp4cgate` (`3954a1a4-386d-40fb-bdc8-e2b4f657f4fd`), key and name both `sp4cgate` |
| assignment | `sp4a10_wh` / `sp4a` / `gate` / **`secret`** (one column) |
| clearing role | `sp4c_cleared` (`9b14218c-30eb-4859-8dc1-46be71faca3e`), platform scope |
| role holder | `d3member` only (`GET /access/roles/{id}/assignees` → `[{"kind":"user","name":"d3member@egret.local"}]`) |
| perm grants | `allow {read,write}` on `sp4a10_wh`.`sp4a` for `analyst` (`d6cc7b6c-…`) and for `d3member` (`8c732df0-…`); each compiled to `{"services":[52,55]}` |

### Baseline, before any clearance exists

Both subjects read the column that is about to be classified, on both engines.
This is what makes a later refusal mean *clearance* rather than *no grant*:

```text
analyst  / Trino      SELECT id, secret FROM sp4a10_wh.sp4a.gate ORDER BY id
                      → finished, 3 rows: [[1,'classified-one'],[2,'classified-two'],[3,'classified-three']]
d3member / Trino      → finished, 3 rows, same values
analyst  / StarRocks  → finished, 3 rows, same values
d3member / StarRocks  → finished, 3 rows, same values
```

The **notebook** path was baselined too, through a real per-user kernel pod (not a
simulation of one): `POST /ws/demo/kernel` → `phase: ready`, `POST
/notebooks/{id}/kernel/connect` → `kernel_id a80c8c9b-…`, then an
`execute_request` over the Jupyter channels WebSocket
(`/api/ws/demo/notebooks/{id}/kernel/channels`) carrying `analyst`'s own session
cookie. The cell calls the kernel's own helpers, which mint a per-user engine JWT
and dial the engine directly — a different credential path from SQL Lab's, which
is the whole reason the union was visible:

```text
import egret_kernel_helpers as h
h.trino("SELECT id, secret FROM sp4a10_wh.sp4a.gate ORDER BY id", engine="sp3c2-trino-live")
→ me = analyst
→ TRINO ROWS: {'id': [1, 2, 3], 'secret': ['classified-one', 'classified-two', 'classified-three']}

h.starrocks("SELECT id, secret FROM sp4a10_wh.sp4a.gate ORDER BY id", engine="sp3c2-sr-live")
→ SR ROWS: {'id': [1, 2, 3], 'secret': ['classified-one', 'classified-two', 'classified-three']}
```

### The cluster repairs this run had to make first, recorded for reproducibility

None of these is part of what SP4c built; all are additive, and all were needed
before any case could mean anything.

1. **`sp3c2-trino-live` was scaled to 0.** Its coordinator and worker deployments
   both had `replicas: 0` (only `demo-trino`, which is out of bounds, was
   running). Scaled each to 1; the coordinator came up with
   `access-control.name=ranger`, `ranger.service.name=sp3c2-trino-live`.
2. **The StarRocks FE had lost its metadata again** (`feMetaPersistent: false` —
   an `emptyDir`, the defect SP4a's gate recorded). Found: `SHOW CATALOGS` →
   `default_catalog` only, `SHOW ALL AUTHENTICATION` → `'root'@'%'` with **no**
   password, `SHOW COMPUTE NODES` → empty, `SHOW SECURITY INTEGRATIONS` → empty,
   `authentication_chain` → `[native]`. Repairs: root's password set back to the
   value in `sp3c2-sr-live-initpwd-secret`; `POST
   /engines/{id}/catalogs/{cid}/apply` → `{"status":"applied"}`, restoring the
   `sp4a10_wh` Iceberg catalog; the compute node re-added by
   `ALTER SYSTEM ADD COMPUTE NODE` plus a CN pod restart (it first reported
   `ErrMsg: Out-dated epoch`, then `Alive: true, 4.1.3-8a8e186`); and the JWT
   security integration `egret_oidc` re-created with egret's own coordinates
   (`jwks_url=http://egret.egret-system.svc.cluster.local:8080/.well-known/jwks.json`,
   `required_issuer=http://egret.localtest.me`, `required_audience=engine`),
   `authentication_chain=native,egret_oidc`.
3. **The StarRocks built-in storage volume pointed at an unreachable endpoint**
   (`aws.s3.endpoint=http://minio:9000`, a name that does not resolve from
   `egret-engines`), so creating anything with internal storage failed with
   `Connection refused`. Set to `http://minio.egret-system.svc.cluster.local:9000`
   with `aws.s3.enable_path_style_access=true`.
4. **`d3member` could not reach Trino at all** — `Access Denied: Principal
   d3member cannot become user d3member`. The five hand-written `t10-*` policies
   on profile 52 (impersonate, execute, sysinfo, function, systemproperty) named
   only `analyst`; `d3member` was added to each through `PUT
   /api/control-plane/policies/{id}`. **A defect found doing it:** that route
   takes the *editor* shape and runs it through `editor_to_ranger_json`, while
   `GET /api/control-plane/policies/{id}` returns the *Ranger* shape — so a
   read-modify-write round trip through the two silently drops every policy item
   (`policyItems: []`, i.e. the policy stops granting anything) and still answers
   `200`. Repaired by re-authoring all five in the editor shape.

Two more baselines, both needed so a later result cannot be read as an engine
limitation:

```text
d3member / notebook   TRINO ROWS:     {'id': [1,2,3], 'secret': ['classified-one','classified-two','classified-three']}
                      STARROCKS ROWS: {'id': [1,2,3], 'secret': ['classified-one','classified-two','classified-three']}

analyst / StarRocks   INSERT INTO sp4a10_wh.sp4a.gate VALUES (90,'write-before-clearance','plain90')
                      → finished     (so StarRocks CAN write this Iceberg table; row 90 was then deleted through Trino)
```

### The view surfaces, and the one that could not be measured

Built as `admin` on the same fixture table, so case 4 has something to read
through:

| surface | engine | state |
|---|---|---|
| view `sp4a10_wh.sp4a.gate_v` | Trino | created (`CREATE OR REPLACE VIEW … AS SELECT id, secret, plain FROM sp4a10_wh.sp4a.gate`) |
| **materialized view** | **Trino** | **cannot exist.** `CREATE MATERIALIZED VIEW sp4a10_wh.sp4a.gate_mv …` → `engine: createMaterializedView is not supported for Iceberg REST catalog` |
| view `default_catalog.sp4cgate.gate_v` | StarRocks | created |
| materialized view `default_catalog.sp4cgate.gate_mv` | StarRocks | created and refreshed (`REFRESH … WITH SYNC MODE`), returns 3 rows to `admin` |

Pre-clearance, `analyst` reads every one of these **except** the StarRocks
materialized view:

```text
analyst / Trino view       SELECT id, secret FROM sp4a10_wh.sp4a.gate_v ORDER BY id
                           → finished, [[1,'classified-one'],[2,'classified-two'],[3,'classified-three']]
analyst / StarRocks view   SELECT id, secret FROM sp4cgate.gate_v ORDER BY id
                           → finished, [[1,'classified-one'],[2,'classified-two'],[3,'classified-three']]
analyst / StarRocks MV     SELECT id, secret FROM sp4cgate.gate_mv ORDER BY id
                           → failed: engine: Server error: `ERROR 5204 (42000): Access denied; you need
                             (at least one of) the SELECT privilege(s) on COLUMN id for this operation.'
```

That last refusal is **a missing grant, not a clearance** — a StarRocks
materialized view is a real table in `default_catalog`, and `default_catalog` is
not a `registry.catalogs` row, so `perm.grants` (whose `Scope` requires a catalog
**uuid**) cannot express a grant on it. The obvious workaround, a hand-written
Ranger policy, is refused by egret itself:

```text
POST /api/control-plane/policies {resources: {catalog: default_catalog, database: sp4cgate, …}}
→ 409 conflict: profile 55 is compiled by the SP4a permission model — edit its grants and rules
  instead of writing a policy directly; the next recompile would silently overwrite this change
```

So the StarRocks **materialized-view** half of case 4 is **BLOCKED** on this
deployment and is reported as such below, rather than being read as a clearance
result. The StarRocks **view** half is measurable, and is measured.

---

## 2. The deploy, verified by the pod and not by helm's report

The image was rebuilt from this branch's tip (`259522a5`) and `kind load`ed.

```text
helm upgrade --install egret deploy/helm/egret -n egret-system --wait --timeout 12m \
  --set oidc.enabled=true --set ingress.enabled=true --set kernel.launcher=kubernetes \
  --set backend.replicas=2 --set sharedStore.enabled=true --set ranger.enabled=true
REVISION: 69   STATUS: deployed
```

| check | result |
|---|---|
| `helm get values` after the upgrade | all **six** user-supplied values, `ranger.enabled: true` among them |
| `kind load` | `Image: "egret-backend:dev" with ID "sha256:3796715e5314…" not yet present on node "egret-control-plane", loading...` |
| node's image (`crictl images`) | `docker.io/library/egret-backend  dev  a17bbc90ff081  87.5MB` |
| serving pods' `imageID` | `sha256:a17bbc90ff081556905ae063f1b52b2872863ecc68bcbf5d15e5e22254b0d90b` on **both** `egret-5bd56794cd-pm4mr` and `-qlkbt` — identical to that node image |
| `EGRET_RANGER__ENABLED` read from **inside** each running pod | `true`, `true` |
| `_sqlx_migrations` max | 157 |

**The plugins are actually enforcing, checked before any pass was trusted.** Not
`allow-all`, and not merely configured — a policy biting, on each engine, read as
output:

```text
/etc/trino/access-control.properties (read from inside the running coordinator)
  access-control.name=ranger
  ranger.service.name=sp3c2-trino-live
/opt/starrocks/fe/conf/fe.conf (read from inside the running FE)
  access_control = ranger

analyst / Trino      SELECT id, item, country FROM sp4a10_wh.sp4a.report ORDER BY id
                     → [[1,'xxxx','XX'], [3,'xxxx','XX']]   (SP4a's mask AND its row filter, both biting)
admin   / StarRocks  SELECT id, item FROM sp4a10_wh.sp4a.srcases ORDER BY id
                     → [[3,'xxxx'], [4,'xxxx']]             (mask + `id > 2` filter, biting even for admin,
                                                             because a mask is not an access check)
```

---

## 3. What the clearance compiled to

`PUT /api/control-plane/tags/3954a1a4-…/access-policy
{"required_roles":["sp4c_cleared"],"required_groups":[]}` → `200 {"ok":true}`.
The write's own recompile ran in the same transaction; nothing was forced.

| profile | policies before | policies after |
|---|---|---|
| 51 `sp3c2-trino-live_tag` (the Trino TAG service) | 2 | **4** |
| 52 `sp3c2-trino-live` | 31 | 31 |
| 55 `sp4a-sr-crosscheck` (StarRocks) | 34 | **35** |

Trino's clearance is one tag policy filed on the tag service (the second new row
on 51 belongs to a pre-existing gated tag, `tag`, which has no assignments);
StarRocks' is exactly **N = 1**, one per tagged column:

```text
8840 | 51 | perm-clearance-3954a1a4-386d-40fb-bdc8-e2b4f657f4fd
8841 | 55 | perm-clearance-3954a1a4-386d-40fb-bdc8-e2b4f657f4fd-sp4a10_wh.sp4a.gate.secret
```

The bodies, abbreviated to the parts D0/D0b made load-bearing — **the exception
names the member, never the role**:

```json
// 8840 (Trino, tag service)                    // 8841 (StarRocks, per column)
"resources": {"tag": {"values": ["sp4cgate"]}}  "resources": {"catalog":["sp4a10_wh"],"database":["sp4a"],
"serviceType": "tag"                                          "table":["gate"],"column":["secret"]}
"denyPolicyItems": [{"groups": ["public"],      "serviceType": "starrocks"
  "accesses": [select, insert, delete]}]        "denyPolicyItems": [{"groups": ["public"],
"denyExceptions":  [{"users": ["d3member"],       "accesses": [select]}]
  "accesses": [select, insert, delete]}]        "denyExceptions":  [{"users": ["d3member"],
"policyPriority": 0                               "accesses": [select]}]
```

`roles: []` in every item, on both engines — the Ranger roles bucket is unused,
exactly as D0 and D0b required. `select` alone on StarRocks, `select, insert,
delete` on Trino.

---

## Case 1 — the intersection property. **PASS**

A user without the clearing role cannot read the classified column, through SQL
Lab **or** through a notebook, on **either** engine. Verbatim:

**SQL Lab, `analyst` (no `sp4c_cleared`):**

```text
Trino      SELECT id, secret FROM sp4a10_wh.sp4a.gate ORDER BY id
           → failed: engine: Access Denied: Cannot select from columns [id, secret] in table or view gate

StarRocks  SELECT id, secret FROM sp4a10_wh.sp4a.gate ORDER BY id
           → failed: engine: Server error: `ERROR 5204 (42000): Access denied; you need (at least one of)
             the SELECT privilege(s) on COLUMN secret for this operation.'
```

**Notebook, `analyst`** — a real per-user kernel pod, `execute_request` over the
Jupyter channels WebSocket, the kernel's own helpers minting their own engine JWT:

```text
me = analyst
TRINO ERROR: RuntimeError Failed to execute sql: SELECT * FROM (SELECT id, secret FROM sp4a10_wh.sp4a.gate
  ORDER BY id) AS subquery LIMIT 10, error: (trino.exceptions.TrinoUserError) TrinoUserError(type=USER_ERROR,
  name=PERMISSION_DENIED, message="Access Denied: Cannot select from columns [id, secret] in table or view gate",
  query_id=20260928_070624_00030_nc3ii)
STARROCKS ERROR: RuntimeError Failed to execute sql: SELECT * FROM (SELECT id, secret FROM sp4a10_wh.sp4a.gate
  ORDER BY id) AS subquery LIMIT 10, error: (pymysql.err.OperationalError) (5204, 'Access denied; you need
  (at least one of) the SELECT privilege(s) on COLUMN secret for this operation.')
```

**The deny is scoped to the classified column, not the table** — the same subject,
the same table, the untagged column, in the same session:

```text
analyst / Trino      SELECT id, plain FROM sp4a10_wh.sp4a.gate ORDER BY id
                     → finished, [[1,'open-one'],[2,'open-two'],[3,'open-three']]
analyst / StarRocks  SELECT id, plain FROM sp4a10_wh.sp4a.gate ORDER BY id
                     → finished, [[1,'open-one'],[2,'open-two'],[3,'open-three']]
```

Before the clearance existed, both of these subjects read `secret` on both engines
through both paths (§1). So the refusal is the clearance and not an outage, a
missing grant, or a broken engine.

---

## Case 2 — the cleared user still reads it. **PASS**, and on StarRocks this is the first observation anywhere of `denyExceptions` working

D0b §5 was explicit: `denyExceptions` had **never been served to StarRocks**, none
of the 32 policies then cached carried one, and half of D0b's answer for that
engine rested on a source read of the shared Ranger evaluator rather than on any
query. Policy **8841** is the first `denyExceptions` that engine has ever held,
and this is the first time it has been observed deciding anything:

```text
d3member / Trino      SQL Lab   SELECT id, secret FROM sp4a10_wh.sp4a.gate ORDER BY id
                                → finished, [[1,'classified-one'],[2,'classified-two'],[3,'classified-three']]
d3member / StarRocks  SQL Lab   same query
                                → finished, [[1,'classified-one'],[2,'classified-two'],[3,'classified-three']]
d3member / notebook             me = d3member
                                TRINO ROWS:     {'id': [1,2,3], 'secret': ['classified-one','classified-two','classified-three']}
                                STARROCKS ROWS: {'id': [1,2,3], 'secret': ['classified-one','classified-two','classified-three']}
```

Same query, same engines, same minute, same deny policy — one subject refused and
the other served, the only difference being one `access_grants` row. That is what
makes case 1 a **clearance** and not an outage, and it is the observation D0b said
the gate owed.

---

## Case 4 — the view bypass. **FAILED on both engines. Measured Critical.**

D0b read in the StarRocks source that `checkViewPrivilege` and
`checkMaterializedViewAction` never descend to base-table columns, and said no
query had confirmed it and nobody had checked Trino. Both are now measured, and
**a view leaks the classified column in full on both engines.**

### 4a. StarRocks — the view leaks, outright

`default_catalog.sp4cgate.gate_v` is `SELECT id, secret, plain FROM
sp4a10_wh.sp4a.gate`. Read by `analyst`, who is refused the base column in the
same minute:

```text
analyst / StarRocks  SELECT id, secret FROM sp4a10_wh.sp4a.gate ORDER BY id
                     → failed: engine: Server error: `ERROR 5204 (42000): Access denied; you need
                       (at least one of) the SELECT privilege(s) on COLUMN secret for this operation.'

analyst / StarRocks  SELECT id, secret FROM sp4cgate.gate_v ORDER BY id
                     → finished, [[1,'classified-one'],[2,'classified-two'],[3,'classified-three']]
```

Every classified value, to a subject the clearance refuses. The column deny is
simply not consulted on this path — and note `analyst` holds **no** egret grant on
`default_catalog` at all, so this is not a grant that was too wide: the view
answers a subject with nothing.

### 4b. Trino — the view leaks whenever its OWNER is cleared

Trino behaves differently and is not safe either. Its view is DEFINER-rights, so
the base-table check is made against the **view owner**:

- With the owner (`admin`) **not** cleared, every reader is refused — including the
  cleared `d3member`, and including `admin`:
  ```text
  analyst  / Trino  SELECT id, secret FROM sp4a10_wh.sp4a.gate_v ORDER BY id
  d3member / Trino  (same query)
                    → failed: engine: Access Denied: View owner does not have sufficient privileges:
                      View owner 'admin' cannot create view that selects from gate
  ```
- Give the **owner** the clearing role and the same view hands the full classified
  column to the **uncleared** reader. `sp4c_cleared` was assigned to `admin`
  (`denyExceptions` recompiled to `["admin","d3member"]` on both policies within
  the poll), then, as `analyst`:
  ```text
  analyst / Trino  SELECT id, secret FROM sp4a10_wh.sp4a.gate_v ORDER BY id
                   → finished, [[1,'classified-one'],[2,'classified-two'],[3,'classified-three'],[91,'written-by-uncleared']]
  analyst / Trino  SELECT id, secret FROM sp4a10_wh.sp4a.gate ORDER BY id      (the base table, same minute)
                   → failed: engine: Access Denied: Cannot select from columns [id, secret] in table or view gate
  ```
  The role was then removed from `admin`; `denyExceptions` went back to
  `["d3member"]` and the view read returned to `View owner 'admin' cannot create
  view that selects from gate`.

**So on Trino a clearance is bypassed by any view owned by a cleared principal,
and simultaneously over-blocks — a cleared user cannot read through a view owned
by an uncleared one.** Both halves are the same mechanism and neither is what a
classification is supposed to do.

### 4c. The materialized views

| | result |
|---|---|
| Trino MV | **BLOCKED — cannot exist.** `CREATE MATERIALIZED VIEW sp4a10_wh.sp4a.gate_mv …` → `engine: createMaterializedView is not supported for Iceberg REST catalog` |
| StarRocks MV | **BLOCKED — not measurable on this deployment.** The MV exists and refreshes, but an uncleared reader is refused it for a reason that is not the clearance: `ERROR 5204 (42000): Access denied; you need (at least one of) the SELECT privilege(s) on COLUMN id for this operation.` An MV is a real table in `default_catalog`, which is not a `registry.catalogs` row, so `perm.grants` cannot express a read on it, and `POST /api/control-plane/policies` refuses a hand-written one: `409 conflict: profile 55 is compiled by the SP4a permission model`. A refusal that cannot be told from a clearance is not a measurement, so it is reported as BLOCKED rather than as a pass. |

**Verdict: case 4 FAILED.** The per-column approach does not achieve SP4c's goal
against a view, on either engine. This is reported as measured rather than
adjusted: the case was not narrowed to make it pass.

---

## Case 5 — StarRocks withholds reads only. **PASS as stated for StarRocks — and the same is now measured to be TRUE OF TRINO, which the UI says the opposite of.**

The uncleared `analyst`, against the column they cannot read:

```text
analyst / StarRocks  INSERT INTO sp4a10_wh.sp4a.gate VALUES (91,'written-by-uncleared','plain91')
                     → finished
d3member / StarRocks SELECT id, secret FROM sp4a10_wh.sp4a.gate WHERE id = 91
                     → finished, [[91, 'written-by-uncleared']]        (the write really landed)
analyst / StarRocks  SELECT id, secret FROM sp4a10_wh.sp4a.gate WHERE id = 91
                     → failed: engine: Server error: `ERROR 5204 (42000): Access denied; you need
                       (at least one of) the SELECT privilege(s) on COLUMN secret for this operation.'
```

So the system behaves exactly as the UI states for StarRocks: **a user without
clearance can overwrite a column they cannot read.** PASS.

**But the contrast the case was written to draw does not exist.** The same probe
on Trino, whose compiled deny carries `select, insert, delete`:

```text
analyst / Trino  INSERT INTO sp4a10_wh.sp4a.gate VALUES (92,'trino-write-by-uncleared','plain92')
                 → finished
d3member / Trino SELECT id, secret FROM sp4a10_wh.sp4a.gate WHERE id IN (91,92) ORDER BY id
                 → finished, [[92,'trino-write-by-uncleared'], [91,'written-by-uncleared']]
analyst / Trino  DELETE FROM sp4a10_wh.sp4a.gate WHERE id = 92
                 → finished       (and row 92 is gone: SELECT id … → [[91],[1],[2],[3]])
```

The uncleared subject **inserted and then deleted** through the clearance. The
`insert` and `delete` accesses in policy 8840 are dead weight for the same reason
D0b measured them to be dead on StarRocks — the tag is attached to a **column**
resource, while `checkCanInsertIntoTable` / `checkCanDeleteFromTable` present a
**table** resource, which the tagged resource does not match.

This falsifies the second sentence Task 5 put on `TagDetail`:

> "On a StarRocks engine this clearance withholds reads only … Trino withholds
> both reads and writes — this is a real asymmetry between engines, not a
> rounding error…"

**There is no asymmetry. Both engines withhold reads only.** The UI tells an
author that a Trino classification blocks writes; measured, it does not.

---

## Case 6 — a rename does not break it. **PASS**

Task 3 found that renaming a gated tag used to leave the deny naming the old tag,
which fails open. The Trino clearance addresses the tag by `tags.name`, so the
name is the field that matters (`PATCH /tags/{id}` carries `name`; `tags.key` is
not renameable through that route).

```text
PATCH /api/control-plane/tags/3954a1a4-… {"name":"sp4cgate2"}  → 200
```

The compiled policy and the tag download both moved in the same write:

```text
policy 8840  resources.tag.values  → ["sp4cgate2"]          (was ["sp4cgate"])
GET /service/tags/download/sp3c2-trino-live_tag
             tagVersion 10, tags {"1":{"type":"sp4cgate2"}}, resource 1 = sp4a10_wh/sp4a/gate/secret
classification_policies.required_roles → {sp4c_cleared}      (unchanged)
denyExceptions on 8840 and 8841        → ["d3member"]        (unchanged)
```

and the clearance still discriminates, after the plugins' next poll:

```text
analyst  / Trino      → failed: engine: Access Denied: Cannot select from columns [id, secret] in table or view gate
analyst  / StarRocks  → failed: engine: Server error: `ERROR 5204 (42000): Access denied; you need (at least one of)
                        the SELECT privilege(s) on COLUMN secret for this operation.'
d3member / Trino      → finished, [[91,'written-by-uncleared'],[1,'classified-one'],[2,'classified-two'],[3,'classified-three']]
d3member / StarRocks  → finished, same four rows
```

No fail-open. **PASS.**

---

## An extra finding, not one of the six: `explain` disagrees with the engine about a cleared user

Task 4 exists so `explain` names a clearance denial instead of reporting an access
the engine does not grant. Called the natural way — naming only the user, as the
endpoint's own `ExplainSubject` doc says a caller does — it is wrong in **both**
directions, because `ExplainSubject.roles` is caller-supplied and defaults to
empty while `workspaces` is resolved server-side:

```text
POST /api/control-plane/perm/explain {"subject":{"user":"d3member"},"action":"read",
     "target":{catalog sp4a10_wh, schema sp4a, table gate, column secret}}
→ {"allowed":false,"reasons":[{"kind":"clearance_required","tag":"sp4cgate","required_roles":["sp4c_cleared"]}]}
  …for the user the engine served the column to, in the same minute.

{"subject":{"user":"analyst","roles":["sp4c_cleared"]}, …}
→ {"allowed":true,"reasons":[{"kind":"allowed_by_grant","grant_id":"c11e6fd0-…"}]}
  …for the user the engine refuses. A caller asserting a role it does not hold is believed.
```

(With the role supplied truthfully, `{"user":"d3member","roles":["sp4c_cleared"]}`
→ `allowed: true` — so the evaluator's logic is right and only its **input** is
unresolved.) This is SP4a Task 15's D3 defect 2 exactly, on the field next to the
one that was fixed: a caller-supplied copy of a fact only egret can verify.
`explain` should resolve the subject's roles from `access_grants` the same way it
now resolves workspace membership. Reported here rather than fixed, because this
task is the gate.

---

## Case 3 — the clearance lifts, and the policy is reaped. **PASS**

```text
DELETE /api/control-plane/tags/3954a1a4-…/access-policy  → 200 {"ok":true}
```

| profile | before lift | after lift |
|---|---|---|
| 51 `sp3c2-trino-live_tag` | 4 | **3** |
| 52 `sp3c2-trino-live` | 31 | 31 |
| 55 `sp4a-sr-crosscheck` | 35 | **34** |

Both of this tag's clearance rows are gone — reaped by the delete's own recompile,
nothing forced. The only `perm-clearance-%` row left belongs to the other gated
tag, which this run never touched:

```text
select id, service_id, name from policy.authz_policies where name like 'perm-clearance-%'
8839 | 51 | perm-clearance-3205756d-543e-49f5-adfb-75f31112b5c2
select count(*) from classification_policies where tag_id = '3954a1a4-…'   →  0
```

And the reads come back, for the uncleared subject, on both engines and both paths:

```text
analyst  / Trino      SQL Lab  → finished, [[91,'written-by-uncleared'],[1,'classified-one'],[2,'classified-two'],[3,'classified-three']]
analyst  / StarRocks  SQL Lab  → finished, same four rows
d3member / Trino, StarRocks    → finished, same four rows
analyst  / Trino      view     → finished, same four rows   (the over-block of case 4b is gone too)
analyst  / notebook            → me = analyst
                                 TRINO ROWS:     {'id': [1,2,3,91], 'secret': ['classified-one','classified-two','classified-three','written-by-uncleared']}
                                 STARROCKS ROWS: {'id': [1,2,3,91], 'secret': [...same...]}
```

No one-way door. **PASS.**

(Two instrument notes from this step, both real and both cheap to hit: a kernel
pod older than its `EGRET_KERNEL_TOKEN` answers every engine call with
`egret: kernel token expired — restart the kernel`, and
`POST /notebooks/{id}/kernel/restart` does **not** fix it — it restarts the Jupyter
kernel inside the same pod. `DELETE /ws/{ws}/kernel` followed by `POST
/ws/{ws}/kernel` launches a new pod with a fresh token, which does.)
