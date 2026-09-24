# SP4a Task 10 — the live gate: both engines, and the evaluator against reality

**Measured:** 2026-09-24, kind context `kind-egret`, helm revision **62**, branch
`feat/sp4a-permission-model` (`c90a9dad`).
**Verdict:** the model, the store, the evaluator, the row filter, the path-selected mask and the
ceiling are **real and enforced live**. Two compiler defects were found that compile green, deploy
cleanly and enforce nothing — exactly the failure this task exists to catch. StarRocks live queries
were **blocked** by a pre-existing cluster defect unrelated to SP4a.

---

## 0. The deploy, verified by the pod and not by helm

```text
helm upgrade … --set oidc.enabled=true --set ingress.enabled=true --set kernel.launcher=kubernetes \
               --set backend.replicas=2 --set sharedStore.enabled=true --set ranger.enabled=true
REVISION: 62   STATUS: deployed
```

`helm get values egret -n egret-system` after the upgrade returned all **six** user-supplied values
(`ranger.enabled: true` among them). The five-flag command in the task brief would have dropped
`ranger.enabled` — the chart defaults it to `false` (`deploy/helm/egret/values.yaml:97-98`) — and
every engine would have denied every query while measuring nothing.

Verified after `kubectl rollout restart deploy/egret`:

| check | result |
|---|---|
| serving pods' `imageID` | `sha256:05abd4b4a94e…` on both `egret-7646c5bc96-pc6j6` and `-sq7pv`, identical to the node's `egret-backend:dev` (`crictl inspecti` → `05abd4b4a94e4`), which `kind load` reported as "not yet present, loading" minutes earlier |
| `EGRET_RANGER__ENABLED` read from inside both running pods | `true` |
| migrations | `_sqlx_migrations` max was **153** before the deploy and **155** after — 0154 and 0155 applied |
| new code actually serving | `GET /api/control-plane/perm/grants` → `200 []` (the route does not exist in the previous image) |

---

## 1. Instruments, and why they are not the ones the brief named

- **`demo-trino` was not used and not touched.** OOMKilled, 42 restarts against its own 1Gi limit.
  A ceiling measured on a coordinator that dies hourly would read a JVM cap as a real limit.
- **`sp3c2-trino-live`** (no memory limits) is the Trino instrument. At task start it carried
  `access-control.name=allow-all` and no `ranger-trino-security.xml`: it was provisioned ~19 h before
  `ranger.enabled` was switched on, and **it was enforcing nothing**. The branch's own late-bind path
  (`engines.rs` "Late-bind (R1)") fixed this during the run — see §2.
- **The catalog already attached to it could not be written to.** `sp3c_live_wh`'s storage row points
  at `s3.endpoint=http://minio:9000`, a name that does not resolve from the `egret-engines`
  namespace. Verbatim:
  ```text
  engine: Failed checking new table's location: s3://e2e/sp3c-live/01a0d2a9-…/report-…
  ICEBERG_FILESYSTEM_ERROR
  ```
  Pre-existing, not caused by this branch. A correctly addressed fixture was provisioned instead.

### Fixture (all additive)

| object | id |
|---|---|
| storage `sp4a10-store` (`http://minio.egret-system.svc.cluster.local:9000`) | `e9aca4e7-0f91-4e97-9949-298f4d753155` |
| warehouse `sp4a10_wh` | `5c0c23f7-7495-4791-b254-a883eedef66b` |
| registry catalog `sp4a10_wh` | `df341757-f78f-4a32-a524-8383352b4be8` |
| Trino engine `sp3c2-trino-live` | `5566fc50-471d-4972-ad20-63f07e056c41` |
| Ranger profile `sp3c2-trino-live` / tag service `sp3c2-trino-live_tag` | ids 52 / 51 |
| StarRocks profile `sp4a10-sr` | id 53 |
| tag `pii` | `2c789387-6e1a-492d-bd78-ea7d0f66951d` |

```sql
CREATE SCHEMA sp4a10_wh.sp4a;
CREATE TABLE  sp4a10_wh.sp4a.report  (id bigint, item varchar, country varchar);
INSERT INTO   sp4a10_wh.sp4a.report  VALUES (1,'book','US'),(2,'pen','VN'),(3,'lamp','US'),(4,'desk','VN');
CREATE TABLE  sp4a10_wh.sp4a.secret  (id bigint, note varchar);
INSERT INTO   sp4a10_wh.sp4a.secret  VALUES (1,'classified'),(2,'top-secret');
CREATE TABLE  sp4a10_wh.sp4a.sibling (id bigint, v varchar);
INSERT INTO   sp4a10_wh.sp4a.sibling VALUES (1,'alpha'),(2,'beta');
```

