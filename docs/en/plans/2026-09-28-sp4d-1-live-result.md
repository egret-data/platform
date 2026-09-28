# SP4d-1 Task 8 — the live gate: the evaluator against a real engine

> **Measured:** 2026-09-28 (UTC; the host's calendar says 2026-09-29 +07), kind context
> `kind-egret`, branch `feat/sp4d-ranger-evaluator`, worktree `egret-sp4a`.
> Canonical (English).

## Why this gate exists, and what it can and cannot see

Task 7's differential gate runs the same policy set and request through the real Java
`RangerPolicyEngineImpl` and through this crate. It is green over 10 000 generated cases at
two plugin versions and it found three real defects. It closes **one** of two gaps:

- **Gap A (Task 7):** the Rust evaluator disagrees with `RangerPolicyEngineImpl`.
- **Gap B (this document):** the Java engine *as a harness constructs it* disagrees with the
  engine *as Trino and StarRocks construct and call it*. Request construction, access-type
  naming, resource keys and column granularity all live in the plugin, and the harness cannot
  see any of it — it builds its own requests.

Gap B is not hypothetical. SP4a measured it: a grant compiled to `{catalog, schema}` produced
`Access Denied: Cannot select from columns [v, id] in table or view sibling` on a real Trino,
while the same grant with `column: "*"` returned rows
(`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3, case T1).

**The method, and it is the point.** For every case the evaluator's answer was computed and
written down FIRST, from the exact `ServicePolicies` download the plugin was holding, and only
then was the query run. A prediction made after seeing a result proves nothing. The prediction
tool is committed as `crates/egret-ranger-eval/examples/live_gate.rs`, so every line below is
reproducible.

**A disagreement would be a finding, not a test to adjust.** There were none on the access
decision; the findings below are of a different kind.

---

## Verdict

**Numbering, because it does not match the brief one-for-one.** The task brief's case list ends
at seven, whose seventh is the fallback probe; here that probe is **case 8**, so that **case 7**
can be the StarRocks tag-fetch question the package brief raised separately (§7). Nothing was
dropped: brief 1-6 are cases 1-6 (with 5 split into 5a and 5b, because Ranger's bucket for
cancelling a deny is `denyExceptions` and the brief's wording named the other one), brief 7 is
case 8, and cases 7, S1 and S2 are the additions.

| # | case | prediction (made first) | engine | agree? |
|---|---|---|---|---|
| 1 | `{catalog, schema}` policy vs a column-granular `SELECT` (the T1 shape) | `Denied { NoPolicyMatched }` | `Access Denied: Cannot select from columns [item, id] in table or view t1` | **YES** |
| 2 | the same written down to `column: ["*"]` | `Allowed { 8850 }` | 4 rows, raw values | **YES** |
| 3 | column mask, read back | `Allowed { 8851 }` + `Masked(maskType MASK)` | `item` = `xxxx / xxx / xxxx / xxxx` | **YES** |
| 4 | row filter, counted | `Allowed { 8853 }` + `Filtered("country = 'US'")` | 2 rows, both `US`; `count(*)` = 2 | **YES** |
| 5a | deny cancelled by a `denyException` | `Allowed { 8855 }` | 4 rows | **YES** |
| 5b | allow cancelled by an `allowException` | `Denied { NoPolicyMatched }` | `Access Denied: Cannot select from columns [item, id] in table or view t5b` | **YES** |
| 6 | grant to group `public`, requester in no groups | `Allowed { 8857 }` | 4 rows | **YES** |
| 7 | does StarRocks' Ranger plugin fetch tags at all? | — (an observation, not a prediction) | **it does not, and the cause is egret's own service-def** | see §7 |
| 8 | a Ranger-undetermined request — does Trino fall through? | `Denied { NoPolicyMatched }` | `Access Denied: Cannot select from columns [item, id] in table or view t7` | **YES** |
| S1 | StarRocks, the ANCESTOR shape | **none — there is nothing to predict from** | **NOT MEASURED** — §9 | — |
| S2 | StarRocks, `column: ["*"]` | **none — same reason** | **NOT MEASURED** — §9 | — |

**Eight of eight Trino cases agree.** The two StarRocks cases were **not measured and not
predicted**, and the second half of that is not an omission: a prediction is computed from a
policy download, and neither shape could be got into one. §9 obstacle 1 is why no policy could be
written to the profile the FE polls, and obstacle 2 is why the ANCESTOR shape is no longer
producible through the permission model either — so there is no download in which S1 exists to be
predicted from. A later run has to create the policy first; §9 says what that costs.

What §8 *does* hold is a different thing, and it is not a substitute: predictions for six
resources that already exist in the live StarRocks policy set. They were computed and written
down before any StarRocks query was attempted, they are the reason F2 was found, and they are
worth re-using — but none of them is the ANCESTOR or the column-wildcard shape.

Three findings came out of the run that no case asked for:

- **F1 — the evaluator answers `Undecidable` for a profile whose bound tag service actually
  exists** — today exactly one, and it is the only engine-facing Trino profile enforcing
  anything here. §6.
- **F2 — egret's rule compiler emits pairs of same-priority mask / row-filter policies whose
  evaluation order Ranger does not define**, and the evaluator refuses them as
  `EvalOrderAmbiguous` rather than guessing. The two MASKS here AGREE (both
  `dataMaskType: "MASK"`, no `valueExpr`); only the two ROW FILTERS carry different expressions
  (`country = 'US'` vs `id > 2`). §8.
- **F3 — "StarRocks has no tag policies" is true as egret deploys it, but the cause is egret's
  `starrocks` service-def carrying `contextEnrichers: []`**, not Ranger 2.8.0 and not
  StarRocks. §7.

---

## 1. Instruments, and the ones that were rejected

### Which database each step touched

- **The live cluster's own Postgres** (`postgres-8675b8486-xn4vx`, database `egret` in namespace
  `egret-system`) — read-only, for the policy/service inventory quoted below, and written only
  through egret's own admin HTTP API (`POST /api/control-plane/policies{,/services}`), never by
  hand-written SQL.
- **No test database was involved.** `egret_test` is the workspace test DB and nothing here runs
  `cargo test`; the crate's own suites are unchanged by this task.

### The engine

`demo-trino` was **not used and not modified** — 26 endpoints, alerts and datasets depend on it.

The live coordinator `sp3c2-trino-live` was not repointed either. Its Ranger profile
(`sp3c2-trino-live`, id 52) has `origin = 'generated'`, and both obstacles that follow from that
are recorded because they shape the whole run:

1. Its download carries a non-empty `tagPolicies` block (it has a bound tag service), which makes
   every evaluator answer `Undecidable` — see **F1**, §6.
2. `POST /api/control-plane/policies` refuses to write to any `generated` profile, this one
   included. The guard is `refuse_if_generated` (`crates/egret-policy/src/store.rs:178`), which
   reads `policy.authz_services.origin` and returns `CompilerOwned`; it does not consult the
   service type, so trino and starrocks profiles take the same path. **No write against profile
   52 was attempted, so no 409 for 52 is quoted anywhere here.** The one verbatim refusal this
   run produced was against profile **54** (`sp3c2-sr-live`) and is quoted, with its attribution
   spelled out, in §9.

So the Trino instrument is a **private, additive coordinator** created for this run, which
modifies nothing shared: pod + ConfigMap `sp4d1-gate-trino` in `egret-engines`, image
`trinodb/trino:470` (the same image, hence the same `ranger-plugins-common-2.5.0` Task 1
measured), `access-control.name=ranger`, `ranger.service.name=sp4d1-gate`, HTTP with no
authentication so the session user is chosen freely, and the `sp4a10_wh` Iceberg catalog
properties copied verbatim from `sp3c2-trino-live-catalogs`. The full manifest is in
`docs/en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md` §7.

The pod was deleted after the run; the profile, its policies and the fixture tables were left in
place so the gate can be re-run.

### The subject

`admin` — which is normally the wrong choice, because auto-bind seeds thirteen `egret-admin`
policies granting `*.*.*.*` on every *generated* profile. On `sp4d1-gate` there are none: it was
created through `POST /api/control-plane/policies/services` with `origin = 'manual'`, so nothing
was seeded and every policy the plugin sees is listed in §3. `admin` is therefore an ordinary
subject here, with no data grant except the one each case writes.

Measured, not assumed: `SELECT current_user` → `"admin"`. Groups are empty (Trino has no group
provider configured), which is what makes case 6 meaningful.

### The fixture

Created through the egret query API as `admin` against the live `sp3c2-trino-live` database
*before* the private coordinator existed, into the Iceberg warehouse `sp4a10_wh`:

```sql
CREATE SCHEMA sp4a10_wh.sp4d1;
CREATE SCHEMA sp4a10_wh.sp4d1a;
CREATE TABLE  sp4a10_wh.sp4d1a.t1  (id bigint, item varchar, country varchar);
CREATE TABLE  sp4a10_wh.sp4d1.t2   (…); t3; t4; t5a; t5b; t6; t7   -- same three columns
INSERT INTO   <each>  VALUES (1,'book','US'),(2,'pen','VN'),(3,'lamp','US'),(4,'desk','VN');
```

One table per case, so no case can be read as the side effect of another. Case 1 lives in its own
schema `sp4d1a` because its policy names `{catalog, schema}` and would otherwise be an ancestor
of case 8's table too, confounding the two.

**The fixture carries no tags.** `GET /service/tags/download/sp3c2-trino-live` lists six tagged
resources, all under schema `sp4a` (`gate.secret`, `report.country`, `report.item`, `sibling.v`,
`srcases.country`, `srcases.item`); none is in `sp4d1` or `sp4d1a`. This matters for §6.

---

## 2. How the plugin was confirmed to be holding the policy set that was predicted from

The plugin polls egret (`ranger.plugin.trino.policy.rest.url` → the egret service); on the live
coordinator that poll is every 30 s. Measuring before a poll lands records the previous state and
produces a false result. Two independent confirmations were taken, and the second is the one that
counts:

1. `GET /service/plugins/policies/download/sp4d1-gate` returned `policyVersion 17` with 16
   policies and **no `tagPolicies` key**. This is the byte-for-byte input the prediction was
   computed from.

   **The timeline, stated precisely, because the two clocks below do not line up on their own.**
   This fetch was issued before the private coordinator existed — with `kubectl exec` into the
   *live* `sp3c2-trino-live` coordinator, used as nothing but a pod that can reach the egret
   service on the cluster network. Nothing of that pod's configuration was read for it or changed
   by it. The pod that ran the queries was created afterwards, at **18:21:52Z** (its own
   `Bootstrap` log line), which is why its cache file's mtime is **18:21:56Z** — the plugin's
   *first* download at start-up, not a later refresh. A reader reconciling 18:14:17Z (prediction)
   against 18:21:56Z (cache) would otherwise reasonably suspect the cache had moved underneath
   the prediction. It had not: profile 56 has sat at `policyVersion 17` since the last policy was
   written at ~18:13Z, no policy was written after the pod existed, and the two fetches returned
   the same version and the same 16 policy ids.
2. The plugin's **own on-disk cache**, read out of the running pod:

   ```text
   /tmp/ranger/sp4d1-gate/policycache/trino_sp4d1-gate.json
   {"serviceName":"sp4d1-gate","serviceId":56,"policyVersion":17,"policies":[{"id":8842,…
   ```

   `policyVersion 17` — the same version — and the file's mtime
   (`2026-09-28 18:21:56.375 +0000`) precedes every query below. It was re-read unchanged after
   the last case. The plugin never saw a different policy set during the run, because every
   policy was written before the pod was created and none was written after.

---

## 3. The policy set, verbatim

Seven baseline policies grant `admin` only what a query needs before it touches data — `execute`
on `queryid`, `impersonate` on `trinouser`, `alter` on `systemproperty` and on
`catalog+sessionproperty`, `execute`/`grant` on `function`, `read_sysinfo` on `sysinfo`, and
`use`/`show` on `catalog: *`. **No baseline policy grants `select` on anything**, so every
allow below comes from that case's own policy. They are ids 8842-8848 and are omitted here for
length; they are visible in `GET /service/plugins/policies/download/sp4d1-gate`.

The nine case policies, as stored (empty lists and defaults elided):

```json
{"id":8849,"name":"sp4d1-c1-ancestor","policyType":0,
 "resources":{"catalog":{"values":["sp4a10_wh"]},"schema":{"values":["sp4d1a"]}},
 "policyItems":[{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}]}]}

{"id":8850,"name":"sp4d1-c2-colwild","policyType":0,
 "resources":{"catalog":{"values":["sp4a10_wh"]},"schema":{"values":["sp4d1"]},
              "table":{"values":["t2"]},"column":{"values":["*"]}},
 "policyItems":[{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}]}]}

{"id":8851,"name":"sp4d1-c3-access","policyType":0,
 "resources":{… "table":{"values":["t3"]},"column":{"values":["*"]}},
 "policyItems":[{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}]}]}

