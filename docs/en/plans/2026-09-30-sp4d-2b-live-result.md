# SP4d-2b Task 8 — the live gate: asking the engine that is actually enforcing

> **Measured:** 2026-10-01 (UTC), kind context `kind-egret`, branch
> `feat/sp4d-2b-evaluate-tag-policies`, worktree `egret-sp4a`. Canonical (English).

This is the task the package exists for. Tasks 1-7 built `crates/egret-ranger-eval`,
wired it to `explain`, and lifted the tag refusal. **Nothing in it had ever been asked of
a Trino whose plugin was holding egret's tag download, and the page's copy — rewritten
three times in Task 7 — had never been looked at in a browser.**

The whole-package record is why neither is a formality:

| instrument | defects it found |
|---|---|
| 127 unit tests written alongside the code | **0** of the 3 the differential corpus found |
| the differential corpus against the real Java engine | **7**, including fail-opens |
| the browser, on this exact page (SP4d-2a) | **2 Majors** a full review and 15 green tests both missed |

## The method, and it is the deliverable

**Every prediction below was written down and committed before the thing it predicts was
observed.** The commit that adds §1-§6 of this file contains no observations; the
observations arrive in a later commit. This is the arrangement SP4d-2a used deliberately
(`6079b2f9` predictions, `2ba3d2c3` observations) so that a prediction can be *wrong on
the record*, and it is the only arrangement in which a prediction means anything.

A disagreement is a **finding**, not a case to retune: either the evaluator, egret's tag
resolution, or this package's reading of Ranger is wrong, and the job is to say which.

---

## 1. The stack, and the proof it is this branch

**A stack was already listening on :5173 and :8080 and it is NOT this branch** —
`./target-check/debug/egret serve` from this worktree, started 2026-09-27, database
`egret_sp4b`; its served spec is 665 KB against this branch's 1.28 MB and carries no
`ExplainAttribution`. Neither it nor its Vite was killed, and nothing below touches
them.

```bash
curl -s localhost:8090/api-docs/openapi.json | grep -c resource_candidacy_not_reproducible
# → 1   (non-zero ⇒ this is the branch's own build; the :8080 stack answers 0)
```

| step | what ran | where |
|---|---|---|
| backend | `target/debug/egret serve`, built from this worktree at 2026-10-01 13:15 | **:8090**, `0.0.0.0` so the kind pod can reach it |
| database | **`egret_sp4d2b`**, created for this task as `CREATE DATABASE … TEMPLATE egret_sp4d2a`, then migrated by this branch's binary to **159** (= `0159_workflow_task_logs.sql`) | local docker `egret-dev-postgres-1` |
| frontend | `npx vite --config frontend/vite.config.sp4d2b.mts` (a scratch config, port + proxy target only; deleted after the run) | **:5290** → :8090 |
| engine | a **private, additive** Trino coordinator pod `sp4d2b-gate-trino`, `trinodb/trino:470` (hence `ranger-plugins-common-2.5.0`, the version Task 1 measured), HTTP with no authentication so the session user is chosen freely | `egret-engines` |

**Not touched, and checked rather than assumed:** `egret_test` and the shared
`egret_test_template` (another session is running `cargo test --workspace` against them,
and the template is at migration 162 while this branch ends at 159); the dev database
`egret`; the kind cluster's own egret and its Postgres; the provisioned engines
`sp3c2-trino-live`, `demo-trino`, `sp3c2-sr-live`. `EGRET_ALLOW_DB_SKIP` was never set —
its "ok in 0.01 s" is a silent skip, not evidence.

**The one thing this run reads from the shared cluster is table DATA.** The private
coordinator's `sp4a10_wh.properties` is a **verbatim copy** of
`cm/sp3c2-trino-live-catalogs`, so it reads the live Iceberg warehouse through the live
egret's catalog REST, and SP4d-1's fixture tables (`sp4a10_wh.sp4d1.t2 … t7`, four rows
each) are reused as-is. **No write of any kind is issued against the live catalog, the
live egret, or the live database.** Every policy, every tag and every tag assignment this
gate evaluates lives in `egret_sp4d2b`.

### Why the Ranger admin is this branch's own build, and not the cluster's

SP4d-1's gate pointed its private coordinator at the cluster's egret. This one points it
at **:8090**, i.e. this branch:

```xml
<property><name>ranger.plugin.trino.policy.rest.url</name>
          <value>http://host.docker.internal:8090</value></property>
```