**The test subject is `analyst`, not `admin`.** The auto-bind seeds thirteen `egret-admin` policies
granting `admin` `select … write_sysinfo` on `*.*.*.*`; every case run as `admin` would have passed
without measuring anything. A second user `analyst@egret.local` was created, added to the `demo`
workspace, and given only the non-data policies a query needs at all (`execute` on `queryid`,
`impersonate` on `trinouser`, `alter` on `systemproperty`, `execute`/`grant` on `function`,
`read_sysinfo`). **No table, schema, column or catalog access was hand-written for `analyst`** —
every data grant in what follows comes from the `perm` model.

`SELECT current_user` as `analyst` → `analyst`. The bare username again, on a second engine (D7).

---

## 2. Ranger actually came on, and default-deny is real

The late-bind fires on `PUT /api/control-plane/engines/{id}`: profile `sp3c2-trino-live` (52) and tag
service `sp3c2-trino-live_tag` (51) were created and bound, and the coordinator rolled onto
`access-control.name=ranger`. Verbatim first denial, before any policy existed for `analyst`:

```text
engine: Access Denied: Principal analyst cannot become user analyst
```

and after the non-data policies, before any `perm` grant:

```text
SELECT id, item, country FROM sp4a10_wh.sp4a.report ORDER BY id
engine: Access Denied: Cannot access catalog sp4a10_wh
```

---

## 3. The eight cases

| # | case | engine | verdict |
|---|---|---|---|
| T1 | grant `read` on a **schema** → query succeeds | `Access Denied: Cannot select from columns [v, id] in table or view sibling` | **FAILED** |
| T1b | the same grant written down to `column:"*"` | 4 rows, raw values | PASS |
| T2 | `deny` on one table, siblings still work | `secret` refused, `report` returns all 4 rows | **PASS** |
| T3 | **tag** mask on a column → column comes back masked | `item` = `book`, `pen`, `lamp`, `desk` — **the real values** | **FAILED** |
| T3b | **path** mask on the same column | `item` = `xxxx` | **PASS** |
| T4 | row filter on a table | 2 rows, both `US`; `count(*)` = 2; `WHERE country='VN'` → **0 rows** | **PASS** |
| S1–S4 | the same four on StarRocks | engine has no compute node | **BLOCKED** |
| C | expansion ceiling refuses, never truncates | `would expand to 501 entries, over the ceiling of 500`, 0 policies written | **PASS** |

### T1 — FAILED. A grant above column level compiles cleanly and grants nothing

Model: `{effect: allow, actions:[read], scope:{catalog: sp4a10_wh, schema: sp4a}, principals:[user analyst]}`.
Compiled body, verbatim from `policy.authz_policies`:

```json
{ "name": "perm-grant-af874b33-…",
  "service": "sp3c2-trino-live",
  "resources": { "catalog": {"values":["sp4a10_wh"]}, "schema": {"values":["sp4a"]} },
  "policyType": 0,
  "policyItems": [ { "users": ["analyst"], "accesses": [{"type":"select","isAllowed":true}] } ],
  "serviceType": "trino" }
```

The catalog name is resolved correctly (a name, not a UUID — D6 holds). But with no `table` and no
`column` resource the Trino plugin does not match a column-granular `select`. Measured, after the
plugin poll:

```text
SELECT id, v FROM sp4a10_wh.sp4a.sibling ORDER BY id
engine: Access Denied: Cannot select from columns [v, id] in table or view sibling
```

A **table**-level grant (`catalog/schema/table`, no column) fails identically. The grant starts
enforcing only when the scope names `column: "*"` explicitly, at which point the same query returns
`[[1,"book","US"],[2,"pen","VN"],[3,"lamp","US"],[4,"desk","VN"]]`.

The denial *did* move from `Cannot access catalog` to `Cannot select from columns`, so the ancestor
match works for the catalog-level check and not for the column-level one. This is exactly the
assumption `grants.rs` flagged as unverified: *"the resource-hierarchy ANCESTOR match … has not been
checked against the live plugin."* It has now. It does not hold downward.

**Severity: Critical.** A steward writes "read on this schema", egret stores it, compiles it,
lists it as live, and the engine grants nothing. The fix belongs in the compiler
(`scope_to_resources` must fill the levels below the scope with `*` for an access policy), not in
the model and not in the test.

### T2 — PASS

With `allow read` on `report.*` and on `secret.*`, both tables read. Adding
`{effect: deny, actions:[read], scope:{…, table: secret, column:"*"}}`:

