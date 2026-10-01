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
| frontend | `npx vite --config frontend/vite.config.sp4d2b.mts` (differs from `vite.config.ts` in the port and the proxy target only, and is **committed** rather than deleted, so this gate is re-runnable; `npm run lint` and `tsc` are clean with it present) | **:5290** → :8090 |
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

## 7. OBSERVATION — the engine

Run through the private coordinator, `trino --user <subject> --execute`. The plugin's
cache at the time of the run: `policyVersion 4 → 7`, `tagVersion 3 → 4` (the later bumps
added `acc-t5b`/`tp-clearance-ancdeny`, then `tp-mask-tblmask`, then two
`information_schema` grants — **none of them touches a case table already measured**, and
the predictions were re-computed on the version the plugin held and came out identical,
case for case). Final cache, after the last query:

```text
trino_sp4d2b-gate.json      {"serviceName":"sp4d2b-gate","serviceId":8,"policyVersion":7,…
trino_sp4d2b-gate_tag.json  {"op":"add","serviceName":"sp4d2b-gate","tagVersion":4,
                             "tags":{"1":{"type":"pii"},"2":{"type":"clr"},
                                     "3":{"type":"ancdeny"},"4":{"type":"tblwide"},
                                     "5":{"type":"tblmask"}},…
store: sp4d2b-gate 7/4, sp4d2b-gate_tag 4/4   — the plugin and the store agree
```

| # | query | engine said | agrees? |
|---|---|---|---|
| 1 | `SELECT id,item,country FROM t2` (analyst) | `1,xxxx,US  2,xxx,VN  3,xxxx,US  4,xxxx,VN` | **YES** |
| 2 | `SELECT id,item,country FROM t3` (analyst) | the same four rows | **YES** |
| 3a | `SELECT id,item FROM t5a` (analyst) | `Access Denied: Cannot select from columns [item, id] in table or view t5a` | **YES** |
| 3b | `SELECT id FROM t5a` (analyst) | `1 2 3 4` | **YES** |
| 3c | `SELECT id,item FROM t5a` (admin) | `1,book  2,pen  3,lamp  4,desk` | **YES** |
| 4a | `SELECT item FROM t4` (analyst) | **`Invalid column mask for 'sp4a10_wh.sp4d1.t4.id': Unexpected parameters (bigint, varchar(7), varchar(1)) for function regexp_replace`** | **NO — my prediction was wrong, see F1** |
| 4a2 | `SELECT count(*) FROM t4` (analyst) | the same failure | **NO**, same cause |
| 4b | `SELECT item FROM t5b` (analyst) | `Access Denied: Cannot select from columns [item] in table or view t5b` | **YES — the ANCESTOR arm** |
| 4c | `SELECT id,item FROM t5b` (admin) | `1,book  2,pen  3,lamp  4,desk` | **YES** |
| 4d | `SELECT id,item,country FROM t4` (analyst) | the same `Invalid column mask` failure | **YES**, and it was the prediction I flagged as least certain |
| 5 | `SELECT id,item,country FROM t6` (analyst) | `"","xxxx",""` ×4 — i.e. `id` and `country` NULL, `item` masked `xxxx / xxx / xxxx / xxxx` | **YES — the comparator ordering holds** |
| F-B | `SELECT count(*),count(id),count(country) FROM t6` | `4, 0, 0` | **YES** — the MASK_NULL columns are genuinely NULL, not empty strings |

**Ten of twelve agree. The two that do not are the same finding, and it is not the
evaluator's.** Both halves of what the evaluator predicted for `t4` were right — `MASK`
via a `pii:ANCESTOR` attachment, `id > 2` via a `pii:SELF` one — and that is exactly what
Trino tried to do and could not.

### Case 4 is the arm the package existed to measure, and it is confirmed twice

The brief called the table-level `ANCESTOR` attachment "derived from bytecode and never
observed … the single most valuable measurement in the package". It is now observed, on
the real engine, in two independent ways:

- **through the access decision** (4b): a tag attached to `sp4d1.t5b` with **no column at
  all** produced `Access Denied` for a request on `sp4d1.t5b.item`. The engine resolved
  the shorter resource as an ANCESTOR of the column exactly as Task 1 derived, and the
  deny-capable clearance fired through it. `admin`, named in the policy's
  `denyExceptions`, read the table (4c), so the arm is measured in both directions and the
  deny is not a blanket refusal.
