# SP4c live result — the intersection property, measured

**Measured:** 2026-09-28, kind context `kind-egret`, branch
`feat/sp4c-classification-clearance`, worktree `egret-sp4a`.

**The acceptance test is one sentence: a user without clearance cannot read a
tagged column, by any path.** Not that a policy was written. Every verdict below
is engine output, pasted verbatim. Each case was written into this document as it
finished; a case that could not be run says so, with what was tried and what came
back. Nothing here is inferred from a policy row.

`demo-trino` was not used and not modified.

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