```text
SELECT id, note FROM sp4a10_wh.sp4a.secret ORDER BY id
engine: Access Denied: Cannot select from columns [note, id] in table or view secret

SELECT id, item, country FROM sp4a10_wh.sp4a.report ORDER BY id
[[1,"book","US"],[2,"pen","VN"],[3,"lamp","US"],[4,"desk","VN"]]
```

### T3 — FAILED. The tag-selected mask is served under the wrong service

Rule: `{kind: mask MASK, selector: {type: tag, key: pii}}`, with `sp4a.report.item` carrying `pii`.
The query, after the poll:

```text
SELECT id, item, country FROM sp4a10_wh.sp4a.report ORDER BY id
[[1,"book","US"],[2,"pen","VN"],[3,"lamp","US"],[4,"desk","VN"]]
```

**`item` came back as `book`, `pen`, `lamp`, `desk` — the literal real values.** A mask that does not
mask.

Localised precisely. The compiled body is correct Ranger:

```json
{ "name": "perm-rule-80337a00-…", "service": "sp3c2-trino-live_tag",
  "resources": { "tag": {"values":["pii"]} }, "policyType": 1, "serviceType": "tag",
  "dataMaskPolicyItems": [ { "groups": ["public"],
      "accesses": [{"type":"select","isAllowed":true}],
      "dataMaskInfo": {"dataMaskType":"MASK"} } ] }
```

but the **row** it lives in has `service_id = 52` (the *trino* profile), not `51` (the tag service
its own body names). `trigger.rs:596` inserts every desired policy with the profile's `service_id`
and never looks up the tag service's id. The consequence, read straight off the two Ranger
downloads:

```text
GET /service/plugins/policies/download/sp3c2-trino-live      → 22 policies, last one:
      perm-rule-80337a00-… | policyType 1 | serviceType tag
GET /service/plugins/policies/download/sp3c2-trino-live_tag  → serviceName sp3c2-trino-live_tag,
      policyVersion 1, 0 policies
```

The tag plugin's feed is empty; the resource plugin is handed a `tag`-typed, `tag`-keyed policy it
cannot match against a `catalog/schema/table/column` request. It deploys, it is listed, it does
nothing.

The tag *resource* feed is correct, so the fault is only the routing of the policy:

```json
GET /service/tags/download/sp3c2-trino-live_tag
{"serviceName":"sp3c2-trino-live_tag","tagVersion":3,"tags":{"1":{"type":"pii"}},
 "serviceResources":[
   {"id":1,"resourceElements":{"catalog":["sp4a10_wh"],"schema":["sp4a"],"table":["report"],"column":["country"]}},
   {"id":2,"resourceElements":{"catalog":["sp4a10_wh"],"schema":["sp4a"],"table":["report"],"column":["item"]}}],
 "resourceToTagIds":{"1":[1],"2":[1]}}
```