{"id":8852,"name":"sp4d1-c3-mask","policyType":1,
 "resources":{… "table":{"values":["t3"]},"column":{"values":["item"]}},
 "dataMaskPolicyItems":[{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}],
                         "dataMaskInfo":{"dataMaskType":"MASK"}}]}

{"id":8853,"name":"sp4d1-c4-access","policyType":0,
 "resources":{… "table":{"values":["t4"]},"column":{"values":["*"]}},
 "policyItems":[{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}]}]}

{"id":8854,"name":"sp4d1-c4-filter","policyType":2,
 "resources":{"catalog":{"values":["sp4a10_wh"]},"schema":{"values":["sp4d1"]},
              "table":{"values":["t4"]}},
 "rowFilterPolicyItems":[{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}],
                          "rowFilterInfo":{"filterExpr":"country = 'US'"}}]}

{"id":8855,"name":"sp4d1-c5a-deny-cancelled","policyType":0,
 "resources":{… "table":{"values":["t5a"]},"column":{"values":["*"]}},
 "policyItems":     [{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}]}],
 "denyPolicyItems": [{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}]}],
 "denyExceptions":  [{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}]}]}

{"id":8856,"name":"sp4d1-c5b-allow-cancelled","policyType":0,
 "resources":{… "table":{"values":["t5b"]},"column":{"values":["*"]}},
 "policyItems":     [{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}]}],
 "allowExceptions": [{"users":["admin"],"accesses":[{"type":"select","isAllowed":true}]}]}