`host.docker.internal` resolves to `192.168.65.254` from inside a kind pod (measured:
`getent hosts` in the pod, and a `curl` to that address:8080 returning the stale stack's
404 — two instruments, one of them positive). Three things follow and all three are the
point:

1. the policy download, the **tag download** and the page are one instance and one
   database, so the browser can be asked about the exact resource the engine just
   masked;
2. `catalog_tag_assignments` is mine to write, which is the only way to create the
   **table-level attachment** no live data has (brief case 4);
3. nothing shared changes, so nothing has to be restored.

### Pre-flight, which is not a case

A table with no tag assignment and its own access policy (`t7`, `acc-t7-preflight`),
queried as `analyst`, who has no other grant anywhere in this profile:

```text
"1","book","US"   "2","pen","VN"   "3","lamp","US"   "4","desk","VN"
```

That establishes two facts the cases are read against: the plugin is enforcing **from
this build's download** (analyst can read exactly the one table it allows), and the raw
data is `(1,book,US) (2,pen,VN) (3,lamp,US) (4,desk,VN)` in every fixture table.

---

## 2. The fixture, in full

Seeded by SQL into `egret_sp4d2b` — warehouses, catalogs, engines and attachments have no
HTTP surface (a k8s provisioning creates them), and the three tag policies are copies of
live bodies that `POST /api/control-plane/policies`' editor shape cannot express
(`denyExceptions`). This is what an operator writing straight into Ranger looks like, and
the live profile 52 carries five hand-written policies of exactly this kind.

```sql
-- profiles: one for the ENGINE gate, one for the PAGE, because the tag-resolution
-- gate is per CATALOG — a wildcard-bearing assignment anywhere in a catalog declines
-- every target in it, which would destroy the engine cases if they shared a catalog.
INSERT INTO policy.authz_services (name, type, config_jsonb, origin) VALUES
 ('sp4d2b-gate',     'trino', '{"tagService":"sp4d2b-gate_tag"}', 'manual'),
 ('sp4d2b-gate_tag', 'tag',   '{}',                               'manual'),
 ('sp4d2b-ui',       'trino', '{"tagService":"sp4d2b-ui_tag"}',   'manual'),
 ('sp4d2b-ui_tag',   'tag',   '{}',                               'manual');

-- catalogs. `sp4a10_wh` is NAMED to match the live Trino catalog, because the tag
-- download's `catalog` level comes from `registry.catalogs.name`.
--   sp4a10_wh    → engine sp4d2b-gate-eng → profile sp4d2b-gate   (the engine cases)
--   d2b_empty_wh ┐
--   d2b_wild_wh  ├ engine sp4d2b-ui-eng   → profile sp4d2b-ui     (the page cases)
--   d2b_fold_wh  ┘

-- tags: pii, clr, tblwide, ancdeny (+ a clearance gate on clr and ancdeny)

-- tag assignments, in the ENGINE catalog (every coordinate one the resolver reproduces)
--  pii      sp4d1.t2.item      column-level, SELF for the column request
--  pii      sp4d1.t3.item      column-level
--  pii      sp4d1.t4.''        TABLE-level  ← the arm no live data has
--  clr      sp4d1.t5a.item     column-level, the clearance
--  ancdeny  sp4d1.t5b.''       TABLE-level, the clearance  ← the ANCESTOR access arm
--  pii      sp4d1.t6.item      column-level  ┐ the same table at BOTH levels,
--  tblwide  sp4d1.t6.''        TABLE-level   ┘ with DIFFERENT mask types

-- tag assignments, in the three PAGE catalogs — each with a legitimate sibling row,
-- so naming "a row that offends" is not the same as naming any row at all
--  d2b_empty_wh: ('sales','orders','email')  and  ('','orders','')
--  d2b_wild_wh : ('sales','orders','email')  and  ('sales','ord*','email')
--  d2b_fold_wh : ('sales','orders','email')  and  ('sales','orders','emaıl')  -- U+0131
```