- **through the mask** (4a/4d): the same shape produced `MASK` on every column of `t4`.

And egret's own resolution produced it: `POST /api/control-plane/perm/explain` for
`sp4d1.t5b.item` answers `denied, policy 60, deny_policy_item` — which it can only do if
`resolved_tags_for_target` returned `ancdeny` with `MatchType::Ancestor`. The ANCESTOR arm
is live, end to end, from `catalog_tag_assignments` through `from_assignments`, the
plugin's enricher, the engine's decision, and this crate's prediction of it.

### Case 5: the `MATCH_TYPE_COMPARATOR` ordering, measured

`sp4d1.t6` carries `pii` on the column `item` (SELF for a request on that column) and
`tblwide` on the table (ANCESTOR for the same request). `pii`'s mask is `MASK`;
`tblwide`'s is `MASK_NULL`. One query decides it:

```text
SELECT id, item, country FROM sp4a10_wh.sp4d1.t6   →   "","xxxx",""   ×4
```

`item` came back `xxxx`, **not NULL** — the SELF attachment's tag policy won, and the
ANCESTOR one decided the two columns where it was the only attachment. Had Ranger ordered
ANCESTOR first, `item` would have been NULL. The same tag at two levels is **not deduped**
and the match type **is** the ordering, on the real engine, as the comparator implies.

### F1 (NEW, Major) — a table-level tag + a MASK rule makes the table unreadable, and the mask is applied to columns the query never names

My prediction for 4a was "2 rows, both `item` = `xxxx`". The engine refused the query, and
the reason is not the one I predicted for 4d either. Two measurements separate the causes:

```text
-- F-A: a NEW table-level tag `tblmask` (MASK), on sp4d1.t7, with NO row-filter policy
SELECT item     FROM t7  →  Invalid column mask for '…t7.id'   -- PREDICTED: would SUCCEED
SELECT id       FROM t7  →  Invalid column mask for '…t7.id'
SELECT count(*) FROM t7  →  Invalid column mask for '…t7.id'
```

**My F-A prediction was WRONG and the truth is worse than it.** I predicted Trino masks
only the columns a query needs, so that `SELECT item` would survive. It does not: Trino
builds the column masks for **every column of the scanned table**, so one table-level tag
assignment whose tag carries a `MASK` rule makes the whole table unreadable — `count(*)`
included — as soon as any column is not a `varchar`. No row filter is needed; `t4`'s
filter was a red herring in my reading.

The mechanism, and each half is measured:

1. egret's `trino` service-def defines `MASK` as
   `cast(regexp_replace(regexp_replace(regexp_replace({col},…),…),…) as {type})`, which is
   **varchar-only**;
2. a tag-addressed mask rule applies to a *tag*, and a table-level assignment puts that
   tag on **every column beneath the table** as an ANCESTOR match — which is precisely
   what case 4 proves the engine honours;
3. Trino type-checks every column mask on the scanned table before running anything.

**It is reachable by two ordinary, supported product actions.**
`PUT /api/control-plane/warehouses/{id}/resource-tags` with `column` **absent** writes
`column_name = ''` (migration 0081: "`''` = table-level tag"), and
`POST /api/control-plane/perm/rules` with a tag selector is how the live cluster's own
mask policy 8799 came to exist. Neither surface warns.

**The live cluster is not in this state today, and that is measured rather than assumed.**
Every live tag assignment is column-level, and every tagged column is a `varchar`
(`report.item`, `report.country`, `sibling.v`, `srcases.item`, `srcases.country`,
`gate.secret` — read from `information_schema.columns`); every table's `id` is a `bigint`
and **none is tagged**. So the live deployment is exactly one table-level tag assignment
away from an unreadable table.

**Whose defect it is.** Not `egret-ranger-eval`'s: it predicted `Masked{MASK}` for
`t4.id`, which is what the engine attempted. Not the tag resolution's. It belongs to the
**writer and the UI** — egret can store a combination its own engine cannot execute — and
the remedy is one of: refuse a tag-addressed `MASK`/`MASK_SHOW_*`/`MASK_HASH` rule when
the tag reaches a non-text column; refuse (or warn on) a table-level assignment for a tag
carrying such a rule; or emit a type-aware transformer. That is a decision for the
programme, not for this task, and it is recorded as an open item.

### F2 (minor, an observation about the live data) — the only deny-capable clearance in the cluster is inert