{"id":8857,"name":"sp4d1-c6-public","policyType":0,
 "resources":{… "table":{"values":["t6"]},"column":{"values":["*"]}},
 "policyItems":[{"users":[],"groups":["public"],
                 "accesses":[{"type":"select","isAllowed":true}]}]}
```

A note on case 5. The brief says "a deny that an allow-exception cancels"; in Ranger the bucket
that cancels a deny is `denyExceptions`, and `allowExceptions` cancels an allow. Both were
written, as **5a** and **5b**, rather than picking one reading.

Table `t7` (case 8) has **no policy of any kind**. That is the case.

---

## 4. The prediction, made before any query ran

Run at 2026-09-28T18:14:17Z, against the download captured in §2, and pasted here verbatim. The
first query ran at 18:22:25Z (Trino's own query id `20260928_182225_00001_mb5pu` carries the
timestamp), eight minutes later.

```text
download   : …/sp4d1-gate-dl.json
service    : sp4d1-gate
policyVer  : Some(17)
engine     : Trino / Ranger2_5_0
user       : admin  (groups {}, roles {})
unmodelled : []
defIssues  : []

── case 1  ANCESTOR  {catalog,schema} policy vs column-granular select
   request  RequestResource { values: {"catalog": "sp4a10_wh", "column": "item", "schema": "sp4d1a", "table": "t1"} }
   access   Denied { policy_id: None, policy_name: "", reason: NoPolicyMatched }

── case 2  column wildcard
   request  RequestResource { values: {"catalog": "sp4a10_wh", "column": "item", "schema": "sp4d1", "table": "t2"} }
   access   Allowed { policy_id: Some(8850), policy_name: "sp4d1-c2-colwild" }

── case 3  mask on column `item`
   request  RequestResource { values: {"catalog": "sp4a10_wh", "column": "item", "schema": "sp4d1", "table": "t3"} }
   access   Allowed { policy_id: Some(8851), policy_name: "sp4d1-c3-access" }
   mask     Masked(MaskResult { policy_id: Some(8852), policy_name: "sp4d1-c3-mask", mask_type: Some("MASK"), masked_value: None, mask_condition: None })

