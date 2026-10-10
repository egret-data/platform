# SP4d-3: classify hand-written Ranger policies, migrate the ones that can be, and put egret's own policies out of hand's reach

**Status:** design, approved 2026-10-10
**Programme:** `2026-09-15-control-plane-dg-review-programme.md`, item SP4
**Depends on (all merged):** SP4a permission model (#596) · SP4b permission UI (#600/#601) ·
SP4c clearance (#603) · SP4d-1 `egret-ranger-eval` (#605) · SP4d-2a explain the served
artifact (#606) · SP4d-2b evaluate tag policies (#611) · SP4d debt and guardrails (#656,
migrations 0176–0179)

## Why this exists

egret **is** the Ranger Admin. No Apache Ranger server runs anywhere; the plugin inside each
engine downloads policy straight from egret, so a row in `policy.authz_policies` is not a
record of a decision — it **is** the decision the engine will enforce. Trino 470 runs
`ranger-plugins-common-2.5.0`; StarRocks fe-ubuntu 4.1 runs `2.8.0`.

Two problems follow, and SP4d-3 closes both.

**egret's own policies are hand-editable.** The seeder mints thirteen all-access policies per
generated Trino profile (`egret-admin - catalog, schema, table`, `all - catalog, …`;
`crates/egret-policy/src/default_policies.rs`), and the delegator plumbing mints two more with
stable names (`egret-delegator-impersonate`, `egret-query-timeout`). The only name reservation
that exists, `refuse_reserved_name` (`crates/egret-policy/src/store.rs`), covers
`perm-grant-` / `perm-rule-` / `perm-clearance-` and **nothing else**. An operator with policy
rights can therefore edit or delete the policies that make delegation work, and nothing
refuses it.

**Nobody can say which hand-written policies could become model objects.** SP4a's premise was
that hand-written profiles stay for external engines; the survey that framed SP4d then found
most of what looked hand-written was egret's own infrastructure and that some of the rest
**cannot** be migrated at all. `attribution.rs` already names the four categories
(`crates/egret-perm-compile/src/attribution.rs`), but only to return `None` from `explain`.
There is no surface that answers "what is this policy, and could it be a Grant?".

### What could not be measured, and what follows from that

There are **zero** policies on this host: the dev database holds 0 services, 0 policies,
0 engines, 0 catalogs, and all eleven databases answer 0 for `policy.authz_policies`
(measured 2026-10-10). The kind cluster that held the original corpus is gone. So **the
population this migration targets cannot be measured today**, and every design choice below is
made against the four categories `attribution.rs` names rather than against a counted corpus.
The classifier is itself the instrument that will measure that population when a real cluster
exists again — which is one reason it ships even though the migrator is the headline.

## Decisions taken by the user, 2026-10-10

1. **Build both halves, with real writes.** Not protection-plus-reporting; the migrator
   writes.
2. **A migrated policy is DISABLED, not deleted** — left in the profile, switched off.
   Ranger unions allow-policies, so leaving the original enabled would grant in parallel with
   the new compiled policy and make a revocation through the new UI ineffective; deleting it
   would remove the only rollback. This also follows the standing rule adopted 2026-09-16:
   never delete something the cluster may still be holding.
3. **All-or-nothing per policy.** A policy migrates whole or not at all. This is close to
   forced: because the original is disabled, migrating only the expressible part of a policy
   would **drop that policy's deny items**, which is a fail-open change.
4. **API routes plus a Control Plane page.** The audience is a platform operator and the data
   is per profile. A UI is not a convenience here — in this programme the browser has
   repeatedly found defects that thousands of green tests did not.

## Measured vocabulary — what the perm model can say

Read off the code, not inferred:

| Model | Shape | Source |
|---|---|---|
| `Grant` | `effect ∈ {allow, deny}`, `actions ⊆ {read, write}`, `scope`, `principals{include, exclude}` | `crates/egret-perm/src/model.rs` |
| `Scope` | `catalog_id: Uuid` + `schema?` + `table?` + `column?`, no level skipped | same |
| `Rule` | `Selector::{Tag{key}, Path(Scope)}` × `RuleKind::{Mask{mask_type, value_expr?}, RowFilter{filter_expr}}` | same |
| `Clearance` | one per gated tag, compiled as a tag DENY with `denyExceptions` | `egret-perm-compile/src/clearance.rs` |

And the action expansion, which is the hinge of the whole grammar:

```
Action::Read  → Trino ["select"]            StarRocks ["select"]
Action::Write → Trino ["insert", "delete"]  StarRocks ["insert", "update", "delete"]
```
(`crates/egret-perm/src/actions.rs::engine_accesses`)

So the expressible access-type set is exactly `{select, insert, delete}` on Trino and
`{select, insert, update, delete}` on StarRocks. Every other access type in the service-def —
`create`, `drop`, `alter`, `show`, `use`, `execute`, `impersonate`, `grant`, `revoke`, the
function family — has no representation.

`principals.exclude` compiles to exactly **one** `allowExceptions` item when the effect is
allow, or one `denyExceptions` item when it is deny, carrying the **same** access set as the
main item (`egret-perm-compile/src/grants.rs`). `is_excludes` is hard-coded `false`.

## Part 1 — ownership, and a classifier

### `managed_by`, a column rather than a prefix

A new column on `policy.authz_policies`: `managed_by text NOT NULL DEFAULT 'operator'` with a
`CHECK (managed_by IN ('compiler', 'infra', 'operator'))`. The seeder stamps `'infra'`, the
compiler stamps `'compiler'`, the human entry point leaves the default.

The write guard refuses every human `UPDATE` and `DELETE` of a row whose `managed_by` is
`'infra'`. The reason this is a column and not a fourth name prefix: **renaming cannot defeat
a column.** A prefix guard is a predicate over a string the editor can change; `managed_by`
travels with the row. It is also the shape this programme has already adopted once — put a
cross-query safety rule in a `CHECK`, not in a predicate somebody must remember to call.

Policy names do **not** change. `attribution.rs` keeps reading names, because it must: the
evaluator only ever sees names, inside the artifact the engine downloaded, with no access to
our columns.

A note for whoever greps this later: `managed_by` already returns twelve hits in this
workspace, and **none of them are egret's**. They are all cached sqlx query JSON under
`crates/egret-catalog-engine/.sqlx-lakekeeper/` — the embedded lakekeeper's vendored schema has
a column of that name. egret's `migrations/` and `src/` have none, so there is no collision;
the new column lives on `policy.authz_policies`, a different schema entirely. (Measured
2026-10-10, and recorded because a bare "0 hits" is the same output a broken grep gives — the
twelve are the control that could have disagreed.)

That leaves two things that must agree, which is the defect shape that appeared five times on
the branch merged as #656. Two countermeasures, not one:

- The backfill for existing rows is computed **by `attribution.rs`'s own predicate**, so the
  column is seeded from the same source of truth that reads names.
- A test asserts the two agree **across the whole table**, not on a sample row.

### The classifier

`GET /api/control-plane/policies/services/{id}/classify` answers, per policy:

- `managed_by` and the category (`compiler` / `infra` / `operator`)
- `migratable: bool`
- `blockers: [...]` — for a refusal, every attribute that blocks it, each as a sentence an
  operator can act on, naming the offending value
- for a migratable policy, the model objects it would become

A refusal that does not name its blocker is useless, and it is also unmeasurable — see Part 4,
where refusals are asserted, not just accepts.

## Part 2 — a closed grammar, then an equivalence gate

Two gates with two different jobs. The grammar decides **whether a policy may be migrated** and
produces the reasons the operator reads. The evaluator decides **whether the write may land**.

### The grammar

A policy is migratable only when every one of these holds:

| Ranger shape | Ruling |
|---|---|
| access set equals an action's **full** expansion, or a union of full expansions | otherwise refuse — see the subset trap below |
| every access type is in the expressible set for the engine's type | otherwise refuse, naming the access type |
| every catalog value resolves to a row in `registry.catalogs` | `Scope.catalog_id` is a UUID; an unregistered catalog has nothing to point at. This is exactly the "hand-written profile for an external engine" case |
| `isExcludes` **and** `isRecursive` are false on every resource level | `grants.rs::level` hard-codes both to `false`, so a recursive or excluding level has no representation. `ambiguous_arbitration.rs` already reads both flags, which is where to borrow the parse from rather than writing a second one |
| at most one `allowExceptions` / `denyExceptions` item, with the same access set as the main item | `principals.exclude` emits exactly one, with the main item's accesses |
| no `validitySchedules`, no `conditions`, no non-default `policyPriority` | nothing in the model carries them |
| no empty level, and no `*` above a named level in a grant scope | migrations 0178 and 0179, in SQL and in Rust |
| a tag selector's tag exists in `tags.key` | the migrator never creates a tag |

**The subset trap.** `Action::Write` is atomic: it expands to `insert` + `delete` on Trino.
A hand-written policy granting **only** `insert` is therefore **not expressible** — migrating
it as `Write` would silently add `delete`. The grammar must refuse an access set that is a
strict subset of an expansion, not merely refuse unknown access types. A translation that is
faithful field-by-field gets this wrong, which is the argument for having a semantic gate at
all.

One Ranger policy may yield **N** model objects: several catalogs at one level become N grants;
a policy carrying both `policyItems` and `denyPolicyItems` becomes one allow grant and one deny
grant. All-or-nothing still holds — N objects are written only when all N are expressible.

### The equivalence gate

The gate evaluates the **whole profile before and after**, never the policy in isolation,
because Ranger decides over the policy *set*: a `denyPolicyItems` or a `policyPriority` in a
sibling policy changes the answer.

It decides **per `PluginVersion` of the engines actually bound to that profile**. The two
versions demonstrably disagree on at least one artifact — a tag DENY plus a higher-priority
resource ALLOW with no deny items is Allowed on 2.8.0 and Denied on 2.5.0 — so "equivalent on
2.5.0, different on 2.8.0" is a valid outcome and must block.

Probes come from the coordinates and principals both sides mention, **plus probes neither side
mentions**: a sibling at each level, a principal in no policy, and an excluded access type.
Without that second half the probe set inherits the blind spots of the two policies it was
derived from — a correlated oracle is not an oracle, and that failure has already occurred in
this programme.

Entry points are the three pure functions SP4d-1 exposes: `evaluate`, `evaluate_data_mask`,
`evaluate_row_filter` (`crates/egret-ranger-eval/src/eval.rs`).

## Part 3 — the write path

### Routes

```
GET  /api/control-plane/policies/services/{id}/classify
POST /api/control-plane/policies/services/{id}/migrate   { policy_names[], dry_run }
POST /api/control-plane/policies/services/{id}/migrate/undo { policy_names[] }
```

`migrate` returns the probe table (before → after, per plugin version) and the artifact diff.
It writes only when `dry_run` is false. Dry-run runs the **same code** and differs in exactly
one place: it always rolls back.

### One transaction, in this order

1. Insert the N `perm.grants` / `perm.rules` rows.
2. Run the existing recompile driver (`apply_diff_in_tx`), which stamps the profile
   `generated` and emits the `perm-*` policies.
3. Switch the original policy off — `policy.authz_policies.enabled = false`, the **column**,
   not a field inside `policy_jsonb`.

   This was measured, and an earlier draft of this spec had the mechanism wrong. The download
   query is `WHERE service_id = $1 AND enabled = true` and then stamps `isEnabled` into the
   emitted body *from that column* (`crates/egret-policy/src/store.rs`). So clearing the column
   is both the right lever and a stronger one than editing the JSON: the artifact the engine
   downloads **does not contain the policy at all**, rather than containing a copy marked
   disabled. `set_policy_enabled(id, enabled)` already exists for this and already runs
   `refuse_if_compiler_owned`, so it refuses a compiler-owned row — and must refuse an `infra`
   row too once Part 1's column exists.
4. Insert provenance.
5. **Re-run the equivalence gate against the state just written, inside the transaction, and
   `ROLLBACK` on any disagreement.**

Step 5 is the point of the ordering. The gate proves its claim about **the artifact the engine
will actually download after the write**, not about a predicted one. That is the lesson SP4d-2a
paid two defects for: an `explain` is only trustworthy when it evaluates what the engine really
loads.

### Provenance

`perm.migrated_from`: the policy name, `service_id`, the ids of the objects produced, the
timestamp, the actor, and **the number of probes that proved it**. It serves two purposes, not
one — the "migrated from …" badge in the UI, and a mechanical rollback.

### Rollback

`migrate/undo` deletes the objects provenance names, recompiles, and flips `isEnabled` back to
true. This route exists **because** decision 2 chose disable over delete; under a delete it
could not be written.

### Idempotency and refusal order

A policy that already has a provenance row answers 409, never a duplicate. The 409-before-400
ordering reuses `refuse_before_policy_insert`, which spells that order once; no second copy.

### A consequence to state plainly

After migration the profile is `generated`. The EDIT and DELETE guards are **policy-scoped**
(moved there by SP4a Task 8 fix round 2), so the operator can still maintain the hand-written
policies that were left behind. `create_policy` is **profile-scoped**, so adding a *new*
hand-written policy to that profile is refused from then on. That is SP4a's D4 working as
designed, and this spec keeps it; the preview must **say so** before the operator commits,
rather than letting them discover it later.

### The page

One table per profile: an owner chip (`infra` / `compiler` / `operator`), a verdict column,
blockers rendered as sentences, checkbox selection, and a preview panel that must be opened
before the Migrate button enables, showing the probe table per plugin version. `infra` rows
render **locked with their reason**, not hidden — a protection nobody can see is one somebody
will route around.

## Part 4 — how this is proven

### The oracle is the real Java engine

The Rust evaluator is differentially validated, but the claim "this migration is equivalent"
deserves the real engine, because the finding this programme keeps reproducing is that only a
real engine finds the defects: for SP4d-2b, 127 unit tests found 0 of 3 while the differential
corpus found 9. The migration corpus therefore runs through the existing harness,
`EgretCaseRunner` with `-Degret.cases` / `-Degret.verdicts` (`tests/ranger/`), **at both 2.5.0
and 2.8.0**, like the gate already in `just test-ranger-gates`. No second harness.

**How before/after fits a harness that takes one policy set per case.** Measured: `Case` carries
a single `service_policies` field, `Case::to_json` emits exactly one `servicePolicies` key, and
`EgretCaseRunner.runOne` deserialises one `ServicePolicies` and calls `evaluatePolicies` once —
the diff axis the harness supports is the **Ranger version**, not the policy-set version. So the
before-state and the after-state are emitted as **two separate cases** (paired ids, e.g.
`mig-0001-before` / `mig-0001-after`) and compared **pairwise on the Rust side** after
`ask_ranger` returns. The Java needs no change, and this is a constraint on the corpus generator
rather than on the harness.

### The corpus is generated from the service-def, not from our vocabulary

A corpus invented by the author only measures shapes the author imagined. This one enumerates
the service-def exhaustively — every access type, every resource hierarchy — which is what
guarantees the `create` / `drop` / `alter` / `execute` cases are actually *reached* and actually
refused, rather than never attempted.

### Both directions, and refusals are measured too

- A refused policy must assert **which blocker** was named. Without that, a classifier that
  refuses everything is also green.
- An expressible policy must not be refused. Without that, a migrator that never migrates is
  also green.

### Three pinned traps, both cells each

| Pin | The two cells |
|---|---|
| `Write` is atomic | `{insert, delete}` accepted · `{insert}` refused (accepting it would add `delete`) |
| catalog outside the registry | registered → accepted · unregistered → refused, naming the catalog |
| `managed_by` ⇄ `attribution.rs` | the two agree across the **whole table**, not on a sample row |

### Mutation gate

Loosen the grammar to accept a strict subset of an action's expansion, and a **named** test must
die. The mutant must compile before a death counts — a mutation that fails to build looks
exactly like one no test killed.

### A live check, as a task and not an afterthought

Stand up a manual profile with hand-written policies on a real cluster, migrate it, and confirm
the engine's enforcement changes **exactly as the preview said** — by real query, and by looking
at the page in a browser. The kind cluster that held the old data is gone, so this includes
rebuilding the fixture. In this programme a browser found four defects on a page carrying 26–33
green tests; this step is not optional.

## Out of scope, named so it is not mistaken for closed

- **A hand-written policy naming a tag absent from `tags.key`** is refused. The migrator never
  creates tags.
- **A StarRocks engine bound to a `trino`-typed profile carrying a `tagService`** still produces
  a wrong artifact and is still only *reported*, through `UndecidableCause::EngineMismatch`. It
  is a cross-table invariant (`engines.authz_service_id` against `authz_services.type`) and needs
  its own measurement, exactly as the #656 plan already recorded.
- **`infra` policy content is not rewritten.** SP4d-3 protects those policies; it does not
  redesign them.
- **The ambiguous-priority emission** — the compiler can still emit two row filters for one table
  at the same priority with different expressions. `explain` refuses to arbitrate; the emission
  is untouched here.
- **The console still cannot create a tag-capable profile** (`createProfile` sends only name and
  type, and no `UPDATE` of `config_jsonb` exists in any `src/`). Adding a config route means
  re-reading SP4d-2b's Task 2 rulings, which were written on the premise that no such statement
  exists.

## Risks

- **The population is unmeasured.** The migrator is being built for a corpus nobody has counted.
  The mitigations are the exhaustive service-def corpus, the refusal-side assertions, and a
  grammar that refuses when unsure rather than guessing.
- **The equivalence gate can only compare what it probes.** The unmentioned-probe requirement
  reduces this; it does not remove it. A migration proven equivalent on every probe may still
  differ on a coordinate no probe reached, which is why the original policy is disabled rather
  than deleted and why `undo` exists.