Tagging a **second** column (`country`) bumped `tagVersion` 2 → 3 and recompiled with no manual step
(Task 7's trigger works), and the newly tagged column came back raw too — `US`, not masked.

**Severity: Critical.** Same shape as T1: green tests, clean deploy, zero enforcement.

### T3b — PASS. The masking machinery itself is sound

The identical mask written as a **path** selector (`catalog/schema/table/column = …report.item`)
masks for real:

```text
SELECT id, item, country FROM sp4a10_wh.sp4a.report ORDER BY id
[[1,"xxxx","US"],[3,"xxxx","US"]]
```

`item` is the literal string `xxxx`. `book` and `lamp` are gone. This isolates T3 to the tag path
alone.

### T4 — PASS. The row filter removes rows, it does not merely reduce them

Rule `{kind: row_filter, filter_expr: "country = 'US'", selector: path …report}`:

```text
SELECT id, item, country FROM sp4a10_wh.sp4a.report ORDER BY id  → [[1,"xxxx","US"],[3,"xxxx","US"]]
SELECT count(*) FROM sp4a10_wh.sp4a.report                        → [[2]]
SELECT id, country FROM sp4a10_wh.sp4a.report WHERE country='VN'  → []
```

4 rows → 2. The explicit probe for the excluded rows returns **zero** rows, so they are genuinely
absent rather than merely fewer.

### S1–S4 — BLOCKED. StarRocks has no compute node

`sp3c2-sr-live` FE is up; its single CN has been in `CrashLoopBackOff` for 19 h, 27 restarts.
Verbatim, from `kubectl logs sp3c2-sr-live-cn-0`:

```text
[Thu Sep 24 09:44:23 UTC 2026] Add myself (sp3c2-sr-live-cn-0.sp3c2-sr-live-cn-search.egret-engines.svc.cluster.local:9050) into FE ...
ERROR 1045 (28000): Access denied for user 'root' (using password: NO)
ERROR 1045 (28000): Access denied for user 'root' (using password: NO)
[Thu Sep 24 09:44:23 UTC 2026] Time out, abort!
```

Cause, and it is **not** SP4a: every new StarRocks engine now gets a random `root` password
(`ensure_root_password_secret`, `engines.rs`) applied by the `initpwd` job, while the CN image's own
entrypoint registers itself with the FE as `root` with **no** password. The CN can never join. With
no compute node, no `SELECT` can run, so the four StarRocks query cases could not be measured.
Recorded as blocked rather than skipped; the remedy is a StarRocks-provisioning fix, not a
permission-model one.

What **was** measured on StarRocks — everything that does not need a running CN:

- The per-column expansion is real. With `sp4a10-sr` (type `starrocks`) bound to the engine and two
  tagged columns, the single tag rule compiled to **one policy per column**, named
  `perm-rule-80337a00-…-sp4a10_wh.sp4a.report.item` and `…report.country`, next to the four grant
  policies. This is D5's asymmetry, live.
- The full ceiling curve (§5).
- The ceiling refusal (§5).

### C — PASS. The ceiling refuses and does not truncate

At 501 tagged columns:

```json
{"error":"bad request: compiling rule \"t10-mask-pii\" would expand to 501 entries, over the ceiling of 500"}
```

and `SELECT count(*) FROM policy.authz_policies WHERE service_id=53` → **0**. Nothing was written,
nothing was truncated, and the message names both the rule and the count. Note the blast radius: the
refusal aborts the *whole* profile recompile, so the grants' policies go too. Fail-closed, but total.

---

## 4. The evaluator against the engine

`POST /api/control-plane/perm/explain` for each case, against the same model the engine was holding.

| # | subject / action / target | `explain` | engine | agree? |
|---|---|---|---|---|
| E1 | `analyst` read `sp4a.sibling.v`, schema-level grant present | `allowed: true`, `allowed_by_grant 42a57fd7` | `Access Denied: Cannot select from columns [v, id]` | **NO** |
| E2 | `analyst` read `sp4a.report` (no column named) | `allowed: false`, `no_grant_matched` | the `SELECT` succeeds | **NO** |
| E3 | `analyst` read `sp4a.report.item` | `allowed: true`, `masked_by_rule` ×2, `filtered_by_rule` | allowed, `item` = `xxxx`, 2 US rows | **YES** |
| E4 | `analyst` read `sp4a.report.country` (tag-masked only) | `allowed: true`, `masked_by_rule 80337a00 (MASK)`, `tags:["pii"]` | allowed, `country` = `US` — **not masked** | **NO** |
| E5 | `analyst` read `sp4a.secret` | `allowed: false`, `no_grant_matched` | denied | **YES** |
| E6 | `analyst` read `sp4a.secret.note` | `allowed: false`, `explicit_deny ea5109d4` | denied | **YES** |
| E7 | `analyst` **write** `sp4a.report.*` | `allowed: false`, `no_grant_matched` | `Access Denied: Cannot insert into table report` | **YES** |
| E8 | `admin` read `sp4a.report.item` | `allowed: false`, `no_grant_matched` | allowed (seeded `egret-admin` policy) | **NO** |

**Agreement: 4 of 8.** Each disagreement is reported, not resolved by adjusting the test.

- **E1 — the compiler is wrong, the evaluator is right.** This is T1 seen from the other side and is
  the single most useful result in this document: the two halves D8 asks for disagree, and the
  disagreement localises the defect to the compiler in one step.
- **E4 — the compiler is wrong, the evaluator is right.** T3 from the other side. `explain` even
  reports `tags: ["pii"]`, so a steward can see the tag resolved and the rule matched — and the
  engine still returned the real value.
- **E2 — the evaluator is stricter than the engine.** A grant scoped to `column:"*"` does not make
  `explain` allow the *table*. Whether `read` on a table is answerable at all when grants are written
  at column granularity is a semantic question SP4b's UI will ask constantly; today the two answers
  differ. (Caveat, stated because it matters: Trino has no table-level `select` a query can trigger
  without naming columns, so the engine column is the same `SELECT` as E3.)
- **E8 — `explain` is blind to hand-written policy.** It evaluates the `perm` model only. Every
  decision an engine makes from a `policy.authz_policies` row that the compiler did not write — which
  today includes the thirteen auto-seeded `egret-admin` policies on *every* profile — is invisible to
  it, and `explain` confidently answers "denied" for a user the engine allows. Not a bug in the
  evaluator's own terms; a real gap between "explain a decision" and "explain *the* decision".

---

## 5. The ceiling, measured

Method: N columns of `sp4a10_wh.sp4a.report` carrying `pii`, one tag-mask rule, the StarRocks profile
`sp4a10-sr` (id 53) emptied and fully rebuilt via `POST /api/control-plane/perm/compile/53` each
time; the download is a full `lastKnownVersion=-1` fetch of
`/service/plugins/policies/download/sp4a10-sr`, which is what every plugin poll pulls.

| tagged columns N | policies in profile | compile (wall clock) | download bytes | download time |
|---|---|---|---|---|
| 10 | 16 | 0.054 s | 22,491 | 0.032 s |
| 50 | 56 | 0.081 s | 51,691 | 0.030 s |
| 100 | 106 | 0.159 s | 88,191 | 0.038 s |
| 250 | 256 | 0.226 s | 197,691 | 0.039 s |
| 500 | 506 | 0.489 s | 381,467 | 0.057 s |
| 500 × 2 rules | 1006 | 0.653 s | 748,197 | 0.094 s |
| 500 × 3 rules | 1506 | 1.331 s | 1,114,698 | 0.113 s |
| 500 × 4 rules | 2006 | 2.224 s | 1,481,198 | 0.142 s |

Every download served the exact policy count the profile held, so nothing is being dropped on the
way out.

**What the numbers say.** The download is linear at ~740 bytes per policy and is never the binding
constraint: 1.48 MB comes back in 142 ms, and a 30 s poll interval makes that ~50 KB/s per plugin.
**Compile time is what bends** — sub-linear to ~500 policies, linear to ~1000, then super-linear:
1006 → 2006 policies costs 3.4× the time, not 2×. The knee is around **1000 policies in one
profile**, and every tag write on a bound warehouse pays it.

**The ceiling stays 500, and it is now a measured 500.** Not because 500 hurts — it does not, at
0.49 s and 381 KB — but because the cap is *per rule* and has to leave room for the profile's other
rules before the profile total reaches the knee. One rule at the cap plus its neighbours is exactly
where the curve is still linear. Written into `DEFAULT_EXPANSION_CEILING`'s doc comment and into D5
of `docs/en/specs/2026-09-24-egret-permission-model-sp4a-design.md`, both with this table.

**And the measurement exposes a gap in the ceiling itself: it does not bound the thing that costs.**
Four rules of 500 columns compiled **2006 policies with no refusal at all** — past the knee, at 2.2 s
per recompile. The per-rule cap cannot see the profile total. A per-profile ceiling is the follow-up.

---

## 6. Everything else this run turned up

1. **A tag-rule mutation recompiles every Trino and StarRocks profile in the installation.**
   Documented behaviour, but its first live consequence is worth stating: creating one tag rule wrote
   a policy into `demo-trino`'s profile — an engine this work was told not to touch. Verified back to
   its original **15** policies afterwards.
2. **The OpenAPI document is wrong about `NewGrant.principals`.** `#/components/schemas/Principal`
   resolves to `egret-policy`'s picker type (`{kind, name, label}`), not `egret-perm`'s wire type
   (`{kind, ref}`). Two distinct Rust types share one `ToSchema` name and one overwrites the other,
   so a client generated from the committed document cannot construct a grant. The endpoint itself
   is correct; the schema is not.
3. **There is no API to bind a StarRocks engine to a policy profile after creation.** Trino got a
   late-bind path in this branch (`engines.rs`, "Late-bind (R1)"); StarRocks binds only from
   `authz_service_id` at create time. Binding `sp3c2-sr-live` for this measurement required a direct
   `UPDATE engine_mgmt.engines SET authz_service_id = 53`.
4. **`sp3c2-trino-live` was enforcing nothing until this run** — `allow-all`, 19 h old, provisioned
   before `ranger.enabled` was switched on. The branch's late-bind fixed it on the first `PUT`. This
   is the same shape D7 recorded for the whole cluster and is worth an operator-visible signal: an
   engine whose profile is unbound is an engine with no access control.

## 7. What was not run

- The four StarRocks query cases (§3, S1–S4) — no compute node, verbatim cause recorded.
- `write` beyond the single `INSERT` probe of E7. SP4a's `write` → `insert,delete` mapping on Trino
  was exercised only as a denial.
- Group principals. D7 already settled this by mechanism (no group provider exists); nothing here
  re-opened it.