Live tag policy 8839 denies on tag `tag`, and the tag named `tag` (id
`3205756d-…`, `key='tag'`) is attached to **no resource at all**. So the clearance that
made this package worth building cannot fire on live data as it stands; measuring it
required a copy on an attached tag. Worth a look by whoever created that tag — a
clearance on an unattached tag reads, on the Classifications page, exactly like one that
is protecting something.

---

## 8. OBSERVATION — the page, in a browser

`http://localhost:5290/governance/explain-access`, signed in as `admin@egret.local`,
Chromium via Playwright. The subject for the decline cases is **`cleared_person`** rather
than `analyst`: the subject is a user PICKER over `users`, and `analyst` is a Ranger-side
username with no egret row, so it cannot be typed. The substitution is policy-neutral —
neither name appears in any `denyExceptions` — and it is the first thing the browser
showed that the API did not.

### The four decline states: all four sentences are TRUE of the data

| # | rendered sentence (verbatim from the DOM) | as predicted? |
|---|---|---|
| P1 | "…egret declined to resolve them: a tag assignment in this catalog is stored at **(none) / orders / (the table itself)**, and a level left empty there is one the engine applies to every resource beneath it while egret cannot match it to any. Re-store that assignment on a table or a column." | **YES, exactly** |
| P2 | "…stored at **sales / ord\* / email**, whose "\*" or "?" the engine matches against many resources by a rule egret's database cannot reproduce. Re-store that assignment on the exact table or column it is meant for." | **YES, exactly** |
| P3 | "…stored at **sales / orders / emaıl**, which carries a character whose upper- and lower-case forms egret's database and the engine's matcher need not agree on. Rename that resource, or re-store the assignment without it." | **YES, exactly** (U+0131 confirmed present in the rendered node) |
| P4 | "…the **column** you asked about, "**ıtem**", carries a character whose upper- and lower-case forms … **Nothing is stored wrongly — re-ask for the resource's exact name.**" | **YES, exactly** |

Each names a coordinate that **exists in the data**, and in each case the catalog also
held a perfectly ordinary sibling row (`sales.orders.email`) which the page correctly did
*not* name. The defect round 3 fixed is fixed, measured against the data rather than
against a string: the thing that was broken (a sentence naming a row that did not exist
and offering a remedy already satisfied) cannot now happen for any of the four.

The head sentence keeps the distinction in all four — *"Declined to resolve" is not the
same as there being none, so no verdict here could be relied on* — and the badge reads
**Undecidable**, next to the standing explanation that this is "neither allowed nor
refused". **The product proposition holds in the browser:** "the engine will deny" (P5,
below: *Refused by a deny item · Somebody forbade this*) and "egret cannot tell you"
(Undecidable · declined to resolve) are visibly different answers, worded differently,
badged differently.

### P7 — the two reaches are legible, better than I predicted

Predicted: distinguishable only by careful reading. Observed: the per-request note opens
"**This one IS specific to what you asked**" against the per-catalog "This is **not**
specific to your target", and the capitalised `IS` carries it at a glance. Nothing to fix.

### P6 — `(none)` is right; the REMEDY beside it is the weak half

`(none) / orders / (the table itself)` renders as predicted, and the two kinds of nothing
are distinguishable — `(none)` for a level that is missing, `(the table itself)` for a
tag that is on the table rather than a column. Keep both.

What does not hold up is the sentence's end: **"Re-store that assignment on a table or a
column"** — while the level that is empty is the **schema**, and the assignment *is*
already on a table (`orders`). A steward follows that instruction, re-stores on
`orders`, and nothing changes. It is a weaker version of exactly the defect round 3 fixed:
the coordinate is now right and the remedy still is not, for the one state where the empty
level is not the column. **Recorded as an open item** rather than fixed here, because
fixing copy during a gate run is how a gate stops measuring.

### F3 (NEW, Major) — the page promises mask verdicts "with SP4d-2b", and this IS SP4d-2b

Rendered on every single result, in the `honest-limits` block:

> "This answers about ACCESS only. … The previous version of this page named the masking
> or filtering rule; evaluating the served artifact answers access, and **mask and
> row-filter verdicts arrive with SP4d-2b**."

This branch *is* SP4d-2b. They do not arrive: `ExplainAccess` carries `decision`,
`attribution` and `access_type`, and nothing else; `explain` never calls
`evaluate_data_mask` or `evaluate_row_filter`, though both work — §4 and §7 are built out
of them. So the page tells a steward that the thing they are looking for ships in the
version they are looking at.