── case 4  row filter (TABLE granularity)
   request  RequestResource { values: {"catalog": "sp4a10_wh", "schema": "sp4d1", "table": "t4"} }
   access   Allowed { policy_id: Some(8853), policy_name: "sp4d1-c4-access" }
   filter   Filtered(RowFilterResult { policy_id: Some(8854), policy_name: "sp4d1-c4-filter", filter_expr: "country = 'US'" })

── case 5a deny cancelled by a denyException
   request  RequestResource { values: {"catalog": "sp4a10_wh", "column": "item", "schema": "sp4d1", "table": "t5a"} }
   access   Allowed { policy_id: Some(8855), policy_name: "sp4d1-c5a-deny-cancelled" }

── case 5b allow cancelled by an allowException
   request  RequestResource { values: {"catalog": "sp4a10_wh", "column": "item", "schema": "sp4d1", "table": "t5b"} }
   access   Denied { policy_id: None, policy_name: "", reason: NoPolicyMatched }

── case 6  grant to group `public`, requester in no groups
   request  RequestResource { values: {"catalog": "sp4a10_wh", "column": "item", "schema": "sp4d1", "table": "t6"} }
   access   Allowed { policy_id: Some(8857), policy_name: "sp4d1-c6-public" }

── case 8  no policy names this table at all (fallback probe)
   request  RequestResource { values: {"catalog": "sp4a10_wh", "column": "item", "schema": "sp4d1", "table": "t7"} }
   access   Denied { policy_id: None, policy_name: "", reason: NoPolicyMatched }
```

Note case 4's request: it is issued at **table** granularity, with no `column`. Both engines'
`rowFilterDef.resources` stop at `table` and name no `column`, so a column-granular request makes
every row-filter policy an ANCESTOR match and yields no filter at all. Task 6 lost six tests to
this before spotting it; a live gate that got it wrong would read as "row filters do not work".

---

## 5. The measurement, verbatim

Every query below was run inside the private coordinator as
`trino --server localhost:8080 --user admin --execute "<sql>"`. Output is the CLI's, unedited
apart from the JLine terminal warning every invocation prints.

### Case 1 — the T1 shape. **PASS (they agree, and they agree on a denial)**

```text
SELECT id, item FROM sp4a10_wh.sp4d1a.t1 ORDER BY id
Query 20260928_182225_00001_mb5pu failed: Access Denied: Cannot select from columns [item, id] in table or view t1
```

A policy naming `{catalog, schema}` and granting `select` grants nothing against a
column-granular `SELECT`. This is SP4a's T1 reproduced on a different coordinator, against a
hand-written policy rather than a compiled one — so the shape, not the compiler, is what
produces it. The evaluator predicted exactly this, from
`resource::MatchType::Ancestor` not being an allow.

### Case 2 — the same, written down to `column: ["*"]`. **PASS**

```text
SELECT id, item FROM sp4a10_wh.sp4d1.t2 ORDER BY id
"1","book"
"2","pen"
"3","lamp"
"4","desk"
```

Cases 1 and 2 together are what make the measurement *sensitive* rather than uniformly denying:
the only difference between them is the `table` + `column: "*"` levels.

### Case 3 — column mask, read back. **PASS**

```text
SELECT id, item FROM sp4a10_wh.sp4d1.t3 ORDER BY id
"1","xxxx"
"2","xxx"
"3","xxxx"
"4","xxxx"
```

The evaluator reports the mask as DATA — `maskType = "MASK"`, no `maskedValue`, no
`maskCondition` — and not as a verdict, because
`RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult` reports allow unconditionally. The
engine is what turns `MASK` into `xxxx`; note `pen` → `xxx`, three characters for three, which is
the transform Trino applies and not something the policy says. The two halves are consistent:
the evaluator names the mask, the engine performs it.

### Case 4 — row filter, counted. **PASS**

```text
SELECT id, item, country FROM sp4a10_wh.sp4d1.t4 ORDER BY id
"1","book","US"
"3","lamp","US"

