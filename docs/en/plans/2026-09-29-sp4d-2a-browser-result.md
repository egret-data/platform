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
df -h /                       # 13Gi free before, unchanged after

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

*(Written after the predictions above were committed. Each case: what rendered, verbatim,
with the network payload beside it.)*

**To be filled in by the observation pass.**