The irony is in the same component: the comment immediately above that Alert says a limit
that no longer holds "is worse than no limit, because it teaches a reader to discount the
ones that still do". **Twenty-six green tests cannot see this**, because a test asserts the
string, not whether the string is true. Only `ExplainAccess.tsx:1167` has to change, and
the honest version is a sentence that states the limit without dating it.

### F4 (NEW, Major) — a tag-driven decision beside "Tags … None."

`sp4a10_wh / sp4d1 / t5b / item`, subject `cleared_person` — the ANCESTOR deny the engine
confirmed in 4b:

```text
select   Refused by a deny item
         Somebody forbade this: a deny item in the downloaded policy set matches this
         subject and resource.
         Ranger policy tp-clearance-ancdeny, id 60
Tags egret has recorded on this resource
         None.
```

The decision was made **by a tag policy, through a tag attached to the table**, and the
page's tag list says **None**. Nothing on the page names `ancdeny`, or the assignment on
`sp4d1.t5b` that carries it. The limits block explains the mechanism correctly three
paragraphs lower ("the engine's own enricher rolls tags up the resource hierarchy and this
list does not"), so the page does not *lie* — but the steward's actual question, *which
tag, attached where, caused this*, is unanswered while the handler **already holds the
answer**: `resolved_tags_for_target` returned `ancdeny:Ancestor` for this very request and
the response reports only the exact-match list. A steward who trusts "None." looks for a
column-level assignment that does not exist.

That is Major for the same reason SP4d-2a's two were: the page is the product, and this is
the shape where tag governance is now most likely to be used.

### F5 (NEW, the composite, and the one I would fix first)

`sp4a10_wh / sp4d1 / t4 / item`, subject `admin`:

```text
select   Allowed
         The policy set this engine downloads allows this access.
         Ranger policy acc-t4, id 49
Tags egret has recorded on this resource
         None.
```

Measured four minutes earlier, on the engine this page is describing, with the same
subject and the same resource:

```text
SELECT item FROM sp4a10_wh.sp4d1.t4
  → Query failed: Invalid column mask for 'sp4a10_wh.sp4d1.t4.id': Unexpected parameters …
```

**Allowed · no tags · and no query against that table can run at all.** Each of the three
statements on the page is defensible on its own terms (the access *is* allowed; the
exact-match tag list *is* empty; masks are *documented* as out of scope) and together they
leave a steward with the opposite of the truth. F3, F4 and F1 meet here, which is why this
is the composite rather than a fourth finding: report the resolved tag set (F4), report
the mask verdict the evaluator can already compute (F3), and the page would say what the
engine does.

### One false finding this pass produced, recorded because the method produced it

Mid-run the page appeared to report `tags: ["pii"]` for `t4.item` and `None` for
`t5b.item`, two table-level attachments — an inconsistency that would have been a real
defect. It was mine: the `ScopePicker` **clears the column when the table changes**, so
the request had gone out as the *table* `t4` with no column, where an exact match on
`column_name = ''` does return `pii`. Caught by reading the inputs back out of the DOM
after the fact. A browser pass can manufacture a finding as easily as it can find one, and
the guard is to read the request back, not the form.

---

## 9. What this gate changes, and what it leaves open

**Confirmed, on the engine that is actually enforcing:** the tag mask (case 1), the
row-filter asymmetry between a column-level and a table-level attachment (cases 2 and 4a),
the deny-capable clearance in both directions (cases 3, 4b, 4c), **the table-level
`ANCESTOR` match that no live data exercises** (case 4b, through the access decision, and
4a/4d through the mask), and **the `MATCH_TYPE_COMPARATOR` mask ordering with the same tag
at two levels, not deduped** (case 5). Twelve measurements, ten agreeing, and the two
disagreements are one product finding that the evaluator got *right*.

**Open items this gate adds** (none of them in `egret-ranger-eval`):

1. **F1** — a tag-addressed `MASK` rule plus a table-level tag assignment makes a table
   unreadable on Trino. Reachable from two supported APIs; not live today; needs a refusal
   or a type-aware transformer.
2. **F3** — `ExplainAccess.tsx:1167` promises mask verdicts "with SP4d-2b" on the SP4d-2b
   branch.
3. **F4** — `explain` reports the exact-match tag list and not the resolved one, so a
   tag-driven decision renders beside "Tags … None."
4. **P6** — `stored_level_empty`'s remedy says "on a table or a column" when the empty
   level is the schema.
5. **F2** — the live clearance on tag `tag` is attached to nothing.

**Cleanup.** The private coordinator pod and its ConfigMap are deleted, and **this gate's
own backend (:8090) and Vite (:5290) are stopped** — a left-running stack is exactly the
hazard this task had to work around on arrival, and leaving mine up would have handed the
same trap to the next one. The two stacks that were already on :8080 and :5173 belong to
other sessions and are still running, untouched. The fixture stays in `egret_sp4d2b` so
the gate can be re-run from §1's commands; nothing in the kind cluster, in `egret`, in
`egret_test` or in the shared template was written at any point.

Screenshots of the two page states that matter are at
`.playwright-mcp/sp4d2b-p1-empty-level.png` (the `(none) / orders / (the table itself)`
decline) and `.playwright-mcp/sp4d2b-p8-allowed-on-an-unreadable-table.png` (F5).

---

## 10. The gates, with exit codes

> ⚠ **SNAPSHOT, and read it as one.** The numbers below are what the gates reported
> **at this document's commit** (`7691dae9`). They are not a running tally and they
> will not be updated: later rounds on this branch added tests, so at the tip the
> same suites report MORE — Task 8's fix rounds took perm-compile `--lib` from 25 to
> 27 and `ExplainAccess.test.tsx` from 26 to 31. Chasing the figures through a
> document that records a measurement is how a measurement stops being one; the
> authority for the current totals is `task-8-report.md` and the branch, and the
> authority for what was true when the live gate ran is here. (Whole-branch review,
> L2.)

| gate | command | result |
|---|---|---|
| ranger-eval suites | `cargo test -p egret-ranger-eval --no-fail-fast` | **259 passed / 0 failed across 10 binaries**, rc 0 — the baseline exactly. The tenth binary is the differential at `0 passed … 0.00s`, which is the opt-in gate being silently green, as documented |
| perm-compile | `env -u DATABASE_URL cargo test -p egret-perm-compile --lib` | **25 passed / 0**, rc 0 |
| the page's own suite | `npx vitest run src/routes/governance/__tests__/ExplainAccess.test.tsx` | **26 passed**, rc 0 (the path with `__tests__/`; without it vitest exits 0 on "No test files found") |
| workspace | `cargo check --workspace --all-targets` | rc 0 |
| fmt | `cargo fmt --all -- --check` | rc 0, empty output (the example needed one `cargo fmt` pass first — the first `--check` run DID report a diff, and the `echo $?` after a pipe reported the *tail*'s status, which is how a false green is manufactured) |
| clippy | `cargo clippy -p egret-ranger-eval -p egret-perm-compile --all-targets -- -D warnings` | rc 0 |
| clippy, workspace | `cargo clippy --workspace --all-targets -- -D warnings` | **rc 101 — two `explicit_auto_deref` errors in `crates/egret-workspace/src/members.rs:191,196`.** Not this task's: `git show origin/main:…/members.rs` is **byte-identical** to the branch's at those lines (diffed, not assumed), last touched by the merged SP4c commit `ce21e167`. Reported, not fixed |
| types / lint | `npx tsc --noEmit` ; `npm run lint` | rc 0 ; rc 0 (0 errors, 177 pre-existing warnings — `eslint .` and `eslint src` give the same 177, so the new config file contributes none) |
| OpenAPI drift | `cargo test -p egret-server --lib committed_openapi_json_is_current` | **1 passed**, 5 filtered out, rc 0 — run WITH `--lib`, or it reports "0 passed, 1 filtered out" and exits 0 |
| differential | **not run, and the reason is a code path rather than a judgement** | `git diff --name-only 20f4febc..HEAD` is exactly `examples/live_gate.rs`, this doc and the Vite config: **no file under `crates/egret-ranger-eval/src/` or `crates/egret-perm-compile/src/` changed** (`git diff --name-only 20f4febc -- <both src dirs> \| wc -l` → 0). `tests/differential.rs` links the library, which is byte-identical, and an `examples/` target is linked into neither the lib nor any test. Nothing this task changed can alter what the evaluator returns |

**No product code changed in this task.** The only Rust edit is the prediction tool
(`examples/live_gate.rs`), which gained a resolved-tag spec. Every finding above is
recorded, not fixed: fixing copy or a writer during a gate run is how a gate stops
measuring.