SELECT count(*) FROM sp4a10_wh.sp4d1.t4
"2"
```

Predicted `filter_expr: "country = 'US'"`; two rows came back, both `US`, and `count(*)` = 2.

### Case 5a — a deny cancelled by a `denyException`. **PASS**

```text
SELECT id, item FROM sp4a10_wh.sp4d1.t5a ORDER BY id
"1","book"
"2","pen"
"3","lamp"
"4","desk"
```

One policy carrying an allow item, a deny item and a deny exception, all naming the same user and
the same access. Ranger's precedence is deny-first, exception-cancels-deny, then allow. Net:
allowed, on both sides.

### Case 5b — an allow cancelled by an `allowException`. **PASS**

```text
SELECT id, item FROM sp4a10_wh.sp4d1.t5b ORDER BY id
Query 20260928_182255_00007_mb5pu failed: Access Denied: Cannot select from columns [item, id] in table or view t5b
```

The mirror image, and the reason both readings of "case 5" were written. `allowExceptions` is not
a deny — the evaluator reports `NoPolicyMatched`, not `DenyPolicyItem`, and the engine's message
is the same "cannot select" it gives for a table with no policy at all. That the two *reasons*
differ while the outcome is identical is exactly why `DenyReason` is kept as a distinct field.

### Case 6 — a grant to group `public`, against a user in no groups. **PASS**

```text
SELECT id, item FROM sp4a10_wh.sp4d1.t6 ORDER BY id
"1","book"
"2","pen"
"3","lamp"
"4","desk"
```

The policy names no user at all, only `groups: ["public"]`. Ranger's
`RangerPolicyEngine.GROUP_PUBLIC` matches every requester regardless of their actual groups
(`policyItem.getGroups().contains(GROUP_PUBLIC) || !disjoint(...)`), and that is what the engine
did for a requester Trino gives no groups to.

### Case 8 — a Ranger-undetermined request. **PASS, and it answers the open question**

```text
SELECT id, item FROM sp4a10_wh.sp4d1.t7 ORDER BY id
Query 20260928_182300_00009_mb5pu failed: Access Denied: Cannot select from columns [item, id] in table or view t7
```

Task 5 kept `Denied { NoPolicyMatched }` as a distinct reason because
`RangerPolicyEngineImpl` only marks such a result `isAccessDetermined` when
`getIsFallbackSupported()` is false, and it did **not** verify how the Trino plugin consumes the
flag. Measured: **the request is denied.** The label is right and the evaluator is not wrong in
the fail-closed direction.

The claim is scoped to what was measured, and the scope matters:
`access-control.config-files` names exactly one file, so the Ranger plugin is the only system
access control installed. Two things are therefore *not* claimed. First, that Trino has no
fallback mechanism at all — only that no fallback fired here. Second, that adding a second
access-control file would change it: Trino consults every configured system access control and
requires all of them to permit, so a second one can only subtract, never turn this deny into an
allow. The plugin's own startup dump on the same pod carries no fallback flag either:

```text
org.apache.ranger.authorization.hadoop.config.RangerPluginConfig
PolicyEngineOptions: { evaluatorType: auto, evaluateDelegateAdminOnly: false,
disableContextEnrichers: false, disableCustomConditions: false,
disableTagPolicyEvaluation: false, disablePolicyRefresher: false, disableTagRetriever: false,
disableUserStoreRetriever: false, enableTagEnricherWithLocalRefresher: false,
enableUserStoreEnricherWithLocalRefresher: false, disableTrieLookupPrefilter: false,
optimizeTrieForRetrieval: false, cacheAuditResult: false, disableRoleResolution: true,
optimizeTrieForSpace: false, optimizeTagTrieForRetrieval: false, optimizeTagTrieForSpace: false }
```

A rejected instrument, recorded because it looked like a second probe and is not one:
`SHOW ROLES FROM sp4a10_wh` returned `This connector does not support roles` — a connector
limitation reached before any authorization check, so it discriminates nothing.

---

## 6. F1 — the evaluator answers `Undecidable` for a profile whose bound tag service exists

This was not a case; it is what made the private coordinator necessary, and it is a live
constraint on SP4d-2.

`GET /service/plugins/policies/download/sp3c2-trino-live` — the real, engine-facing download —
carries a `tagPolicies` block: a whole second policy set (service `sp3c2-trino-live_tag`,
`policyVersion 22`, three policies: a tag mask on `pii`, a tag row-filter on `pii`, and a
clearance access policy). `crates/egret-ranger-eval/src/model.rs` reports any non-null
`tagPolicies` as `Unmodelled::TagPolicies`, and `eval` turns an envelope-level unmodelled
construct into `Undecidable { EnvelopeConstruct(TagPolicies) }` **before** any policy is
considered. So on that service the evaluator answers nothing at all, for any request.

That is the correct answer — a tag policy set is DENY-capable and can change any decision — and
the code path that produces it is egret's own: `download.rs` embeds `tagPolicies` whenever the
service's `config_jsonb` carries a `tagService`, and egret's Trino auto-bind always creates one
(`<profile>_tag`).

**The blast radius, scoped to what is measurable rather than to what the code path suggests.**
An earlier draft of this section said "every Trino profile egret provisions is therefore
undecidable to this crate today". That is **false**, and the store refutes it. Counted on
2026-09-28, `policy.authz_services` holds **eight** profiles: four `trino` (`demo-trino` 2,
`trino` 50, `sp3c2-trino-live` 52, `sp4d1-gate` 56), three `starrocks` (53, 54, 55), and one
`tag` (`sp3c2-trino-live_tag` 51). Two of the four trino profiles carry a `tagService` in
`config_jsonb` — and only **one** of those two is actually undecidable:

| profile | `config_jsonb.tagService` | tag service row exists? | `tagPolicies` in its download | evaluator |
|---|---|---|---|---|
| `sp3c2-trino-live` (52, generated) | `sp3c2-trino-live_tag` | yes (id 51) | **present** | `Undecidable` |
| `demo-trino` (2, generated) | `demo-trino_tag` | **no** | absent | decides normally |
| `trino` (50, manual), `sp4d1-gate` (56, manual) | none | — | absent | decides normally |
| the three starrocks profiles | none | — | absent | decides normally |

`demo-trino` is the exception because its tag service was never created (or was deleted): `GET
/service/plugins/policies/download/demo-trino_tag` returns **404**, `download.rs`'s lookup is
best-effort (`if let Ok(tsvc) = s.store.get_service(&name)`) and omits the block, and the live
`GET /service/plugins/policies/download/demo-trino` comes back with keys
`[serviceName, serviceId, policyVersion, serviceDef, policies, auditMode]` and no `tagPolicies`.
Read-only; `demo-trino` was not touched.

So the accurate statement, and the one F1 should be read as making, is:

> **Undecidable today: every profile whose named tag service actually exists — which is one
> profile, `sp3c2-trino-live`, and it is the only engine-facing Trino profile enforcing anything
> in this cluster. Undecidable from now on: every profile Trino auto-bind creates, because it
> creates the tag service alongside.**

The narrower claim is the stronger one. A reader who checks `demo-trino`, finds it decidable, and
concludes the finding was overstated would discount an escalation that is real.

Consequences, in order of who has to act:

- **This gate.** The Trino instrument had to be a profile with no tag service, which is why
  `sp4d1-gate` exists. Its download has no `tagPolicies` key at all and every prediction in §4 is
  decidable (`unmodelled : []`).
- **SP4d-2 (`explain`).** An `explain` built on this crate will return "I cannot answer" for the
  one engine-facing Trino profile enforcing today, and for every profile auto-bind creates from
  here on, unless tag policies are modelled or the answer is qualified. That is a product
  decision, not a bug, and it belongs to SP4d-2.
- **What was *not* done.** The captured download was **not** edited to remove `tagPolicies` in
  order to obtain a decidable answer. Stripping an input until the evaluator answers is exactly
  the "tune it into agreement" move this gate forbids.

---

## 7. Case 7 — does StarRocks' Ranger plugin fetch tags at all?

A survey claimed StarRocks has no tag policies and an earlier brief repeated it as fact; Task 6
refused to encode it, because encoding an unverified claim would have removed an `Undecidable`
and failed open. egret serves `/service/tags/download/{service}`, so it is directly observable.

**Observation — the StarRocks plugin never fetches tags.** Its own policy cache directory, read
out of the running FE (`sp3c2-sr-live-fe-0`, `starrocks/fe-ubuntu:4.1-latest`, Ranger plugin
2.8.0, polling profile `sp4a-sr-crosscheck`):

```text
/tmp/ranger/sp4a-sr-crosscheck/policycache/
  starrocks_sp4a-sr-crosscheck.json          33864 bytes  2026-09-28 11:21:00 +0000
  starrocks_sp4a-sr-crosscheck_roles.json      100 bytes  2026-09-28 11:20:47 +0000
