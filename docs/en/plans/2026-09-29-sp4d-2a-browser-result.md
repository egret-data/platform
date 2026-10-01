# SP4d-2a Task 9 — opening `/governance/explain-access` in a browser

**Branch** `feat/sp4d-2a-explain-the-artifact`, at `afd08165` (Task 8's commit) plus this
document. **Measured** 2026-09-30.

The page was rewritten by the same person who wrote its tests, and 15 of those tests pass.
This document is the first thing that is neither. It exists because the `clearance_required`
defect this package fixes shipped through SP4c's live *engine* gate, 35 commits and a full
review — a clearance denial rendered as the raw string `clearance_required` with
"Unrecognized reason class." for two releases, dropping the gating tag and the clearing
roles, because nobody opened the page.

**The method, and it is the deliverable.** Every expectation below was written down and
**committed** before the page was opened — the commit that adds this file's *Predictions*
section contains no observations, and the observations arrive in a later commit. A
prediction recorded after its observation proves nothing; this programme has a recorded
instance of exactly that being caught.

---

## 1. The stack, exactly as run

The kind cluster's egret runs `main`, which does **not** carry this branch's `explain`. A
browser pointed at `http://egret.localtest.me` would exercise the OLD endpoint and tell us
nothing, so the stack is this branch's own backend against its own database.

### Which database each step touched

| step | database | why it is safe |
|---|---|---|
| `egret migrate` / `egret init` / `egret serve` | **`egret_sp4d2a`** (local docker `egret-dev-postgres-1`), created for this task | not the dev database `egret`, not `egret_test`, not any `_sqlx_test_*` |
| reading the live fixture's shape | kind cluster's `egret` | **read-only `SELECT`s only.** No write, no `pg_dump`, no schema change. `demo-trino` and its data untouched. |
| everything else | `egret_sp4d2a` | — |

`egret_sp4d2a` is a disposable scratch database created by this task. Nothing else on this
machine uses it.

### Commands

```bash
# 0. disk check first — this machine has had Docker killed by a full disk
df -h /                       # 13Gi available before, 12Gi after (the scratch DB is 13MB;
                              # the rest is the warm cargo target dir this worktree already had)

# 1. the local dev stack was already up (deploy/docker-compose.dev.yml):
#    egret-dev-postgres-1, egret-dev-minio-1, egret-dev-starrocks-1, egret-dev-keycloak-1

# 2. a fresh database, migrated and bootstrapped BY THIS BRANCH's binary
cargo build -p egret-bin                      # binary is target/debug/egret
docker exec egret-dev-postgres-1 psql -U egret -d postgres -c "CREATE DATABASE egret_sp4d2a"
EGRET_DATABASE__URL=postgres://egret:egret@localhost:5432/egret_sp4d2a ./target/debug/egret migrate
EGRET_DATABASE__URL=postgres://egret:egret@localhost:5432/egret_sp4d2a \
  EGRET_ADMIN_EMAIL=admin@egret.local EGRET_ADMIN_PASSWORD='Admin12345!' ./target/debug/egret init

# 3. serve on :8099 — NOT :8080. A stale `egret serve` from the SP4b-2 gate (2026-09-27,
#    ./target-check/debug/egret, database egret_sp4b) still holds :8080, and a stale Vite
#    still holds :5173 pointing at it. Using those would have measured the OLD endpoint,
#    which is the exact trap this task exists to avoid. Neither was killed.
EGRET_DATABASE__URL=postgres://egret:egret@localhost:5432/egret_sp4d2a \
EGRET_SERVER__HOST=127.0.0.1 EGRET_SERVER__PORT=8099 \
EGRET_OIDC__ENABLED=false \
EGRET_RANGER__ENABLED=true EGRET_RANGER__AUTH_TOKEN=dev \
EGRET_SECURITY__SECRET_KEY=<dev key from .env> EGRET_SECRETS__MASTER_KEY=<same> \
  ./target/debug/egret serve

# `EGRET_RANGER__ENABLED=true` is what makes the backend seed the three service-defs
# (`trino` 203, `tag` 204, `starrocks` 205) into policy.authz_service_defs at boot. Without
# it policy.authz_services has no `type` to point at and the whole fixture is unbuildable.

# 4. the frontend, on :5199, proxying /api to :8099
#    frontend/vite.config.ts hardcodes target :8080, so a SCRATCH config was used
#    (frontend/vite.config.sp4d2a.mts — identical except port 5199 and target 8099) and
#    deleted afterwards, leaving `git status` clean.
cd frontend && npx vite --config vite.config.sp4d2a.mts

# 5. sign in at http://localhost:5199 as admin@egret.local / Admin12345!
```

### The fixture, and what is real about it

There is no k8s here, so the objects a real engine provisioning would create were seeded
into `egret_sp4d2a` directly. Every one is a row the compiler and the evaluator read.

| object | rows |
|---|---|
| `policy.authz_service_defs` | `trino`, `tag`, `starrocks` — seeded by the backend at boot from its own embedded JSON |
| `policy.authz_services` | `sp4d2a-trino` (id 1, `trino`, `generated`, `{"tagService":"sp4d2a-trino_tag"}`), `sp4d2a-trino_tag` (id 2, `tag`), `sp4d2a-sr` (id 3, `starrocks`, `generated`) |
| `registry.catalogs` | `sp4d2a_wh` (`11111111-…`, iceberg, warehouse `77777777-…`), `sp4d2a_orphan` (`22222222-…`) |
| `engine_mgmt.engines` | `sp4d2a-trino-eng`→svc 1, `sp4d2a-sr-eng`→svc 3, `sp4d2a-nogov-eng`→**NULL** |
| `registry.engine_catalog_attachments` | trino-eng↔`sp4d2a_wh`, sr-eng↔`sp4d2a_wh`, nogov-eng↔`sp4d2a_orphan` |
| `catalog.warehouses` + `storage.storage_connections` | one each, so `registry.catalogs.warehouse_id` can be non-NULL — `catalog_tag_assignments` and the StarRocks per-column clearance expansion both join through it |
| `tags` | `sp4d2a_ssn` / "SP4D2A Restricted" |
| `classification_policies` | `sp4d2a_ssn` gated on role `sp4d2a_cleared` |
| `roles` + `users` + `access_grants` | role `sp4d2a_cleared`, user `cleared_person` holding it — so the clearance's exception list is non-empty |
| `catalog_tag_assignments` | `sales.orders.ssn` and `sales.orders` (table level) |

So `sp4d2a_wh` is reached by **two profiles of different engine types**, and `sp4d2a_orphan`
is reached by an engine with **no policy profile at all** — the two live shapes the brief
names (`sp4a10_wh` is served by kind's profile 52/Trino and 55/StarRocks; the live cluster
also has ~25 catalogs whose only engine attachment has `authz_service_id IS NULL`).

**The thirteen seeded `egret-admin` policies per profile are not invented.** They are
`seed_admin_all_access`'s real output, and that function is only ever called from
`egret_engine_mgmt::engines` during a k8s provisioning, which cannot run here. So the 13
Trino bodies were copied **verbatim** from the live kind cluster's profile 52
(`sp3c2-trino-live`) and the 13 StarRocks bodies verbatim from profile 55
(`sp4a-sr-crosscheck`), with **only the `service` field renamed** to the local profile.
Their `policyItems[0].users` is already `["admin"]`, the same username this fixture's
bootstrap admin gets, so not a byte of the grant itself was touched.

Everything downstream was made by the **real compiler** through the **real HTTP API**, not
by hand:

- `POST /api/control-plane/perm/grants` — allow read for user `analyst` on
  `sp4d2a_wh.sales.orders` → `{"services":[1,3],"created":4,…}` (the grant on both profiles,
  plus the clearance the gate above had been waiting for).
- `POST /api/control-plane/perm/rules` — row-filter `sp4d2a-filter-orders` on the path, and
  mask `sp4d2a-mask-ssn` on tag `sp4d2a_ssn` → the tag-addressed one lands in the **tag
  service**, which is what gives the Trino profile a non-empty `tagPolicies` block.

Two policies were inserted by hand, on purpose, because that is what an operator writing
straight into Ranger looks like and no egret surface can produce one on a `generated`
profile (`refuse_if_generated`). The live cluster's own profile 52 carries five of exactly
this kind (`t10-exec`, `t10-func`, `t10-imp`, `t10-sysinfo`, `t10-sysprop`), and one of them
was read for shape:

- `zz-sp4d2a-operator-hand-written` — allows `select` to user `contractor` on
  `sp4d2a_wh.raw.ledger`; on both profiles (id 27 Trino, id 37 StarRocks).
- `zz-sp4d2a-operator-wrong-case` — the same, on `raw.journal`, but with the access type
  spelled `SELECT` while the StarRocks def declares `select` (id 38).

Resulting policies, for reading the observations against:

```text
 19 sp4d2a-sr        egret-admin - catalog, database, table, column
 27 sp4d2a-trino     zz-sp4d2a-operator-hand-written
 28 sp4d2a-trino     perm-grant-e7f62547-dbc3-42b7-87b6-e1d679627983
 29 sp4d2a-trino_tag perm-clearance-88888888-8888-4888-8888-888888888888
 30 sp4d2a-sr        perm-clearance-88888888-…-888888888888-sp4d2a_wh.sales.orders.ssn
 31 sp4d2a-sr        perm-grant-e7f62547-dbc3-42b7-87b6-e1d679627983
 33 sp4d2a-trino     perm-rule-b0f8efb1-57d6-49ea-921f-12d79617b1f9
 34 sp4d2a-sr        perm-rule-b0f8efb1-57d6-49ea-921f-12d79617b1f9
 35 sp4d2a-trino_tag perm-rule-488c2637-e565-4cc2-a993-6c7d47525085
 36 sp4d2a-sr        perm-rule-488c2637-…-4c7d47525085-sp4d2a_wh.sales.orders.ssn
 37 sp4d2a-sr        zz-sp4d2a-operator-hand-written
 38 sp4d2a-sr        zz-sp4d2a-operator-wrong-case
   + 13 `egret-admin - …` under sp4d2a-trino and 13 under sp4d2a-sr
```

### One consequence of the fixture, stated before any observation

Once the tag service holds **any** policy, `explain` passes `resolved_tags: None` and the
evaluator refuses every Trino access with `Unmodelled::TagPolicies`. So **every Trino
verdict in this fixture is `Undecidable`**, and the decided shapes — a grant, a clearance, a
raw policy, a seeded admin policy, "no policy matched" — can only be observed on the
StarRocks profile, which has no tag service. That is not a fixture artefact; it is what a
steward will see the moment one tag-addressed rule exists anywhere in the installation.

---

## 2. Predictions — written and committed BEFORE the page was opened

Derived from the payloads and from reading `ExplainAccess.tsx`. What is under test is what
the **rendered page** does with them, and whether a steward reading it would be told the
truth.

### P1 · one catalog, two profiles, two engine types · `admin` / read / `sp4d2a_wh.sales.orders`

1. **Two** `explain-profile` sections, in `service_id` order: `sp4d2a-trino` then `sp4d2a-sr`.
2. Each names its own profile, its engine as an outline badge (`Trino` / `StarRocks` — display
   spelling, while the wire stays `trino` / `starrocks`) and `service id 1` / `service id 3`.
3. **No aggregate badge.** Exactly two `decision-badge`s for two access rows; no page-level
   "Allowed"/"Not allowed" and no "only a summary" text.
4. Trino row: `select` + **Undecidable** (not a refusal colour), hint "Neither allowed nor
   refused: …", and one cause **as prose with the count as a number**:
   "The downloaded policy set carries 2 tag policies that could decide this, which egret does
   not evaluate yet — so this access cannot be decided." No `TagPolicies`, no `Unmodelled`,
   no `{`.
5. StarRocks row: `select` + **Allowed**, decided by `egret-admin - catalog, database, table,
   column` — see P3b for how that must read.
6. `Tags considered`: one outline badge `sp4d2a_ssn` (the table-level assignment).
7. The honest-limits block shows four paragraphs and **does not** claim explain cannot see
   hand-written Ranger policy.

### P2 · a catalog served by nothing · `admin` / read / `sp4d2a_orphan`

1. A `no-profile-state` alert with an **outline** badge "Nothing is enforcing" — not a
   destructive/refusal colour.
2. Prose that says in words it is not a denial: "No policy profile serves this catalog, so no
   engine plugin is ever consulted about it. This is not a denial — nothing is enforcing here,
   which is a different fact from a policy refusing the access. Attach the catalog to a
   profile if it is meant to be governed."
3. **Zero** profile sections and **zero** decision badges. The word "Denied" appears nowhere
   in the result.
4. `Tags considered` → "None." (a catalog-only target can carry none).

### P3a · an allow that comes from a grant · `analyst` / read / `sp4d2a_wh.sales.orders`

StarRocks row: **Allowed**, decided by `perm-grant-e7f62547-…`, rendered as a link
`View grant e7f62547-dbc3-42b7-87b6-e1d679627983` → `/governance/data-permissions?grant=…`,
beside `Ranger policy perm-grant-e7f62547-…, id 31`. Trino row: Undecidable, as P1.

### P3b · the headline win: an admin allowed by a seeded `egret-admin` policy · from P1

The StarRocks row's deciding policy is minted by egret itself, so
`attribute_policy_name` returns `None` and it reaches `RawPolicy`. It must read as the
**reassuring explanation it is, not as an alarm**:

- a neutral `secondary` badge "**Seeded admin policy**" with a shield, `data-kind="seeded-admin"`
- "egret seeds this policy itself, outside the permission model: auto-bind puts thirteen
  admin-all-access hierarchies onto every generated profile, granting its platform admins all
  access. **For an administrator this is the expected explanation.**"
- `Ranger policy egret-admin - catalog, database, table, column, id 19`
- **not** the "Outside the permission model" wording, which is the same block's other face.

This is the blindness the package removes: the old evaluator could not see one of these and
answered "no grant matched" while the engine allowed.

### P4 · the defect that shipped twice: a clearance denial · `analyst` / read / `sp4d2a_wh.sales.orders.ssn`

StarRocks row: **Refused by a deny item** (the clearance denies `{public}` with the cleared
users as `denyExceptions`, and `analyst` is not cleared), with **all three facts**:

1. a warning badge "**Clearance required**" with a key icon — **not** the raw string
   `clearance_required`, and **not** "Unrecognized reason class."
2. the **gating tag**: `Gating tag sp4d2a_ssn` (`tags.key`, not `tags.name`)
3. the **clearing roles**, with OR semantics stated: "Cleared by holding any one of
   `sp4d2a_cleared`."
4. plus the expansion locator `expansion sp4d2a_wh.sales.orders.ssn` — one `perm.rules`-shaped
   object becomes N StarRocks policies and the locator is the only thing saying which decided —
   and `Ranger policy perm-clearance-88888888-…-sp4d2a_wh.sales.orders.ssn, id 30`.

A grant allows `analyst` `select` on the table; the deny must still win, and the page must
show the *clearance* as the decider, not the grant.

### P5 · an operator-written raw policy, named and not dropped · `contractor` / read / `sp4d2a_wh.raw.ledger`

StarRocks row: **Allowed**, and the deciding policy is reported by its Ranger name and id,
marked outside the permission model, **unlinked**:

- outline badge "**Outside the permission model**" with a server icon, `data-kind="unowned"`
- "egret's compiler did not mint this policy, so it is outside the permission model and has
  no governance page to open. It is reported by its Ranger name and id, because it decided
  this access."
- `Ranger policy zz-sp4d2a-operator-hand-written, id 37`
- **no anchor** anywhere inside that block.

### P6 · a second `Undecidable` cause, also as prose · `contractor` / read / `sp4d2a_wh.raw.journal`

StarRocks row: **Undecidable**, cause rendered as
"Ranger policy “zz-sp4d2a-operator-wrong-case” (policy id 38) names the access type “SELECT”
in a different case than the service definition does." Prose, a real id, no Debug token.

### P7 · `write` is several evaluations and they are never collapsed · `admin` / write / `sp4d2a_wh.sales.orders`

Trino: **2** access rows, `insert` and `delete`. StarRocks: **3**, `insert`, `update`,
`delete` — each answered separately, five decision badges in total and still no aggregate
one. The form says so before submitting: "Ranger has no `write`: it is evaluated as insert
and delete on Trino, and insert, update and delete on StarRocks, each answered separately
below."

### P8 · "nobody said yes" ≠ "somebody said no" — and a predicted defect · `nobodyatall` / read / `sp4d2a_wh.sales.orders`

StarRocks row: a **warning** badge "No policy matched" (not destructive), hint "Nobody granted
this — not the same as somebody forbidding it. Ranger's own audit shows “-” for exactly this
case."

**And I predict a defect underneath it.** `egret-ranger-eval` returns
`Decision::Denied { policy_id: None, policy_name: String::new(), reason: NoPolicyMatched }`
(`eval.rs:759-763`) — there is no deciding policy, by construction. But `AccessRow` sends
every non-`Undecidable` decision to `DecidedBy`, and with no attribution `DecidedBy` renders
`RawPolicy`. So I expect the page to print, under a verdict that means *no policy matched*:

> Outside the permission model
> egret's compiler did not mint this policy, so it is outside the permission model and has no
> governance page to open. It is reported by its Ranger name and id, **because it decided this
> access.**
> Ranger policy ⟨empty⟩, id unknown

which asserts a nonexistent policy decided the access, and invites a steward to go looking
for a hand-written Ranger policy that is not there. If that is what renders, it is a finding.

---

## 3. Observations

Driven in Chromium via Playwright against `http://localhost:5199/governance/explain-access`,
signed in as `admin@egret.local`. Every "rendered" block below is the result element's
`innerText`, read out of the live DOM, not retyped. Every payload is the response body of the
`POST /api/control-plane/perm/explain` that produced it, read from the browser's own network
log.

### P1 · two profiles, two engine types · **PASS**

Payload:

```json
{"outcome":"per_profile","profiles":[
 {"service_id":1,"service_name":"sp4d2a-trino","engine":"trino","accesses":[
   {"access_type":"select","decision":{"kind":"undecidable","why":[{"kind":"envelope_construct",
     "construct":{"kind":"tag_policies","block":{"kind":"policies","count":2}}}]},"attribution":null}]},
 {"service_id":3,"service_name":"sp4d2a-sr","engine":"starrocks","accesses":[
   {"access_type":"select","decision":{"kind":"allowed","policy_id":19,
     "policy_name":"egret-admin - catalog, database, table, column"},"attribution":null}]}],
 "tags":["sp4d2a_ssn"]}
```

Rendered, verbatim:

```text
sp4d2a-trino
Trino
service id 1

Evaluated over the policy set this profile’s plugin downloads.

select
Undecidable

Neither allowed nor refused: something stands between the evaluator and a defensible answer,
and it is named below rather than guessed at in either direction.

The downloaded policy set carries 2 tag policies that could decide this, which egret does not
evaluate yet — so this access cannot be decided.
─────
sp4d2a-sr
StarRocks
service id 3

Evaluated over the policy set this profile’s plugin downloads.

select
Allowed

The policy set this engine downloads allows this access.

Seeded admin policy

egret seeds this policy itself, outside the permission model: auto-bind puts thirteen
admin-all-access hierarchies onto every generated profile, granting its platform admins all
access. For an administrator this is the expected explanation.

Ranger policy egret-admin - catalog, database, table, column, id 19
```

Measured from the DOM: `explain-profile` = **2**, `decision-badge` = **2** (so there is no
aggregate badge over the two), `no-profile-state` absent, `raw-policy` `data-kind` =
`seeded-admin` with **0** anchors inside it. Badge classes: Undecidable → `bg-sky-500`,
Allowed → `bg-green-…` — three distinct colours across the run (sky / green / destructive /
yellow-600), so `Undecidable` is neither an allow nor a refusal by colour either. The result
text contains no `Denied`, no `Unrecognized`, no `TagPolicies`, no `{`. `Tags considered`
renders one badge `sp4d2a_ssn`; the four honest-limits paragraphs are all present and none of
them claims explain cannot see hand-written Ranger policy.

### P2 · a catalog served by nothing · **PASS**

Payload: `{"outcome":"no_profile_serves_the_catalog","profiles":[],"tags":[]}`

Rendered, verbatim:

```text
Nothing is enforcing

No policy profile serves this catalog, so no engine plugin is ever consulted about it. This is
not a denial — nothing is enforcing here, which is a different fact from a policy refusing the
access. Attach the catalog to a profile if it is meant to be governed.

Tags considered

None.
```

`explain-profile` = **0**, `decision-badge` = **0**, `no-profile-state` present. Its alert
classes are `bg-background text-foreground` — the neutral variant, **not** `destructive` — and
the badge carries `text-foreground` with no `bg-`, i.e. `outline`. Neither "Denied" nor
"Refused" appears anywhere in the result. So "nothing is enforcing" and "refused" do not look
alike; the layout is not lying.

### P3a · an allow attributed to a grant · **PASS**

Payload (StarRocks arm): `{"kind":"allowed","policy_id":31,"policy_name":"perm-grant-e7f62547-…"}`
with `"attribution":{"kind":"grant","grant_id":"e7f62547-dbc3-42b7-87b6-e1d679627983"}`.

Rendered: `View grant e7f62547-dbc3-42b7-87b6-e1d679627983` /
`Ranger policy perm-grant-e7f62547-dbc3-42b7-87b6-e1d679627983, id 31`, the link's `href`
read off the DOM as `/governance/data-permissions?grant=e7f62547-dbc3-42b7-87b6-e1d679627983`.

### P3b · the headline win: an admin allowed by a seeded `egret-admin` policy · **PASS**

It is the P1 StarRocks block above, and it reads as the **reassuring explanation**, not as an
alarm: the neutral `secondary` badge "Seeded admin policy", `data-kind="seeded-admin"`, the
sentence "For an administrator this is the expected explanation.", and the policy named
(`egret-admin - catalog, database, table, column`, id 19). The old model-only evaluator could
not see this policy at all and answered "no grant matched" while the engine allowed; the page
now names it.

**And the two profiles name DIFFERENT seeded hierarchies** — Trino decided on
`egret-admin - catalog, schema, table` (id 5, measured in the tag-free run below) and
StarRocks on `egret-admin - catalog, database, table, column` (id 19). That is each engine's
own resource vocabulary, from each profile's own download; it is not one answer copied twice.

### P4 · the defect that shipped twice: a clearance denial · **PASS**

Payload (StarRocks arm):

```json
{"access_type":"select",
 "decision":{"kind":"denied","policy_id":30,
   "policy_name":"perm-clearance-88888888-8888-4888-8888-888888888888-sp4d2a_wh.sales.orders.ssn",
   "reason":"deny_policy_item"},
 "attribution":{"kind":"clearance","tag_id":"88888888-8888-4888-8888-888888888888",
   "tag":"sp4d2a_ssn","required_roles":["sp4d2a_cleared"],"locator":"sp4d2a_wh.sales.orders.ssn"}}
```

Rendered, verbatim:

```text
select
Refused by a deny item

Somebody forbade this: a deny item in the downloaded policy set matches this subject and resource.

Clearance required
Gating tag sp4d2a_ssn
expansion sp4d2a_wh.sales.orders.ssn
Ranger policy perm-clearance-88888888-8888-4888-8888-888888888888-sp4d2a_wh.sales.orders.ssn, id 30

Cleared by holding any one of sp4d2a_cleared.
```

All three facts are there: a real label ("Clearance required", warning badge, `bg-destructive`
on the verdict badge), the **gating tag** (`sp4d2a_ssn` — the `tags.key`), and the **clearing
roles** with OR semantics stated. The string `clearance_required` does not appear in the
rendered text, and neither does "Unrecognized reason class.". The expansion locator is
rendered too, which is the only thing that says which of N per-column policies decided. A
grant allows `analyst` `select` on the table and the deny still won, with the *clearance*
named as the decider rather than the grant.

**This is the defect that shipped for two releases. It is fixed, and it was measured in a
browser, not inferred from the commit that claims to fix it.**

### P5 · an operator-written raw policy, named and not dropped · **PASS**

Payload (StarRocks arm): `{"kind":"allowed","policy_id":37,"policy_name":"zz-sp4d2a-operator-hand-written"}`,
`"attribution":null`.

Rendered, verbatim:

```text
select
Allowed

The policy set this engine downloads allows this access.

Outside the permission model

egret’s compiler did not mint this policy, so it is outside the permission model and has no
governance page to open. It is reported by its Ranger name and id, because it decided this access.

Ranger policy zz-sp4d2a-operator-hand-written, id 37
```

`data-kind="unowned"`, **0** anchors inside the block. Reported by Ranger name and id,
unlinked, marked outside the model, not dropped.

### P6 · a second `Undecidable` cause, as prose · **PASS**

Payload (StarRocks arm): `{"kind":"undecidable","why":[{"kind":"access_type_case_mismatch",
"policy_id":38,"policy_name":"zz-sp4d2a-operator-wrong-case","access_type":"SELECT"}]}`

Rendered, verbatim:

```text
Ranger policy “zz-sp4d2a-operator-wrong-case” (policy id 38) names the access type “SELECT”
in a different case than the service definition does.
```

One `undecidable-cause` node, prose, a real id, no Debug token.

### P7 · `write` is several evaluations, never collapsed · **PASS**

`explain-access` rows = **5**, `decision-badge` = **5**, and still no aggregate badge. Trino:
`insert`, `delete` (both Undecidable). StarRocks: `insert`, `update`, `delete`, each
**Allowed** by `egret-admin - catalog, database, table` (id 18). The form said so before
submitting: "Ranger has no write: it is evaluated as insert and delete on Trino, and insert,
update and delete on StarRocks, each answered separately below."

### P8 · "nobody said yes" ≠ "somebody said no" — **PASS on the verdict, FINDING 1 underneath**

Payload (StarRocks arm):
`{"kind":"denied","policy_id":null,"policy_name":"","reason":"no_policy_matched"}`,
`"attribution":null`.

Rendered, verbatim:

```text
select
No policy matched

Nobody granted this — not the same as somebody forbidding it. Ranger’s own audit shows “-”
for exactly this case.

Outside the permission model

egret’s compiler did not mint this policy, so it is outside the permission model and has no
governance page to open. It is reported by its Ranger name and id, because it decided this access.

Ranger policy , id unknown
```

The badge is `bg-yellow-600` (warning), kept apart from the two destructive denies — that half
is right, and it is the distinction Ranger's own audit throws away. **The block under it is
the finding; see Finding 1.** The prediction that this would happen was written and committed
before the page was opened.

### F1 · falsification: is the tag-policy count real, or a string? · **PASS**

The claim "2 tag policies" is only worth something if it can be made to say something else.
One of the two tag-service policies was deleted (`DELETE FROM policy.authz_policies WHERE id
= 35`) and the same request re-run. Payload went to `"block":{"kind":"policies","count":1}`
and the page rendered, verbatim:

```text
The downloaded policy set carries 1 tag policy that could decide this, which egret does not
evaluate yet — so this access cannot be decided.
```

The count is content-derived **and** the noun agrees with it ("1 tag policy", "2 tag
policies"). Not a hardcoded sentence.

### F2 · falsification: can the Trino profile decide at all? · **PASS**

With the tag service emptied (the second policy deleted too), the same `admin` request came
back `allowed` on **both** profiles, by two different policies — Trino
`egret-admin - catalog, schema, table` (id 5), StarRocks
`egret-admin - catalog, database, table, column` (id 19). So the Trino `Undecidable` in P1–P7
is the tag-policy refusal doing its job, not the Trino arm being broken.

### F3 · the case per-profile exists for: two profiles DISAGREEING · **PASS**, and it matters

With the Trino tag service empty but the StarRocks per-column clearance still in place,
`analyst` / read / `sp4d2a_wh.sales.orders.ssn` rendered:

```text
sp4d2a-trino · Trino · service id 1
  select   Allowed
           View grant e7f62547-dbc3-42b7-87b6-e1d679627983
           Ranger policy perm-grant-e7f62547-dbc3-42b7-87b6-e1d679627983, id 28
─────
sp4d2a-sr · StarRocks · service id 3
  select   Refused by a deny item
           Clearance required
           Gating tag sp4d2a_ssn
           expansion sp4d2a_wh.sales.orders.ssn
           Ranger policy perm-clearance-…-sp4d2a_wh.sales.orders.ssn, id 30
           Cleared by holding any one of sp4d2a_cleared.
```

Two `decision-badge`s, no third. **No single word would have been honest here** — the same
subject on the same column is allowed by one engine's download and refused by the other's,
because a Trino clearance is a TAG policy and a StarRocks clearance is a per-column resource
policy. An aggregate "Allowed" over this would have been the most misleading thing on the
page, and the page does not have one. This is the strongest evidence in the run that the
per-profile shape is the right one.

---

## 4. Findings

### Finding 1 (Major) — `no_policy_matched` is rendered as a hand-written Ranger policy that does not exist

**Measured**, P8 above. `egret-ranger-eval` returns, by construction,

```rust
// crates/egret-ranger-eval/src/eval.rs:759-763
None => Decision::Denied {
    policy_id: None,
    policy_name: String::new(),
    reason: DenyReason::NoPolicyMatched,
},
```

— there is **no deciding policy**, that is what the verdict means. But `AccessRow` sends every
non-`Undecidable` decision to `DecidedBy`, and `DecidedBy` with no attribution renders
`RawPolicy`. So the page prints, under a verdict that means *nothing matched*:

> **Outside the permission model**
> egret's compiler did not mint this policy, so it is outside the permission model and has no
> governance page to open. It is reported by its Ranger name and id, **because it decided this
> access.**
> Ranger policy ⟨empty⟩, id unknown

Two separate untruths in four lines. It asserts a policy decided the access when none did, and
it tells a steward the policy is *outside the permission model* — which sends them looking
through raw Ranger for a hand-written policy that is not there. The literal rendering is also
broken text: `Ranger policy , id unknown` (confirmed in the DOM as
`Ranger policy <span class="font-mono"></span>, id <span class="font-mono">unknown</span>`).

This is the same **class** of error as D10's `clearance_required` defect — a decision variant
falling into a branch written for a different variant — reached through the new mechanism
rather than the old one.

**Why no test caught it, and this is the more important half.** The test file's fixture invents
a shape the backend cannot produce:

```ts
// frontend/src/routes/governance/__tests__/ExplainAccess.test.tsx:81
decision: { kind: "denied", policy_id: null, policy_name: "unnamed", reason: "no_policy_matched" },
```

`"unnamed"` is not a value `explain` can send; the server sends `""`. With a non-empty name the
`RawPolicy` block reads plausibly and nothing looks wrong, so the fixture hides exactly the
rendering it is standing in for. This is the trap this package's own Task 1 brief names — *"a
fixture drawn from the same imagination as the code inherits its blind spots"* — appearing on
the frontend side of the same package.

**The minimal fix** (not applied — see §5): in `AccessRow`, a `denied` decision whose
`reason` is `no_policy_matched` has no decider, so render no `DecidedBy` block at all (the hint
already says everything true about it); and change the test fixture's `policy_name` to `""`,
which is what the server sends.

### Finding 2 (Major) — the clearance block asserts "it declares only groups" about a gate it has just said it cannot see

**Measured.** `ExplainAttribution::Clearance` reconstructs `tag` and `required_roles` from the
`tags` row the policy name points at (`routes.rs:1652-1667`). When that row is gone — which the
page's own copy says happens, "a compiled clearance can outlive its gate until the next
recompile" — `tag` is `None` **and** `required_roles` is `[]`. The page handles the first
correctly and then, for the second, commits to one of its two possible causes.

Forced live: the mask rule referencing the tag was deleted through the real API, then
`DELETE FROM tags WHERE id='8888…'` (which cascades `classification_policies`), leaving policy
30 compiled. Payload:

```json
"attribution":{"kind":"clearance","tag_id":"88888888-8888-4888-8888-888888888888",
               "tag":null,"required_roles":[],"locator":"sp4d2a_wh.sales.orders.ssn"}
```

Rendered, verbatim, in one block:

```text
Clearance required
Gating tag unknown — the tag row is gone until the next recompile
expansion sp4d2a_wh.sales.orders.ssn
Ranger policy perm-clearance-88888888-8888-4888-8888-888888888888-sp4d2a_wh.sales.orders.ssn, id 30

No role clears this gate: it declares only groups, and no engine egret provisions can see a group.
```

The two sentences contradict each other. The first says nothing is known about the gate; the
second asserts a specific fact about it. And the asserted fact is **false**: this gate declared
`required_roles: ['sp4d2a_cleared']` and no groups at all. A steward reads "it declares only
groups" and concludes the classification is misconfigured, when the truth is that its row was
deleted and the engine is still enforcing the old compilation. Worse, the clearing role — the
fact D10 exists to surface — is silently absent rather than reported as unknown.

The page's own doc comment already knows there are two causes ("`required_roles` is empty for
the same reason, and also for a gate that declares only `required_groups`"). The rendering
picks one.

**The minimal fix** (not applied — see §5): the groups-only sentence is only sayable when
`tag !== null`. With `tag === null` the honest sentence is that the gate's roles are unknown
for the same reason its tag is. Two branches, not one. `ExplainAttribution::Clearance` could
also make this unrepresentable by carrying `required_roles: Option<Vec<String>>`, so "the gate
says no roles" and "the gate could not be read" cannot arrive as the same value.

**Applied** in SP4d-2b Task 6, by the `Option` route rather than the two-branch one:
`attribution_for` sends `null` when it had no `tags.key` to look a gate up by, and the page's
three sentences all read that one field. The residual conflation is stated there and here:
`Some([])` still covers both a groups-only gate and a deleted gate row.

### Minor 1 — the honest-limits block talks about "the verdicts above" when there are none

In the no-profile state (P2) the block still renders "…— the verdicts above are what the engine
reaches for this username alone" and "…this answers why the subject is allowed or refused as
they are", under a result whose whole point is that nobody is deciding anything. Not false
exactly, but it reads as though a verdict were somewhere on the page. Cosmetic.

### Minor 2 — "Cleared by holding any one of `sp4d2a_cleared`." with a single role

Correct, and the OR semantics are worth stating explicitly for the multi-role case; with one
role it reads a little oddly. Cosmetic, no change recommended.

### Minor 3 — "Tags considered" (already flagged by Task 8's own report)

The label still says "considered" for a list the page's next paragraph says decides nothing.
Task 8's report raised this as its own concern 2; the browser confirms the two sit adjacent,
`Tags considered` / `sp4d2a_ssn` / "They are shown for information and decide nothing here". A
reader who stops at the label gets the wrong idea. Worth the reword Task 8 proposed.

### Non-finding, worth knowing: one tag-addressed rule anywhere makes every Trino answer `Undecidable`

Not a defect — it is the sound refusal Task 1 built, and the page explains it in prose with a
real count. But it is worth a steward knowing the scale: because `explain` passes
`resolved_tags: None` unconditionally, a **single** tag-selected rule or clearance anywhere in
the installation turns *every* Trino verdict on *every* catalog into `Undecidable`. In this
fixture that was two policies, and all seven Trino answers were refusals to answer. The
StarRocks side is unaffected (no tag service). The page never pretends otherwise, and the
alternative — answering from egret's non-hierarchy-complete tag resolution — fails open, so
this is the right trade. It is simply a larger blast radius than "a profile whose tag block
carries a policy" suggests.

---

## 5. What was NOT done, and why

**Neither finding was fixed.** This is the browser gate; its job is to be the first reader who
is not the author, and a gate that edits the page to match its own expectation destroys the
only evidence it was there to produce. Both findings are recorded above with the exact minimal
change each needs, including the test-fixture correction that would let Finding 1 be caught by
a test at all. Nothing in `ExplainAccess.tsx`, `explain_wire.rs`, `routes.rs`,
`egret-ranger-eval` or the generated client was touched by this task.

## 6. Verdict

| # | case | result |
|---|---|---|
| P1 | a catalog served by two profiles of different engine types | **PASS** |
| P2 | a catalog served by nothing, visually distinct from a refusal | **PASS** |
| P3a | an allow attributed to a grant, with a working deep link | **PASS** |
| P3b | an admin allowed by a seeded `egret-admin` policy, reading as reassurance | **PASS** |
| P4 | a clearance denial: real label + gating tag + clearing roles | **PASS** — the D10 defect is gone |
| P5 | a raw operator policy, named by Ranger name and id, unlinked, not dropped | **PASS** |
| P6 | an `Undecidable` cause as prose, not a Debug string | **PASS** |
| P7 | `write` answered per access type, never collapsed | **PASS** |
| P8 | "no policy matched" kept apart from a deny | **PASS** on the verdict, **FINDING 1** on the block beneath it |
| F1 | the tag-policy count is content-derived, singular/plural included | **PASS** |
| F2 | the Trino profile can decide when nothing refuses | **PASS** |
| F3 | two profiles disagreeing, with no dishonest aggregate | **PASS** |
| — | a clearance whose tag row is gone | **FINDING 2** |

Six of the six shapes the brief named render correctly. Two defects were found that the 15
passing unit tests did not, one of them because the test fixture for that very case uses a
value the backend cannot send.

## 7. Teardown

- `frontend/vite.config.sp4d2a.mts` deleted; `git status` clean.
- The scratch backend (:8099) and Vite (:5199) were stopped.
- The stale `:8080` backend and `:5173` Vite from the 2026-09-27 SP4b-2 gate were **left
  running and untouched** — they belong to another session's setup, not this one.
- The kind cluster was read with `SELECT`s only. `demo-trino` and its 26 endpoints, alerts and
  datasets were not touched; nothing was written to the cluster database at any point.
- The dev database `egret`, `egret_test` and the `_sqlx_test_*` databases were not touched.
- `egret_sp4d2a` is **left in place**, as `egret_sp4b` was left after the SP4b-2 gate: it is
  the instrument this document's measurements were taken on, and dropping it would make them
  unrepeatable. `DROP DATABASE egret_sp4d2a;` removes every object this task created, in one
  statement; nothing outside that database was added.
- `df -h /` before 13Gi available, after 12Gi. The scratch database is 13MB (measured); the change
  is the already-warm 19G `target/` growing during `cargo build -p egret-bin`. Docker was
  never at risk.