Policies, as the plugin downloads them (ids are the DB's):

```text
sp4d2b-gate      40 base-queryid        49 acc-t4
                 41 base-trinouser      50 acc-t5a
                 42 base-sysprop        51 acc-t6
                 43 base-sessionprop    56 acc-t7-preflight
                 44 base-func           59 acc-t5b
                 45 base-sysinfo       —— no baseline policy grants `select` on anything
                 46 base-catalog
                 47 acc-t2   48 acc-t3

sp4d2b-gate_tag  52 tp-mask-pii          policyType 1, tag=pii,     groups=[public], MASK
                 53 tp-filter-pii        policyType 2, tag=pii,     groups=[public], id > 2
                 54 tp-clearance-clr     policyType 0, tag=clr,     deny public, denyException admin
                 55 tp-mask-tblwide      policyType 1, tag=tblwide, groups=[public], MASK_NULL
                 60 tp-clearance-ancdeny policyType 0, tag=ancdeny, deny public, denyException admin

sp4d2b-ui        57 ui-acc-all           select on *.*.*.* for analyst, admin
sp4d2b-ui_tag    58 ui-mask-pii          policyType 1, tag=pii, MASK
                 61 ui-deny-pii          policyType 0, tag=pii, deny public, denyException admin
```

52, 53 and 54 are the **live** tag policies 8799, 8805 and 8839 with their bodies copied
field for field — `groups:["public"]`, `dataMaskType:"MASK"`, `filterExpr:"id > 2"`, and
the clearance's `denyPolicyItems`/`denyExceptions` pair — and only the `service` name and
the tag VALUE changed. The live 8839's tag value is `tag`, and **no live resource carries
a tag called `tag`**, so the only deny-capable clearance in the cluster is inert on live
data; a copy on an attached tag is the only way to measure it.

### The live tag block this is a copy of

```json
{"id":8799,"service":"sp3c2-trino-live_tag","policyType":1,"resources":{"tag":{"values":["pii"]}},
 "dataMaskPolicyItems":[{"groups":["public"],"accesses":[{"type":"select"}],
                         "dataMaskInfo":{"dataMaskType":"MASK"}}]}
{"id":8805,…,"policyType":2,"resources":{"tag":{"values":["pii"]}},
 "rowFilterPolicyItems":[{"groups":["public"],…,"rowFilterInfo":{"filterExpr":"id > 2"}}]}
{"id":8839,…,"policyType":0,"resources":{"tag":{"values":["tag"]}},
 "denyPolicyItems":[{"groups":["public"],"accesses":[select,insert,delete]}],
 "denyExceptions":[{"users":["admin"],"accesses":[select,insert,delete]}]}
```

`GET /service/plugins/policies/download/sp3c2-trino-live` on the cluster: resource
service at `policyVersion 116`, **tag block `sp3c2-trino-live_tag` at `policyVersion 22`
with these three** — the version the brief predicted from, confirmed before anything was
copied.

---

## 3. The poll: evidence the plugin holds what the prediction was computed from

The prediction below is computed from `$SP/dl.json`, fetched **from inside the pod** off
`http://host.docker.internal:8090` — `policyVersion 3`, 13 resource policies, a tag block
with 4 policies at `policyVersion 2`. The plugin's own on-disk cache, read out of the
running pod:

```text
/tmp/ranger/sp4d2b-gate/policycache/trino_sp4d2b-gate.json
  {"serviceName":"sp4d2b-gate","serviceId":8,"policyVersion":3,"policies":[{"id":40,…
/tmp/ranger/sp4d2b-gate/policycache/trino_sp4d2b-gate_tag.json
  {"op":"add","serviceName":"sp4d2b-gate","tagVersion":2,"tags":{"3":{"isEnabled":true,
   "type":"tblwide","owner":0},"1":{"isEnabled":true,"type":"pii",…
  mtime 2026-10-01 06:24:49Z  (policy) / 06:24:49Z (tag)
```

**The tag cache file is the thing SP4d-1 could not show**: that profile had no tag
service, and the one that did answered `Undecidable`. `tagVersion 2` is this build's own
tag download, and the poll interval on this pod is 10 s (`pollIntervalMs`), so both
caches are re-read after the last case and quoted again in §7.

Two policies (59, 60) and one tag assignment were added for the ANCESTOR access arm after
that fetch; the cache is re-captured in §7 and the version it holds at measurement time
is what each case is read against. Any case whose prediction was computed from a
different version than the plugin held is marked as such and discarded.

---

## 4. PREDICTION — the engine cases

The evaluator's answers come from `cargo run -p egret-ranger-eval --example live_gate`,
extended by this task to carry a resolved tag set. **It goes through the composed
`evaluate` / `evaluate_data_mask` / `evaluate_row_filter`** — never the `#[cfg(test)]`
phase functions, per the Task-3 ruling, so what is predicted is what an operator is
shown. The tag set is built the way `attachment_for` builds it; that is a **second copy
of that mapping**, which this package normally refuses, and what keeps it honest is §5:
the ACCESS arm of every case is also asked of the live `explain` endpoint, which builds
the set through the real `attachment_for` from the real `catalog_tag_assignments`.

`{col}` masking in Trino's def is
`cast(regexp_replace(regexp_replace(regexp_replace({col},'([A-Z])','X'),'([a-z])','x'),'([0-9])','0') as {type})`;
`MASK_NULL` has no transformer at all.

| # | brief | query, as `analyst` unless stated | evaluator says (computed first) | I expect the engine to |
|---|---|---|---|---|
| 1 | 1 | `SELECT id,item,country FROM sp4d1.t2` | access `Allowed{47 acc-t2}`; mask on `item` `Masked{52 tp-mask-pii, MASK}`; **table-granularity filter `NoFilter`** (the column attachment is a DESCENDANT of the table, so the table request resolves the empty set) | return **4 rows**, `item` = `xxxx / xxx / xxxx / xxxx`, `id` and `country` raw |
| 2 | 2 | `SELECT id,item,country FROM sp4d1.t3` | same shape as 1: mask applies to `item`, `NoFilter` at table granularity | **4 rows**, not 2 — the row-filter tag policy does **not** fire for a tag attached only to a column, because `rowFilterDef.resources` names no `column` and the table request carries no tag |
| 3a | 3 | `SELECT id,item FROM sp4d1.t5a` | `Denied{54 tp-clearance-clr, DenyPolicyItem}` on `item` | `Access Denied` naming `item` |
| 3b | 3 | `SELECT id FROM sp4d1.t5a` | `Allowed{50 acc-t5a}` — `id` carries no tag, so the clearance never reaches it | **4 rows** of raw `id` |
| 3c | 3 | `SELECT id,item FROM sp4d1.t5a` as **admin** | `Allowed{50 acc-t5a}` — the `denyException` cancels the deny | **4 rows**, raw |
| 4a | 4 | `SELECT item FROM sp4d1.t4` | mask `Masked{52, MASK}` **via `pii:ANCESTOR`** (a table-level attachment matching a column request); filter at table granularity `Filtered{53, id > 2}` via `pii:SELF` on the table | **2 rows** (`id` 3,4), both `item` = `xxxx` |
| 4b | 4 | `SELECT item FROM sp4d1.t5b` | `Denied{60 tp-clearance-ancdeny, DenyPolicyItem}` via `ancdeny:ANCESTOR` | `Access Denied` |
| 4c | 4 | `SELECT item FROM sp4d1.t5b` as **admin** | `Allowed{59 acc-t5b}` | **4 rows**, raw |
| 4d | 4 | `SELECT id,item,country FROM sp4d1.t4` | every column of `t4` resolves `pii:ANCESTOR`, so every column is `Masked{52, MASK}` | **the query FAILS.** `MASK`'s transformer wraps the column in `regexp_replace`, and `id` is a `bigint`: Trino has no implicit `bigint→varchar` coercion for a function argument. A tag-selected mask reaches *every* column of a table-tagged table, numeric ones included |
| 5 | 5 | `SELECT id,item,country FROM sp4d1.t6` | `item` resolves `pii:SELF` **and** `tblwide:ANCESTOR` → `Masked{52 tp-mask-pii, MASK}`, i.e. the SELF tag's policy; `id` and `country` resolve `tblwide:ANCESTOR` only → `Masked{55 tp-mask-tblwide, MASK_NULL}`; table-granularity filter `NoFilter` (the table-level tag is `tblwide`, which has no filter policy) | **4 rows** with `item` = `xxxx / xxx / xxxx / xxxx` and `id`, `country` **NULL**. ⚠ This single query decides the `MATCH_TYPE_COMPARATOR` question: if Ranger ordered ANCESTOR before SELF, `item` would be NULL too |

**Two artefacts of the instrument, stated before they can be mistaken for defects.**
`live_gate` asks all three questions of whatever resource it is given, so it reports a
row filter for a column-granular request (case 1) and a mask for a table-granular one
(case 4a). A tag policy matches on the TAG resource, not on the column, so those answers
are correct — Trino simply never asks them that way (`getRowFilters` is per table,
`getColumnMask` per column). Nothing below reads them as engine predictions.

### What I expect to be wrong

Case 4d is the prediction I am least sure of, and it is a prediction about **Trino**, not
about the evaluator: the plugin may well skip a mask whose transformer does not type-check,
or Trino may coerce. If 4d returns rows, my reading of the transformer is wrong and the
finding evaporates; if it fails, egret can write a tag mask that makes an ordinary
`SELECT *` unrunnable, which is a product finding this package has not recorded.

Case 5 is the one I am most sure of and the most valuable if wrong.

---

## 5. PREDICTION — `explain`'s access arm, as the cross-check on the tag set

Asked of the running :8090 instance, `POST /api/control-plane/perm/explain`, which
resolves the tag set through the real `resolved_tags_for_target`. Predicted to agree with
the `live_gate` access column above, case for case: `t2.item` `allowed 47`, `t3.item`
`allowed 48`, `t4.item` / `t4.id` `allowed 49`, `t5a.item` **`denied 54 deny_policy_item`**,
`t5a.id` `allowed 50`, `t5a.item` as admin `allowed 50`, `t6.item` / `t6.country`
`allowed 51`, `t5b.*` **`denied 60 deny_policy_item`** and as admin `allowed 59`.

If any of those disagrees with `live_gate`, the second copy of `attachment_for` in the
example is wrong and every mask/filter prediction above is void.

---

## 6. PREDICTION — the page, in a browser

`http://localhost:5290/governance/explain-access`, signed in as
`admin@egret.local`. Task 7 rewrote this page's copy **three times** and it now renders
four distinct decline sentences and two distinct reaches; the suite asserts they are
pairwise distinct, which is not the same as asserting any of them is true of the data.

**The four decline states, by the catalog that produces them.** Each prediction is the
API variant and the sentence, both derived from the code and neither observed:

| # | ask | API `tag_resolution_declined` | the sentence I expect, and the reach note |
|---|---|---|---|
| P1 | `d2b_empty_wh` / `sales` / `orders` / `email` | `stored_level_empty {schema:"", table:"orders", column:""}` | "a tag assignment in this catalog is stored at **(none) / orders / (the table itself)**, and a level left empty there…" + the **per-catalog** note. One row exercising BOTH copy decisions: `(none)` for the empty schema and `(the table itself)` for the empty column |
| P2 | `d2b_wild_wh` / `sales` / `orders` / `email` | `stored_wildcard {schema:"sales", table:"ord*", column:"email"}` | "…stored at **sales / ord\* / email**, whose "\*" or "?" the engine matches…" + per-catalog |
| P3 | `d2b_fold_wh` / `sales` / `orders` / `email` | `stored_fold_divergent {schema:"sales", table:"orders", column:"emaıl"}` | "…stored at **sales / orders / emaıl**, which carries a character whose upper- and lower-case forms…" + per-catalog |
| P4 | `sp4a10_wh` / `sp4d1` / `t2` / `ıtem` (U+0131) | `target_fold_divergent {level:"column", value:"ıtem"}` | "the **column** you asked about, "ıtem", carries a character…" + the **per-request** note: "This one IS specific to what you asked: nothing is stored wrongly, and every other target in this catalog answers normally." |

In all four the decision must be `Undecidable` with the cause `tags_unresolved`, and the
head sentence must keep the distinction *"declined to resolve" is not "there are none"* —
the product proposition is that "the engine will deny" and "egret cannot tell you" are
different answers, and a steward who confuses them makes a wrong change.

**P5 — the decided cases, which is where I expect to find something.** `sp4a10_wh` /
`sp4d1` / `t5b` / `item` as `analyst` renders **Denied**, by the Ranger policy
`tp-clearance-ancdeny`, and `explain`'s `tags` field for that target is **`[]`** —
measured at the API, and by design: `tags` is `tag_keys_for_resource_via`, an exact-match
query with no roll-up, while the decision came from an ANCESTOR attachment. So I predict
the page shows a steward **"Denied, by a tag policy" beside an empty tag list**, with
nothing on the page naming the table-level assignment that caused it. If that is what it
shows, it is a Major of exactly SP4d-2a's kind: not a wrong sentence, a *missing* one.

**P6 — `(none)` judged in the browser, not in a test.** The round-3 implementer flagged
`storedCoordinate`'s `(none)` as "a copy decision made without measurement". My
expectation: `(none) / orders / (the table itself)` reads as two different kinds of
nothing in one line and will be hard to act on, because the remedy sentence
("Re-store that assignment on a table or a column") names *table or column* while the
thing that is empty is the *schema*. I expect to want a wording change here.

**P7 — legibility of the two reaches.** I expect the per-catalog and per-request notes to
be distinguishable only by reading both carefully, since they are rendered in the same
place, in the same italic style, and both begin "This…". I expect to find that acceptable
but want to see it.

---