```

Two files. **No `_tag.json`.** The directory was created at 2026-09-28 11:20:47Z and was read at
18:30Z — **7 h 10 m** during which the plugin's refresher thread was demonstrably alive (6 822
`PolicyRefresher(serviceName=sp4a-sr-crosscheck)` lines in `fe.log`, whose window is
`2026-09-28 00:03:41.379Z` → `2026-09-28 18:30:53.332Z`), and in which not one line mentions a
tag download. Every `tag`-matching line in that log is either a `tablet` substring, a stack frame,
or the plugin's own options dump.

**Positive control on the same day, same cluster, one namespace away.** The Trino plugin (2.5.0)
*does* fetch tags, and it does so even for a service with **no tag service bound at all**. The
private coordinator's cache, seconds after start-up:

```text
/tmp/ranger/sp4d1-gate/policycache/
  trino_sp4d1-gate.json        21162 bytes
  trino_sp4d1-gate_roles.json     92 bytes
  trino_sp4d1-gate_tag.json      117 bytes
```

```text
$ cat trino_sp4d1-gate_tag.json
{"op":"add","serviceName":"sp4d1-gate","tagVersion":1,"isDelta":false,"tagsChangeExtent":"ALL","isTagsDeduped":false}

$ curl .../service/tags/download/sp4d1-gate
{"op":"add","serviceName":"sp4d1-gate","tagVersion":1,"tags":{},"serviceResources":[],"resourceToTagIds":{}}
```

So the difference is not "one engine has tags and the other does not have any to fetch" — the
Trino plugin fetched an *empty* tag set and wrote it down; the StarRocks plugin fetched nothing.

**F3 — and the mechanism is egret's, not StarRocks'.** Ranger builds a plugin's context
enrichers from the service-def it downloads. The two defs egret serves differ exactly there:

| service-def | `contextEnrichers` |
|---|---|
| `trino` | `[{"name":"TagEnricher","enricher":"org.apache.ranger.plugin.contextenricher.RangerTagEnricher","enricherOptions":{"tagRetrieverClassName":"…RangerAdminTagRetriever","tagRefresherPollingInterval":"30000","disableCacheIfServiceNotFound":"true"}}]` |
| `starrocks` | `[]` |

Confirmed in the repo (`crates/egret-policy/src/service-defs/{trino,starrocks}.json`) **and** in
the live downloads both plugins actually received. And the StarRocks plugin is not suppressing
anything — its own startup dump says the opposite:

```text
[RangerPluginConfig.<init>():144] PolicyEngineOptions: { evaluatorType: auto,
evaluateDelegateAdminOnly: false, disableContextEnrichers: false, disableCustomConditions: false,
disableTagPolicyEvaluation: false, disablePolicyRefresher: false, disableTagRetriever: false,
disableUserStoreRetriever: false, enableTagEnricherWithLocalRefresher: false, … ,
enableResourceMatcherReuse: true }
```

`disableTagRetriever: false`, `disableTagPolicyEvaluation: false`, `disableContextEnrichers:
false`. It has no tag enricher because the service-def gives it none.

**What may and may not be concluded.** "StarRocks tag policies do not reach the engine" is true
**of egret's deployment, as configured on 2026-09-28**, and the cause is a single field in
egret's own `starrocks` service-def. It is *not* a property of Ranger 2.8.0, nor of StarRocks:
adding a `TagEnricher` to that def is all it would take to change the answer. Task 6 was right
not to encode the survey's version of this claim — the claim as surveyed is true by accident of
configuration, and an evaluator that hard-coded it would silently become wrong the day the def
changes.

---

## 8. StarRocks — predictions for the policy set that exists, made first, and F2

**These are not S1 and S2.** The brief's two StarRocks cases need policies that could not be
created (§9), so no download contains them and nothing about them can be predicted. What follows
is a prediction-first cross-check against the StarRocks policies that **already exist** — which
still exercises the Gap-B surface that matters for StarRocks (the middle level is `database`, not
`schema`; the plugin is 2.8.0 against a different service-def), just not the ANCESTOR shape. The
live queries could not be run either (§9 obstacle 3), so these predictions stand unmeasured; they
are recorded because they are what found F2, and because a later run should measure against them
rather than produce fresh ones after the fact.

Input: `GET /service/plugins/policies/download/sp4a-sr-crosscheck`,
`policyVersion 54`, 34 policies, **no `tagPolicies` key**; confirmed against the FE's own cache,
which reads `{"serviceName":"sp4a-sr-crosscheck","serviceId":55,"policyVersion":54,…}`.

```text
engine     : StarRocks / Ranger2_8_0
user       : analyst  (groups {}, roles {})

