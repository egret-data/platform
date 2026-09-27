# SP4b-2 Task 7 — driving the permission UI in a browser

**Measured:** 2026-09-27, branch `feat/sp4b-2-permission-ui` (`f7e336ba`), local dev stack
(Postgres/MinIO/StarRocks from `deploy/docker-compose.dev.yml`, backend `egret serve` on :8080,
Vite on :5173), Playwright-driven Chromium.

**Why a fresh database.** The dev database `egret` refused `egret migrate` with
`migration 127 was previously applied but has been modified` — it had been migrated from a
different branch. A new database `egret_sp4b` was created and migrated from this branch
instead; the dev database was left untouched.

## The instrument, and what it is made of

There is no k8s here, so the objects a real engine provisioning would create were seeded
directly into `egret_sp4b`. Every one of them is a row the compiler reads, and the fixture is
shaped so that **a grant's blast radius and a tag rule's blast radius differ** — otherwise
case 6's fan-out claim cannot be falsified:

| object | rows |
|---|---|
| `policy.authz_service_defs` | `trino` (the repo's own `crates/egret-policy/tests/fixtures/trino-servicedef.json`) |
| `policy.authz_services` (profiles) | `trino-alpha`, `trino-beta` — both `type=trino`, both `origin=manual` at the start |
| `engine_mgmt.engines` | `alpha` → `trino-alpha`, `beta` → `trino-beta` |
| `registry.catalogs` | `wh`, `other_wh` |
| `registry.engine_catalog_attachments` | `alpha`↔`wh`, `beta`↔`other_wh` |
| `tags` | `pii` / "PII" |
| `policy.authz_policies` | `zz-hand-written-admin` inside `trino-alpha` — a **foreign** policy, for case 8 |

So `services_for_catalog_via('wh')` can only reach `trino-alpha`, while
`compiler_targeted_services_via()` reaches both. A preview that names both profiles for a
catalog-scoped grant would be wrong, and a preview that names one profile for a tag rule
would be wrong; the fixture can tell those apart.

Users: `admin@egret.local` (platform_admin) and `reader@egret.local`, whose only platform
role `policy_reader` carries exactly one permission, `policy:read`.

---

**Cases appear below in the order they were actually run, not in numerical order** (4 before 3;
5 last, because 7 needs the grant 5 deletes). Each section was written when that case finished, so
nothing here is a verdict written ahead of its measurement. The `Verdict` table at the end lists
them by number.

---

## Case 1 — sign in and reach all four pages · PASS

Signed in as `admin@egret.local` on the dev login form. The **Governance** app's sidebar, in
order, top to bottom, verbatim:

```text
Data permissions      → /governance/data-permissions
Mask & filter rules   → /governance/mask-filter-rules
Explain access        → /governance/explain-access
Policies              → /governance/policies
Classifications       → /governance/classifications      (pre-existing)
Data Lineage          → /governance/data-lineage         (pre-existing)
```

That is D4's order exactly: steward pages first, then the admin pages.

All four render. Their `h1` and subtitle, as rendered:

| page | heading | subtitle |
|---|---|---|
| `/governance/data-permissions` | Data permissions | Who can do what with data, read as a sentence — e.g. "analyst can read wh.sales.* except group contractors". |
| `/governance/mask-filter-rules` | Mask & filter rules | Named rules that mask columns or filter rows wherever a classification or a path matches. |
| `/governance/explain-access` | Explain access | Why a subject can or cannot reach a target — the reason class, not just the boolean. |
| `/governance/policies` | Policies | Ranger policy profiles — each engine enforces one profile. Open a profile to manage its access, masking, and row-filter policies. |

**No collision with the RBAC page.** `/control-plane/permissions` is a different app, a
different label and a different subject: heading **"Permissions"**, subtitle *"The full
catalog of platform permissions and how many roles each one is currently granted to. Change
what a role grants on the Roles page."*, summary strip `62 permissions · 18 areas · 40
granted · 22 ungranted`, rows like `alert:manage`, `catalog_registry:read`, `engine:provision`.
The governance item is "Data **permissions**" and lists data grants; nothing in either page
links to or resembles the other.

The Policies page already reads the fixture correctly: `2 profiles · 1 policies · 2 bound
engines`, rows `trino-alpha` (engine `alpha`, v1, 1 policy) and `trino-beta` (engine `beta`,
v1, 0 policies).

---

## Case 2 — create a grant through the dialog · PASS

`New grant` → Effect **Allow**, Actions **Read**, Scope catalog `wh` + schema `sales` (table and
column left empty), Include `role analyst`, Exclude `group contractors`.

Two things the dialog refused or warned about on the way, both hardcoded-knowledge decisions
from the spec, observed as rendered:

- The principal-kind picker offers exactly `user`, `group`, `role` — **no `workspace_members`**
  (D6).
- Choosing `group` immediately rendered, beside the field: *"Not enforced yet: no group provider
  or usersync, so this won't match inside the engine."* (D3).

Before a preview, `Create` was disabled and the panel read *"Run a preview before saving — a
rule or grant's blast radius is also a failure radius."*

**The preview, verbatim:**

```text
Preview: read on wh.sales.*
Would recompile trino-alpha.
created 1 · updated 0 · deleted 0 · unchanged 0
```

**Which profiles it named: `trino-alpha`, and only that one.** That is the right answer and a
falsifiable one — `trino-beta` exists, is `type=trino` and is bound to an engine, but its engine
reaches `other_wh`, not `wh`. A catalog-scoped grant must not name it. (Case 6 below is the same
preview naming *both*, for a rule that really does reach both.)

`POST /api/control-plane/perm/grants` → **201 Created**. The write was confirmed three ways
rather than by the dialog closing:

1. `GET /perm/grants` returns the row with `principals.include=[{role,analyst}]`,
   `principals.exclude=[{group,contractors}]`, `scope={catalog_id:4a8fe34a…, schema:"sales",
   table:null, column:null}`, `origin.type="explicit"`, id `e615e686-215b-4513-85bd-15cbc7310cc0`.
2. The list page's summary strip went to `1 grant · 1 allow · 0 deny · 0 from a workspace
   assignment`, and the row renders, **verbatim**:

   ```text
   role analyst can read wh.sales.* except group contractors
   ```

3. The compiler actually produced something: `policy.authz_policies` in profile `trino-alpha`
   now holds `perm-grant-e615e686-215b-4513-85bd-15cbc7310cc0` beside the pre-existing foreign
   `zz-hand-written-admin`; `perm.compiled` has one row linking that policy to that grant id;
   `trino-alpha.policy_version` went 1 → 2 and its **`origin` flipped `manual` → `generated`**.
   `trino-beta` was untouched (`policy_version` still 1, `origin` still `manual`) — the preview's
   claim and the write's effect agree.

## Case 4 — the preview is binding · PASS

Run in the same dialog, immediately after the successful preview above and before saving.

With the preview fresh, `Create` was **enabled**. Ticking **Write** (nothing else) changed the
panel to:

```text
Preview: read and write on wh.sales.*
The form changed since that preview — run it again before saving.
```

and `Create` went back to `disabled` — checked as the DOM property, not from the look of it.
The previously returned "Would recompile trino-alpha" block was **removed**, not left standing
beside a stale-warning, so there is no window in which an operator can read a profile list that
no longer describes the form.

**One thing worth recording because it distinguishes two implementations.** Un-ticking Write
again — returning the body to exactly what was previewed — re-enabled `Create` *without* a second
preview, and the panel showed the original result again. So the guard is a **hash of the body**,
not a "the form was touched" flag. That is the stronger of the two behaviours (a touch-flag would
make an operator re-preview an identical body), and it is what the brief says shipped.

---

## Case 3 — edit the grant; `PATCH` is a full-body replace · PASS

Clicking the row's `Edit role analyst can read wh.sales.* except group contractors` opened
**"Edit grant"**. Every field was pre-filled, read off the DOM rather than off a screenshot:

| field | value in the reopened dialog |
|---|---|
| Effect | `Allow` |
| Read / Write | `checked` / `unchecked` |
| Catalog | `wh` |
| `input#scope-schema` | `"sales"` |
| `input#scope-table` | `""` (enabled) |
| `input#scope-column` | `""` (disabled — a column needs a table first) |
| Include | badge `ROLE analyst` |
| Exclude | badge `GROUP contractors` |
| preview line | `Preview: read on wh.sales.*` |
| `Save changes` | `disabled` (no preview yet — the binding rule applies to an edit too) |

**Sentence before:** `role analyst can read wh.sales.* except group contractors`

Typed `orders` into Table — the single change — then previewed:

```text
Preview: read on wh.sales.orders
Would recompile trino-alpha.
created 0 · updated 1 · deleted 0 · unchanged 0
```

`updated 1` rather than `created 1` is the first evidence that this is an edit in place and not
a delete-and-recreate. Saved.

**Sentence after:** `role analyst can read wh.sales.orders except group contractors`

**Nothing was blanked.** `GET /perm/grants` afterwards:

- `id` is still `e615e686-215b-4513-85bd-15cbc7310cc0` — the same row, so D1's stable id holds
  and an `explain` deep link to it would survive the edit.
- `effect: "allow"`, `actions: ["read"]`, `principals.include: [{role, analyst}]`,
  `principals.exclude: [{group, contractors}]` — all carried through the full-body replace.
- `scope.table` went `null` → `"orders"`; `scope.column` is still `null`.

And the compiler followed rather than duplicating: profile `trino-alpha` is at
`policy_version = 3`, and there is still exactly **one** `perm-grant-e615e686-…` policy, now at
`version 2`, whose resources read
`catalog=[wh] schema=[sales] table=[orders] column=[*]` — the old `table=[*]` shape was replaced,
not left behind beside the new one.

---

## Case 6 — a mask rule on a tag, and the fan-out the preview exists for · PASS (with two findings)

`New rule` → Name `mask-pii-columns`, Kind **Mask a column** (default), Mask type **Redact**
(default), Selects by **Tag** (default), Tag `PII`. The tag field is a search that shows what it
will actually store: the suggestion rendered `PII` with `stored as pii`, so the steward sees the
key the model persists rather than only the display name.

Two D6 refusals, observed in the form rather than as a 400:

- The **Exempt** block offers only an `Include` bucket — there is no `Exclude` at all.
- Its kind picker offers only `user` and `group`; **`role` is absent** (Ranger's mask items have
  no roles bucket).

Before previewing, the panel warned: *"Run a preview before saving. A tag rule is
catalog-unscoped, so this can recompile every Trino profile in the installation — see which,
here, before committing."*

**The preview, verbatim:**

```text
Would recompile trino-alpha, trino-beta.
created 0 · updated 0 · deleted 0 · unchanged 1
rule 9c4c1e6f-88bb-4baa-a12d-0b7093e2821d: profile has no usable `tagService` (unconfigured, or naming a profile that does not exist), so tag rule "mask-pii-columns" cannot compile here
rule 9c4c1e6f-88bb-4baa-a12d-0b7093e2821d: profile has no usable `tagService` (unconfigured, or naming a profile that does not exist), so tag rule "mask-pii-columns" cannot compile here
```

**Which profiles it named: `trino-alpha` AND `trino-beta` — both.** This is the discriminating
measurement of the whole case. The grant in case 2, on catalog `wh`, named `trino-alpha` alone;
this tag rule, from the same preview endpoint and against the same two profiles, names both,
because a tag selector is catalog-unscoped and `trino-beta`'s engine never touches `wh`. D2's
fan-out is real and the preview reports it.

It also reports honestly that nothing compiled: my fixture profiles carry `config_jsonb = {}` and
therefore no `tagService`, so the rule is stored but expresses nothing in either profile. The
skip prose is surfaced as-is, which is what the spec says to do while `SkipReport` is free text.

Saved: `POST /perm/rules` → created. `GET /perm/rules` returns
`{id: 3698443b-6ef7-4318-9415-7d9f9a247ead, name: "mask-pii-columns", kind: {type: "mask",
mask_type: "MASK", value_expr: null}, selector: {type: "tag", key: "pii"}, exempt: {include: [],
exclude: []}}`. The list strip reads `1 rule · 1 masks · 0 row filters · 1 tag-selected` and the
row reads `mask-pii-columns | mask · MASK | tag PII | —`. `perm.compiled` gained no row and
both profiles' `policy_version` stayed where they were (alpha 3, beta 1) — exactly what
`unchanged 1` plus the two skips predicted, so the preview did not lie in either direction.

### Finding 1 (Minor, real, found only here) — duplicate React key on the skip list

The console carries, at error level:

```text
Encountered two children with the same key, `rule-9c4c1e6f-88bb-4baa-a12d-0b7093e2821d`.
Keys should be unique so that components maintain their identity across updates.
```

`RuleDialog`/`GrantDialog` key the skip list on `` `${s.source_kind}-${s.id}` ``, which is not
unique the moment the **same** rule is skipped in more than one profile — and for a tag rule that
is the normal case, not an edge one. It is the fan-out this very panel was built to show that
triggers it.

### Finding 2 (the same line, twice, unattributable) — `SkipReport` carries no profile

The two skip lines above are **byte-identical** and neither says which profile it is about. An
operator reading them cannot tell whether one profile failed twice or two profiles failed once.
This is not a frontend choice: `SkipReport` is `{source_kind, id, reason}` with no service or
profile field (`crates/egret-perm-compile/src/routes.rs:356`), so the page has nothing to print.
Fixing it properly is an API change; the frontend could at least de-duplicate.

---

## Case 7 — explain an allowed subject and a denied one · PASS

Three explains were run, because the brief's requirement that `no_grant_matched` and
`explicit_deny` "read differently" needs all three of them to be visible at once. Target in every
case: catalog `wh`, schema `sales`, table `orders`, action `read`.

### 7a — allowed

Subject: user `reader` (chosen from the search, which rendered `Read Only` / `stored as reader`),
roles `[analyst]`.

Summary line: **`Allowed`** — *"the reason class below is the answer; this is only a summary."*

| | as rendered |
|---|---|
| reason badge | **Allowed by grant** |
| hint | A grant matches this subject and target. |
| link | `View grant e615e686-215b-4513-85bd-15cbc7310cc0` |

Wire value from `POST /perm/explain`: `{"allowed":true,"reasons":[{"kind":"allowed_by_grant",
"grant_id":"e615e686-215b-4513-85bd-15cbc7310cc0"}],"tags":[]}`.
**Literal reason class: `allowed_by_grant`.**

### 7b — denied, nobody said yes

Same subject with the `analyst` role chip removed — so no grant matches at all.

Summary line: **`Not allowed`**

| | as rendered |
|---|---|
| reason badge | **No grant matched** |
| hint | Nobody granted this — not the same as a deny. Ranger's own audit shows "-" for exactly this case. |
| link | none (there is no grant to link to) |

Wire: `{"allowed":false,"reasons":[{"kind":"no_grant_matched"}],"tags":[]}`.
**Literal reason class: `no_grant_matched`.**

### 7c — denied, somebody said no

A second grant was created through the dialog with Effect **Deny**, `role analyst`, `read`,
`wh.sales.orders` — rendering in the list as `role analyst cannot read wh.sales.orders` (the
sentence's verb flips `can` → `cannot`, and the deny grant is `b27a993f-3c75-409c-9d4c-d2791602a0f3`).
Then the **same** subject as 7a was explained again.

Summary line: **`Not allowed`**

| | as rendered |
|---|---|
| reason badge | **Explicit deny** |
| hint | Somebody forbade this — a deny grant matches. A different administrative problem than no grant matching at all. |
| link | `View grant b27a993f-3c75-409c-9d4c-d2791602a0f3` |

Wire: `{"allowed":false,"reasons":[{"kind":"explicit_deny","grant_id":"b27a993f-…"}],"tags":[]}`.
**Literal reason class: `explicit_deny`.**

**They read differently in three independent ways, not one:** a different badge label
(`No grant matched` vs `Explicit deny`), a different explanatory sentence, and a different badge
variant (warning vs destructive). 7c also carries a grant link that 7b cannot have — which is the
practical difference for an administrator, since one of these problems has a row to go and fix and
the other does not. The deny also correctly *overrode* the allow grant that produced 7a from the
identical subject and target.

### The two limits are stated on the result panel

Both, verbatim, rendered on the result of every one of the three runs:

```text
This tool cannot see hand-written Ranger policy: it can answer "not allowed" for a subject the
engine actually allows through a seeded admin policy.

Tags populate only when the target names both a schema and a table — a catalog-only target always
returns none.
```

`Tags considered` read `None.` in all three runs, which is correct and consistent with the second
limit being about a *different* case — the fixture has no `catalog_tag_assignments` row for
`sales.orders`, so there was nothing to resolve even though schema and table were both named.

### Small observation, not a defect

`Explain` stays **disabled** until a `User` is chosen, so a subject expressed only as a role
(`roles=[analyst]`, no user) cannot be explained from the page even though the API accepts it —
7b's and 7c's wire calls were made with `user: "reader"` for that reason. Defensible (a real
engine request always has a user), recorded because it is a UI-only constraint.

---

## Case 8 — a generated profile, a refused compiler-owned write, and a still-editable foreign policy · PASS

This is the case with the discriminating half, so it is measured in three steps.

### The profile is marked

On the Policies list, `trino-alpha` carries a **`Generated`** badge and `trino-beta` carries none —
so D10's first-ever consumer of `ProfileDto.origin` is live, and the marking follows the data
(alpha's `origin` flipped to `generated` when the compiler first wrote into it in case 2; beta is
still `manual`).

On the profile page the badge's title text reads, verbatim:

```text
Compiled by the permission model from grants and rules. Its perm-grant-/perm-rule- policies are
protected below; everything else stays editable.
```

The `Access` tab lists all three policies. The two compiler-owned ones are visibly different from
the foreign one, on every affordance:

| policy | name cell | Enabled switch | Delete |
|---|---|---|---|
| `perm-grant-b27a993f-…` | plain text, title *"Compiled by the permission model — edit the underlying grant or rule instead."* | `disabled` | `disabled` |
| `perm-grant-e615e686-…` | plain text, same title | `disabled` | `disabled` |
| `zz-hand-written-admin` | **link** to `/governance/policies/trino-alpha/access/1` | enabled | enabled |

### The refused write, and the 409 text

The row's controls are disabled, but the editor route is reachable by URL, so the write was
actually attempted: `/governance/policies/trino-alpha/access/3` opened the editor for
`perm-grant-e615e686-…` (resource `wh.sales.orders.*`) and `Save` was pressed.

`PUT /api/control-plane/policies/3` → **409 Conflict**. The page rendered, as a destructive alert
above the form:

```text
This policy is owned by the permission model. policy "perm-grant-e615e686-215b-4513-85bd-15cbc7310cc0" is compiled by the SP4a permission model — it is produced by a grant or rule, not hand-authored; edit or delete that grant/rule instead of writing this policy directly, since the next recompile would otherwise silently overwrite this change
```

The server's own body, for comparison — the UI adds the leading sentence and passes the rest
through unchanged:

```json
{"error":"conflict: policy \"perm-grant-e615e686-215b-4513-85bd-15cbc7310cc0\" is compiled by the SP4a permission model — it is produced by a grant or rule, not hand-authored; edit or delete that grant/rule instead of writing this policy directly, since the next recompile would otherwise silently overwrite this change"}
```

`DELETE /api/control-plane/policies/3` returns the **same 409 and the same message**, so the guard
is on the policy for every mutation, not just on the editor's save path.

### The foreign policy in that same generated profile is still editable — proven by mutating it

Not inferred from an enabled-looking control. The same editor, one id over:

- `PUT /api/control-plane/policies/1` (the editor's own Save on `zz-hand-written-admin`) →
  **200 OK**, and the app navigated back to the profile page, which is its success path.
- `PUT /api/control-plane/policies/1/enabled` → **204 No Content** (disabled, then restored to
  enabled).
- `trino-alpha.policy_version` advanced across these writes (4 → 6), so the profile really was
  written to while `origin = 'generated'`.

So the guard is scoped to the **policy**, exactly as SP4a moved it, and the UI reflects that
distinction rather than freezing the profile: one editor, two ids, 409 for the compiler's and
200 for the operator's.

---

## Case 5 — delete the grant; the confirmation names the sentence · PASS

Clicking the allow grant's `Delete` opened an `alertdialog` whose text, verbatim:

```text
Delete "role analyst can read wh.sales.orders except group contractors"?

This grant is permanently removed and every profile it reaches recompiles.

Type delete to confirm:
```

**It names the sentence, not the uuid** — `e615e686-215b-4513-85bd-15cbc7310cc0` appears nowhere in
the dialog. It also requires typing `delete`: the confirm button stayed `disabled` until the word
was typed, so a mis-aimed click on the wrong row cannot destroy a grant.

After confirming: the list dropped to `1 grant · 0 allow · 1 deny · 0 from a workspace assignment`
with only `role analyst cannot read wh.sales.orders` left, and `GET /perm/grants` returns only
`b27a993f-… deny`.

The compiled side was reaped rather than orphaned — the failure SP4a had once fixed and D1 warned
could return through the edit/delete door. `policy.authz_policies` now holds exactly
`zz-hand-written-admin` and `perm-grant-b27a993f-…`; the deleted grant's
`perm-grant-e615e686-…` policy is **gone**, and `trino-alpha.policy_version` advanced to 8. No
policy is left being served for a grant that no longer exists.

---

## The gate check — a `policy:read`-only user

A whole run as `admin@egret.local` proves the pages render, not that the gates work: `platform_admin`
bypasses inside the PDP. So everything above was re-measured as `reader@egret.local`, whose
`/api/auth/me` returns exactly:

```json
{"email":"reader@egret.local","platform_roles":["policy_reader"],"permissions":["policy:read"]}
```

**A note on how this was set up, because it nearly produced a false result.** The role and grant
were seeded with raw SQL, and the first attempt had the reader getting `403 {"error":"forbidden"}`
on *every* route including the `policy:read` reads. The cause is not an authz bug: `CedarPdp`
reloads its template set from a `RolesChanged` event on the `EventBus`, and a direct `INSERT`
emits none, so the running PDP had never heard of `policy_reader`. Restarting `egret serve` fixed
it. Had I stopped at the first measurement I would have reported the read gate as broken.

### What the reader CAN do

| | result |
|---|---|
| `GET /perm/grants` | 200, the grant list |
| `GET /perm/rules` | 200, the rule list |
| `GET /policies/services` | 200, both profiles with `origin` and engine bindings |
| `GET /policies/1` | 200, the foreign policy's raw body |
| `POST /perm/explain` | **200** — `{"allowed":false,"reasons":[{"kind":"explicit_deny",…}]}` |
| reach all four pages | yes; the sidebar shows exactly `Data permissions`, `Mask & filter rules`, `Explain access`, `Policies` |

`Classifications` (`tag:read`) and `Data Lineage` are correctly absent from the sidebar, and the
top-level `Control Plane` app link is gone too — only `Explorer` and `Governance` remain.

`explain` being a `POST` that a reader may call is right: it mutates nothing, and D9's whole point
is that an administrator can ask why without being able to change the answer.

### What the reader CANNOT do

Every write refused with `403 {"error":"forbidden"}`, measured one by one:

| attempt | result |
|---|---|
| `POST /perm/preview` | **403** |
| `POST /perm/grants` | 403 |
| `DELETE /perm/grants/{id}` | 403 |
| `PUT /policies/1/enabled` | 403 |

And the two steward pages hide the write affordances rather than offering them and failing:
on `Data permissions` the only buttons left in `main` are the `Effect:` and `Action:` facet
selects — **no `New grant`, and no per-row `Edit`/`Delete`**. Same on `Mask & filter rules`: only
the `Kind:` facet, no `New rule`, no row actions. So `policy:update` really does gate the writes,
and the gate is visible in the UI and enforced at the API.

### Finding 3 (Important) — the sentence degrades to a raw UUID for exactly this user

On `Data permissions`, as the reader, the grant row renders:

```text
role analyst cannot read 4a8fe34a-af58-4984-a1e7-12854fb9d496.sales.orders
```

instead of `role analyst cannot read wh.sales.orders`. `formatScope` falls back to
`scope.catalog_id` when the name lookup is empty, and the catalog name comes from
`GET /api/control-plane/catalogs`, which returns **403 Forbidden** for a `policy:read`-only user
(it needs `catalog_registry:read`). D5's decision is that the sentence *is* the primary column;
for the user class the page's own `perm` gate admits, that column is a uuid.

`Mask & filter rules` has the milder version of the same problem: the rule's `Selects` column
reads `tag pii` (the stored key) instead of `tag PII` (the display name), because `useTags` needs
`tag:read`. Still readable, but it is the same missing-lookup fallback.

### Finding 4 (Important) — `Explain access` is unusable with `policy:read` alone

The page renders, `POST /perm/explain` is allowed, and yet the page cannot be operated: **the
Catalog dropdown contains zero options** (`[role=listbox]` present, `optionCount: 0`) because it is
fed by the same 403ing `/catalogs`. `Schema`/`Table`/`Column` stay disabled until a catalog is
chosen, and `Explain` stays `disabled: true`. There is no message saying why the list is empty.

So the admin-facing page of this feature is reachable but inert for the permission it gates on. It
worked for the admin only because `platform_admin` also passes `catalog_registry:read`. Findings 3
and 4 are one root cause with two severities: `policy:read` is not sufficient to read a catalog
*name*, and three of the four pages need one.

### Finding 5 (pre-existing SP4a page, not SP4b's four) — a write offered and then silently refused

On the profile detail page the reader still sees `Seed default policies`, `Download Trino config`,
`New policy`, and an **enabled** `Enabled` switch and `Delete` on the foreign policy. Clicking the
switch was measured: `PUT /api/control-plane/policies/1/enabled` → **403 Forbidden**, and the page
showed **nothing at all** — no toast, no alert; the switch just snapped back to `true`. Nothing was
destroyed (the API held), but the operator gets no explanation. This is the SP4a raw-policy UI,
which SP4b only touched to add the `Generated` marking and the 409 handling, so it is recorded as
scope-adjacent rather than as a regression — and it is the contrast that shows SP4b's own two
steward pages got this right.

---

## Verdict

| # | case | result |
|---|---|---|
| 1 | Sign in, reach all four pages, no collision with Access → Permissions | **PASS** |
| 2 | Create a grant; sentence and preview profiles recorded | **PASS** |
| 3 | Edit it; `PATCH` pre-fills everything and blanks nothing | **PASS** |
| 4 | The preview is binding; editing re-blocks Save | **PASS** |
| 5 | Delete; the confirmation names the sentence, not a uuid | **PASS** |
| 6 | Mask rule on a tag; preview names both Trino profiles | **PASS** (2 findings) |
| 7 | Explain allowed and denied; both limits stated | **PASS** |
| 8 | Generated profile marked, compiler policy 409s, foreign policy still editable | **PASS** |
| — | gate check as a `policy:read`-only user | **PASS** for the gates; 3 findings |

Nothing was BLOCKED. The stack ran locally with a seeded fixture in place of k8s-provisioned
engines, which is the one substitution in this run and is described in full at the top.

### The findings, in priority order

| # | severity | what |
|---|---|---|
| 3 | Important | the grant sentence falls back to the raw catalog **uuid** for a `policy:read`-only user, because `/catalogs` needs `catalog_registry:read`; the rule page's tag falls back to the key |
| 4 | Important | `Explain access` is **inert** with `policy:read` alone — the Catalog dropdown is empty, so `Explain` can never enable, with no message saying why |
| 5 | Important, pre-existing | the SP4a profile page offers writes to a reader and shows **nothing** when the API 403s them |
| 1 | Minor | React duplicate-key error on the preview's skip list — `${source_kind}-${id}` collides on exactly the tag-rule fan-out the panel exists for |
| 2 | Minor | two byte-identical skip lines with no profile named; `SkipReport` has no profile field, so this needs an API change |

Findings 3 and 4 share one root cause and are the two worth fixing before this ships to anyone who
is not a platform admin: the four pages gate on `policy:read`, but three of them need
`catalog_registry:read` to render a legible answer. Either the `policy_reader`-shaped role must
carry `catalog_registry:read`, or these pages need a name source that `policy:read` can reach.

### What was not run

- **No real engine.** There is no Trino or Ranger in this run, so nothing here says a compiled
  policy is *enforced* — only that it is produced, updated and reaped correctly in
  `policy.authz_policies` and `perm.compiled`. SP4a's Task 10 live result is where enforcement was
  measured; this document is about the UI.
- **No tag actually assigned to a column**, so `explain`'s `tags` array was empty in all three
  runs and the tag-resolution half of D9 is untested here. The tag *rule* path was exercised; the
  tag *assignment* path was not.
- **The row-filter rule kind and `mask_type: CUSTOM`** were not driven; only a `MASK`-type mask on
  a tag selector.
- **D6's third refusal** (a path-selected row filter that does not name exactly catalog, schema and
  table) was not reached, since no row-filter rule was created.
- **D8's two ceilings** were not provoked: the fixture is far too small to push a profile over
  either cap, so the refusal messages were not seen on screen.