── case sp4a/report/item
   access   Allowed { policy_id: Some(8772), policy_name: "perm-grant-34bffc0d-…" }
   mask     Undecidable { why: [EvalOrderAmbiguous { policy_ids: [Some(8801), Some(8777)],
              policy_names: ["perm-rule-47ab3a6a-…-sp4a10_wh.sp4a.report.item",
                             "perm-rule-d48f048a-…"] }] }
   filter   NoFilter

── case sp4a/report                      (TABLE granularity)
   access   Allowed { policy_id: Some(8772), … }
   mask     NoMask
   filter   Undecidable { why: [EvalOrderAmbiguous { policy_ids: [Some(8776), Some(8806)],
              policy_names: ["perm-rule-48fc5a0c-…",
                             "perm-rule-570d7e80-…-sp4a10_wh.sp4a.report"] }] }

── case sp4a/secret/note
   access   Denied { policy_id: Some(8775), policy_name: "perm-grant-ea5109d4-…",
              reason: DenyPolicyItem }

── case sp4a/sibling/v
   access   Allowed { policy_id: Some(8771), … }
   mask     Masked(MaskResult { policy_id: Some(8802),
              policy_name: "perm-rule-47ab3a6a-…-sp4a10_wh.sp4a.sibling.v",
              mask_type: Some("MASK"), masked_value: None, mask_condition: None })

user       : d3member
── case sp4a/report/item   access Allowed { 8838 }   mask  <same EvalOrderAmbiguous>
── case sp4d1/t2/item      access Allowed { 8773 }   mask  NoMask   filter NoFilter
```

**F2 — egret's rule compiler produces order-ambiguous policy pairs, and the evaluator refuses
them.** The two halves are NOT the same, and the difference is worth stating plainly because
short summaries of F2 have got it wrong:

- **The two MASKS agree.** Policies `8777` and `8801` select the same resource
  `sp4a10_wh.sp4a.report.item`, both `policyPriority 0`, both granting `select` to
  `groups: ["public"]`, and **both carry `dataMaskType: "MASK"` with no `valueExpr`** — an
  AGREEING tie. Whichever wins, the column is masked the same way. The evaluator still refuses,
  because a `MaskResult` names the policy that produced it and it cannot say which that was; see
  `mask_filter.rs::a_same_priority_mask_tie_is_undecidable_when_the_masks_are_identical`.
- **The two ROW FILTERS differ.** Policies `8776` (`country = 'US'`) and `8806` (`id > 2`)
  select the same table at the same priority and carry **different** filter expressions, so which
  one wins is a visible difference in the rows a user sees. This is the half that costs
  something.

Ranger sorts evaluators with `EVAL_ORDER_COMPARATOR`, which for equal priority falls through to a
tie-break the download does not determine; the evaluator reports `EvalOrderAmbiguous` rather than
picking one, for both halves.

This is a finding about **egret's compiler**, not about the evaluator: two rules that overlap on
one resource compile into two same-priority policies with no defined precedence between them. It
is also precisely the class of defect an evaluator that guessed would hide — it would answer
confidently with whichever order its own sort happened to produce, and agree with the engine
about half the time. Not fixed here; SP4d-2 and the perm compiler own it.

A second, smaller observation on the same download: 13 policies (the seeded `egret-admin` set)
carry `delegateAdmin: true` and are reported as `Unmodelled::DelegateAdmin`. They did **not**
make any decision above undecidable, which is the intended behaviour — only a construct on a
policy that participates in *this* decision can.

---

## 9. S1 and S2 — NOT MEASURED, and exactly why

The two StarRocks cases the brief asks for (the ANCESTOR shape, and the same written down to
`column: ["*"]`) were **not measured**. Trino's result is **not** assumed to carry over: StarRocks
runs a different plugin version (2.8.0) against a different service-def whose middle level is
`database`, not `schema`.

Three obstacles, each recorded with what was tried:

1. **The policy could not be written.** The only running StarRocks FE polls the profile
   `sp4a-sr-crosscheck` (id 55) — read out of
   `/opt/starrocks/fe/conf/ranger-starrocks-security.xml` inside the FE, not assumed from the
   engine name, which is `sp3c2-sr-live`. That profile has `origin = 'generated'`, and
   `POST /api/control-plane/policies` refuses every write to a generated profile.

   **The refusal quoted below is borrowed, and the borrowing matters.** The attempt that produced
   it named the profile `sp3c2-sr-live` — the *engine*'s name — before the FE's own
   `ranger-starrocks-security.xml` was read and showed it polls `sp4a-sr-crosscheck` instead. So
   this is the verbatim 409 for **profile 54**, not 55:

   ```json
   {"error":"conflict: profile 54 is compiled by the SP4a permission model — edit its grants and
   rules instead of writing a policy directly; the next recompile would silently overwrite this
   change"}
   ```

   `crates/egret-policy/src/store.rs:190` interpolates the **service id** into that message, so
   `54` names `sp3c2-sr-live` and nothing else. No write against profile 55 was ever issued, so
   no 409 for 55 is quoted anywhere in this document. What carries the conclusion is not the
   quote but the guard: `refuse_if_generated` reads `policy.authz_services.origin` and returns
   `CompilerOwned` for any `'generated'` profile, and profile 55 is `generated` — measured,
   `select id, name, origin from policy.authz_services` →
   `55|sp4a-sr-crosscheck|starrocks|generated`. Identical code path, same outcome; the message
   would differ only in the number.

   That this slip is the exact engine-named-vs-profile-id confusion the paragraph above it warns
   against is worth leaving on the record rather than quietly fixing.

2. **The shape cannot be produced through the permission model either.** Writing a `perm` grant
   is the sanctioned way onto a generated profile, but today's compiler always emits a full
   four-level resource ending in `column: ["*"]` — every `perm-grant-*` policy in profile 55
   shows it. That is the SP4a T1 fix working as intended, and it means the ANCESTOR shape is no
   longer reachable through egret's own API on a generated profile. Producing it requires a
   hand-written policy on a manual profile.

3. **Repointing the FE was rejected, and so was a live query as a non-`root` subject.**
   Repointing means editing `ranger-starrocks-security.xml` inside the FE and restarting it; that
   FE's metadata lives on an `emptyDir` and has been lost twice already
   (`docs/en/plans/2026-09-26-starrocks-fe-meta-persistence.md`,
   `docs/en/plans/2026-09-28-sp4c-live-result.md` §1). Separately, the FE's
   `authentication_chain` currently reads `[native]` — the `egret_oidc` security integration
   exists but is not in the chain — so every delegated query fails before Ranger is reached:

   ```text
   connect: Server error: `ERROR 1045 (28000): Access denied for user 'analyst' (using password: YES)'
   ```

   Repairing that means `ADMIN SET FRONTEND CONFIG ("authentication_chain" = "native,egret_oidc")`
   on a shared engine. `root` is not a substitute subject: it bypasses the access check before
   Ranger is consulted (`RangerAccessController.java:115-117`, recorded in SP4c), so a `root`
   measurement proves nothing about a deny.

Building a private StarRocks the way the private Trino was built is possible in principle but is
not proportionate: it needs an FE **and** a CN, its own metadata, the external Iceberg catalog
re-registered and an authentication chain configured — the same four repairs SP4c had to make —
for two cases.

**What StarRocks contributed anyway:** case 7 (§7), which needs no query and no authentication,
and §8's predictions against the policy set that *does* exist — one of which (F2) found a real
compiler defect in the live policy store. To be exact, because the distinction is the whole point
of §9: §8 does **not** contain predictions for S1 or S2. Obstacles 1 and 2 above mean neither
shape exists in any download, and a prediction is computed from a download. A later run must
create the policy before it can predict anything, and only then measure.

---

## 10. What was left behind, and what was cleaned up

| object | state after the run |
|---|---|
| pod + ConfigMap `sp4d1-gate-trino` (`egret-engines`) | **deleted** — it held 2 GiB of a 24 GiB node |
| profile `sp4d1-gate` (id 56) and its 16 policies | **kept** — additive, enforced by nothing now, and what makes the gate re-runnable |
| schemas `sp4a10_wh.sp4d1` / `.sp4d1a` and their 8 tables | **kept**, four rows each |
| `sp3c2-trino-live`, `demo-trino`, `sp3c2-sr-live` | **untouched** — no ConfigMap, deployment, FE config or policy of theirs was modified |
| `sp3c_live_wh` | untouched; confirmed still unwritable (`Failed checking new table's location: s3://e2e/sp3c-live/…`), the pre-existing defect SP4a recorded |
| the impersonated session used for the StarRocks attempt | discarded; the overlay expires 30 min after issue |

Disk on the host was 42 GiB free before and after.

---

## 11. Reproducing this

```bash
# 1. the prediction, from whatever the plugin is actually holding
kubectl --context kind-egret exec -n egret-engines <trino-pod> -- \
  curl -s http://egret.egret-system.svc.cluster.local:8080/service/plugins/policies/download/sp4d1-gate \
  > /tmp/dl.json
cargo run -p egret-ranger-eval --example live_gate -- /tmp/dl.json trino admin sp4a10_wh

# 2. confirm the plugin holds that same policyVersion
kubectl --context kind-egret exec -n egret-engines <trino-pod> -- \
  head -c 110 /tmp/ranger/sp4d1-gate/policycache/trino_sp4d1-gate.json

# 3. only then, the query
kubectl --context kind-egret exec -n egret-engines <trino-pod> -- \
  trino --server localhost:8080 --user admin --execute "SELECT id, item FROM sp4a10_wh.sp4d1a.t1"
```

Step 2 is not optional. The live coordinator polls every 30 s
(`ranger.plugin.trino.policy.pollIntervalMs = 30000`); measuring before a poll lands records the
previous policy state and produces a result that looks like a finding.

The full private-coordinator manifest, and the evaluator check as a step in the existing manual
recipe, are in
[`docs/en/testing/2026-06-18-trino-iceberg-ranger-manual-test.md`](../testing/2026-06-18-trino-iceberg-ranger-manual-test.md)
§7.
