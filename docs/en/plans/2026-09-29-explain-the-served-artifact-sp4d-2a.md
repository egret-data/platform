# SP4d-2a — `explain` answers for the artifact the engine loads

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** `explain` stops evaluating egret's permission model and starts
evaluating the policy set each engine actually downloads, answering once per
profile, with `Undecidable` as a first-class outcome.

**Architecture:** the explain handler resolves the target catalog to its serving
services, loads each one's served policy set through the same assembler the
download route uses, and runs `egret-ranger-eval` over it. Policy names are
mapped back to model objects where one exists. The model evaluator survives only
as a test oracle.

**Tech stack:** Rust (axum, utoipa, sqlx), `egret-ranger-eval`, React + orval.

**Spec:** `docs/en/specs/2026-09-29-explain-the-served-artifact-sp4d-2a-design.md`

## Global Constraints

- **`egret-ranger-eval` stays dependency-pure:** `serde` + `serde_json` only. No
  `utoipa`, no sqlx, no tokio, no I/O in library code. Wire DTOs are hand-written
  in the route crate (D7).
- **Do NOT narrow `Unmodelled::ServiceConfig`.** An empty `tagPolicies` block is a
  measured no-op; an unmodelled `serviceConfig` fails **OPEN** through a super-user
  allow at `policyPriority = Integer.MAX_VALUE` that nothing downstream can
  override. The ⚠ block on that variant in `model.rs` says so — leave it and its
  refusal intact. Verified again 2026-09-29: `RangerPolicyEngineImpl.<init>` builds
  `ServiceConfig` from `ServicePolicies.getServiceConfig()`.
- **Tag relevance is gated on the service-def's `contextEnrichers`, never on
  `EngineType`** (D4). Gating on the engine becomes silently wrong the day one def
  field changes.
- **`roles` and `groups` reach the evaluator EMPTY** (D2), because that is what the
  engine sees. egret serves no `/service/roles/download` and runs no usersync.
- **`explain` stays read-only** and gated on `policy:read`, not `policy:update`. It
  must not compile, must not bump `policy_version`, and must open no transaction.
  `explain_writes_nothing` pins this and must keep passing.
- **Read the stored `users.username`; never re-derive it** from an email.
  `ensure_username` lowercases, slugifies and appends `-2`/`-3` on collision.
- Never run tests against the dev DB `egret`; use `egret_test`.
  `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`,
  `env -u EGRET_ALLOW_DB_SKIP`, `RUST_TEST_THREADS=4`.
- Never use `git stash`. Never override the git author.
- Frontend: **no prettier** (`eslint .` only); OKLCH semantic utilities with
  `var(--token)`, never `hsl(var(--token))`; consume orval hooks directly;
  regenerate with **`npm run gen:api`**, never bare `orval` (the second step adds
  `encodeURIComponent`).
- The OpenAPI drift test must be run as
  `cargo test -p egret-server --lib committed_openapi_json_is_current` — **without
  `--lib` it reports "0 passed, 1 filtered out" and exits 0**.
- Environment traps, all hit on the predecessor branch: `cp` is aliased to `cp -i`
  and `rm` to `rm -i` — restore with `cat backup > file`, delete with
  `command rm -f`. After a text substitution, verify the file actually changed
  before believing a SURVIVED result. This is zsh: `${PIPESTATUS[0]}` silently
  yields nothing; capture exit codes from the command itself.
- Commands over ~120s are auto-backgrounded; a completion notification is a
  finished result to read, not a signal to wait. Commit before running one.

---

### Task 1: Narrow the `tagPolicies` refusal to the tier that can actually decide

**Files:**
- Modify: `crates/egret-ranger-eval/src/model.rs`
- Modify: `crates/egret-ranger-eval/src/servicedef.rs`
- Modify: `crates/egret-ranger-eval/src/eval.rs`
- Create: `crates/egret-ranger-eval/tests/fixtures/tag-policies-live.json`
- Test: `crates/egret-ranger-eval/tests/tag_tiers.rs`

**Interfaces:**
- Produces: a payload on `Unmodelled::TagPolicies` distinguishing the tiers;
  `ServiceDef::has_tag_enricher()`; an `AccessRequest` input for the resolved tag
  set that distinguishes "not resolved" from "none".

- [ ] **Step 1: Capture a real fixture, do not invent one.** The repo has no real
tag-policy fixture — `tests/ranger/fixtures/service-policies.json` contains zero
occurrences of `tagPolicies`, and the only existing fixture is a synthetic empty
block at `tests/model.rs:355`. A real one is available from the live plugin's own
cache:

```bash
command kubectl --context kind-egret exec -n egret-engines <trino-coordinator-pod> -- \
  cat /tmp/ranger/sp3c2-trino-live/policycache/trino_sp3c2-trino-live.json
```

**Note the filename.** An earlier draft of this plan pointed at the sibling
`…_tag.json`, and its byte count matched — but that file is the tag **STORE**
(`tags`, `serviceResources`, `resourceToTagIds`) and contains **no policies and no
`tagPolicies` key** at all. The tag policies live inside the main policy cache
under `tagPolicies`. A matching byte count made the wrong file look confirmed;
capturing it would have produced a policy-free fixture and every tier-(c) test
would have passed vacuously.

Measured 2026-09-29 in the main cache (37049 B): tag service 51,
`policyVersion 22`, three policies — `8799` (policyType 1, mask,
`resources.tag.values = ["pii"]`), `8805` (policyType 2, row filter), `8839`
(policyType 0, ACCESS, deny-capable, `denyPolicyItems` for `public` with
`denyExceptions`). Read-only; do not touch `demo-trino`.

This matters beyond convenience: a fixture drawn from the same imagination as the
code inherits its blind spots. The predecessor package lost a fail-open defect to
exactly that.

- [ ] **Step 2: Write the failing tests.** Three tiers plus the empty-tag-set case:

1. Block **absent** → decidable (already true; a control).
2. Block present, `policies: []` → **decidable**. Ranger's `evaluateTagPolicies`
   exits early at `CollectionUtils.isNotEmpty(evaluators)`. Verify in the jars, and
   note the precise form: the early-exit target is **not** a bare return — it reads
   `getIsAllowed()` and can call `setIsAccessDetermined(true)`. The conclusion still
   holds, because the result arrives from `createAccessResult` with `isAllowed`
   false and the only block that could flip it first (the super-user short-circuit)
   already sets that same flag — so the write is unreached or idempotent. Record
   that reasoning where the code is, so a later reader who finds the write does not
   conclude the tier analysis was wrong.
3. Block present, non-empty (the captured fixture) → `Undecidable`, with a cause
   naming how many tag policies could decide. Not a generic envelope refusal.
4. Block present, non-empty, but the request's resolved tag set is **empty** →
   **decidable**. `getLikelyMatchPolicyEvaluators` returns `EMPTY_LIST` at
   `isNotEmpty(tags)`.

   ⚠ **THIS NARROWING IS FAIL-OPEN UNLESS THE CALLER'S TAG RESOLUTION IS AT LEAST
   AS BROAD AS THE ENRICHER'S, AND EGRET'S IS NOT.**
   `tag_keys_for_resource_via` (`crates/egret-policy/src/store.rs:782-806`) matches
   `schema_name`/`table_name`/`column_name` by **exact string equality** with no
   hierarchy roll-up: asked about `report.country` it does **not** return a tag
   attached to the table `report`. Ranger's `RangerTagEnricher` does the opposite —
   it walks the hierarchy and emits ancestor tags carrying `MatchType::ANCESTOR`,
   which Ranger **admits** for a tag request. So egret can report "no tags" while
   the engine is enforcing a tag policy through an ancestor attachment.

   The crate side of this task is therefore only allowed to *offer* the narrowing;
   whether it is **sound** depends entirely on what the caller passes. Express that
   in the type: the input is an `Option`, `None` means "not resolved" and must NOT
   take this path, and the doc comment on it must state this trap. Task 5 decides
   whether egret can pass a sound value.

   Every live tag assignment today is column-level (6 rows, `column_name` never
   empty), so the ancestor path is **not exercised by current data** — which is
   exactly how this would ship green and wrong.
5. A def whose `contextEnrichers` contains **no** `RangerTagEnricher` → tag
   policies cannot decide, so a non-empty block does not force `Undecidable`.
   egret's `starrocks.json` has `contextEnrichers: []`; its `trino.json` declares
   the enricher.
6. **Control, and this is the one that keeps the package honest:**
   `Unmodelled::ServiceConfig` still forces `Undecidable`. Assert it explicitly so
   a future narrowing cannot take it by accident.

- [ ] **Step 3: Run them, watch them fail.**

- [ ] **Step 4: Implement.** `Unmodelled::TagPolicies` gains a payload;
`servicedef.rs` parses `contextEnrichers`; `AccessRequest` gains the resolved tag
set as an `Option` so "the caller did not resolve tags" is distinguishable from
"there are none" — the same discipline `EngineConfig`'s `Option` fields use. An
unresolved tag set treated as empty would fail **open**.

- [ ] **Step 5: Mutation-check the narrowing.** Make tier (c) decidable and
confirm a test dies. Make `has_tag_enricher()` always true, then always false, and
confirm each kills something. Restore with `cat backup > file` and verify.

- [ ] **Step 6: Commit.**

---

### Task 2: A fixture with one catalog served by two profiles of different types

**Files:**
- Modify: `crates/egret-perm-compile/tests/routes.rs` (the shared helpers)
- Test: exercised by Tasks 5 and 6

**Interfaces:**
- Produces: a test helper building one catalog attached to **two** engines whose
  profiles are one `trino` and one `starrocks`.

This shape exists live — warehouse `sp4a10_wh` is served by profile 52 (Trino) and
55 (StarRocks), and both perm grants on it compiled to `{"services":[52,55]}` — and
**exists nowhere in the test suite.** Every existing fixture is one workspace → one
warehouse → one engine → one catalog → one profile; `extra_profile`
(`tests/routes.rs:825-891`) deliberately builds a second profile reaching a
DIFFERENT catalog. Without this helper nothing tests the per-profile response.

- [ ] **Step 1: Read the existing fixtures first.** `setup()` at
`crates/egret-perm-compile/tests/routes.rs:204-318` and `extra_profile` at
`:825-891`. Follow their shape; do not invent a parallel one.

- [ ] **Step 2: Write a test that asserts the fixture is what it claims.**
`services_for_catalog_via(conn, catalog_id)` must return **two** ids, and the two
services must have types `trino` and `starrocks`. A fixture that silently builds
one profile would make every per-profile test vacuously pass.

- [ ] **Step 3: Run it, watch it fail. Step 4: Build the helper. Step 5: Confirm green.**

- [ ] **Step 6: Commit.**

---

### Task 3: Map a compiled policy name back to its model object

**Files:**
- Create: `crates/egret-perm-compile/src/attribution.rs`
- Test: `crates/egret-perm-compile/tests/attribution.rs`

**Interfaces:**
- Consumes: the three compiler prefixes from `crates/egret-policy/src/store.rs`.
- Produces: a function taking a Ranger policy name and returning an optional model
  attribution — Grant(uuid) / Rule(uuid, optional locator) / Clearance(tag uuid,
  optional locator) / None.

- [ ] **Step 1: Write the failing tests**, one per shape the compiler actually
mints. Read the minting sites rather than trusting this list:
`perm-grant-{uuid}` (`grants.rs:451`), `perm-rule-{uuid}` (`rules.rs:339`, `:445`),
`perm-rule-{uuid}-{catalog}.{schema}.{table}.{column}` (`rules.rs:317-326`),
`perm-clearance-{tag_id}` (`clearance.rs:307`),
`perm-clearance-{tag_id}-{catalog}.{schema}.{table}.{column}` (`clearance.rs:354`).

Also assert the **negative** cases return `None` without error, because they are
live: `"egret-admin - catalog, schema, table, column"` and
`"all - catalog, schema, table, column"` (`default_policies.rs`),
`"egret-delegator-impersonate"` and `"egret-query-timeout"`
(`delegator_policy.rs`), an operator-written name, and `"superuser"` — Ranger's
short-circuit reports `policy_id: -1, policy_name: "superuser"`, which is not a
policy at all.

- [ ] **Step 2: Run, watch fail. Step 3: Implement.** A `Uuid`'s `Display` is
exactly 36 characters, so after the prefix the id is a fixed-width slice and an
optional `-{locator}` cannot be confused with it however many hyphens a catalog
name contains. Verify that rather than assuming it.

- [ ] **Step 4: Confirm green. Step 5: Mutation-check** that a wrong prefix or a
truncated uuid does not silently produce a plausible attribution.

- [ ] **Step 6: Commit.**

---

### Task 4: Wire DTOs for the evaluator's result types

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs`
- Test: `crates/egret-perm-compile/tests/wire_dtos.rs`

**Interfaces:**
- Produces: utoipa `ToSchema` types mirroring `Decision`, `DenyReason`,
  `UndecidableCause`, and the mask and row-filter results, plus `From` conversions.

- [ ] **Step 1: Enumerate the source variants from the code, not from this plan.**
`crates/egret-ranger-eval/src/eval.rs` — `UndecidableCause` has 13 variants and
`DenyReason` 3, several carrying payloads (`policy_id`, `policy_name`, `expr`,
`item_index`, and a `Vec` of tied policy ids for `EvalOrderAmbiguous`). `why` is a
`Vec`, so one profile can carry several causes at once.

- [ ] **Step 2: Write the failing test.** A conversion test that is **exhaustive by
construction**: match on the source enum with no wildcard arm, so adding a variant
upstream breaks the build rather than silently serialising as something else. That
property is the whole reason to hand-write these.

- [ ] **Step 3: Run, watch fail. Step 4: Implement. Step 5: Confirm green.**

- [ ] **Step 6: Commit.**

---

### Task 5: Route `explain` through the evaluator, once per profile

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs`
- Modify: `crates/egret-perm-compile/Cargo.toml` (add `egret-ranger-eval`)
- Modify: `crates/egret-perm/src/lib.rs` (drop `evaluate` from the public re-export)
- Test: `crates/egret-perm-compile/tests/routes.rs`

**Interfaces:**
- Consumes: Tasks 1-4.
- Produces: the new `ExplainResponse` — one verdict per profile, plus the
  no-profile case.

- [ ] **Step 1: Write the failing tests** against Task 2's two-profile fixture:

1. Two profiles ⇒ two verdicts, each naming its profile and engine type.
2. A catalog no engine serves ⇒ the **no-profile** outcome, distinct from `Denied`.
3. `action: write` ⇒ per-access-type results: two on Trino, three on StarRocks.
4. A seeded `egret-admin` policy is **visible** — this is the blindness the package
   exists to remove, so assert it directly.
5. A raw operator-written policy appears in the reasons, named, with no model
   attribution and **not dropped**.
6. `roles` and `groups` reach the evaluator empty even when the subject holds a
   role in the database.

- [ ] **Step 2: Run, watch fail.**

- [ ] **Step 3: Implement.** Resolve service ids via `services_for_catalog_via`,
each service's row via `get_service_by_id_via`, its policy set via
`enabled_policies_for_service` (**the same assembler the download route uses** —
not `list_policies`, so what explain evaluated and what the engine received cannot
drift), its def via `get_service_def`, and the catalog **name** via
`catalogs_for_service_via` (the compiler's own reachability test: a catalog absent
from that map is out of scope for that profile). Resource level names differ per
engine — `resource_level_names()` in `grants.rs` is already `pub(crate)`.

`EngineConfig::without_super_users()` is the only usable value today, and it
encodes a measurement: neither live Trino sets `ranger.plugin.super.users`.

- [ ] **Step 3b: Decide, in code, whether egret can pass a sound tag set — and
default to refusing.** Task 1's empty-tag-set narrowing is only sound if egret's
resolution covers ancestor attachments, and `tag_keys_for_resource_via` does not
(see its ⚠ block). Two acceptable outcomes, and you must pick one deliberately and
say which in your report:

- **Make the resolution hierarchy-complete** — resolve at catalog, schema, table
  and column level and pass the union. Then the narrowing is sound, and a test must
  pin it with a **table-level** tag attached while a **column** is requested. No
  live data has that shape, so the fixture has to create it.
- **Do not use the narrowing** — pass `None` and let tier (c) refuse. Smaller and
  provably safe; the cost is that a profile with any tag policy stays `Undecidable`
  whether or not the target carries tags.

What is NOT acceptable is passing the existing resolution's result as though it
were complete. That reports "no tags" for a resource the engine is masking through
a table-level attachment, which is a confident wrong answer in the fail-open
direction — the single failure this whole programme exists to prevent.

- [ ] **Step 4: Confirm green, and confirm `explain_writes_nothing` still passes.**
The new path is pure reads; if that test fails, something opened a transaction or
compiled.

- [ ] **Step 5: `evaluate` becomes internal.** Drop it from `egret-perm`'s public
re-export, keep it reachable for tests. `PUBLIC_GROUP` in the same module is
genuine compiler code and must stay exported. Rename the three
`explain_and_the_compiled_policy_agree_*` tests and their doc comments — they no
longer describe what `explain` answers with — and **add one new agreement test**
asserting the artifact-based answer and the model oracle agree on the happy path.
This change removes a drift alarm; that test puts it back.

- [ ] **Step 6: Commit.**

---

### Task 6: Rework the explain tests the change invalidates

**Files:**
- Modify: `crates/egret-perm-compile/tests/routes.rs`

Each of these exists because of a rule. The rule survives even where the mechanism
does not — identify the rule first, then rewrite the assertion.

- [ ] **Step 1: `explain_names_the_reason_class_not_just_a_verdict`** — the
principle is the package's whole point. Rewrite the assertion; keep the test.

- [ ] **Step 2: `explain_distinguishes_an_explicit_deny_from_nothing_matching`** —
Ranger keeps the distinction (`DenyPolicyItem` vs `NoPolicyMatched`). This test also
asserted the reason names the grant, so it becomes **the acceptance test for Task
3's mapping**.

- [ ] **Step 3: `explain_gives_a_group_only_allow_its_own_reason_class_on_the_wire`
— the meaning inverts, and that is the point.** Today the model allows through a
`group` principal and the page warns the engine will refuse. Measured against the
artifact the answer is a refusal, so the reason class disappears **because the
defect it existed to flag disappears**. Rewrite it to assert exactly that
inversion. Measure the outcome rather than assuming it — the compiler does emit
`groups` into the policy body.

- [ ] **Step 4: `explain_agrees_with_the_compiled_policy_for_a_collision_suffixed_member`**
— keep the two-user fixture and the ground-truth read of `policyItems[0].users`.
The rule that must survive: a collision-suffixed identity is distinct from the bare
one and explain must not conflate them. It becomes the end-to-end test of Task 3's
mapping over a real compiled body.

- [ ] **Step 5: `explain_ignores_a_caller_supplied_role_and_resolves_the_subjects_own`
— budget this as a rewrite, not an edit.** The surviving rule: a caller may not
assert a role the subject does not hold. Under D2 that rule is kept by sending no
roles at all, which is a stronger guarantee than filtering them. The clearance half
has no Ranger analogue on the wire (`tag` + `required_roles` must be reconstructed
from the policy name → `tags` row), and on Trino the clearance lives in the tag
service, which is tier (c) and therefore `Undecidable`. The fixture is StarRocks,
where a clearance is a per-column resource deny the evaluator handles. State in the
test which engine's path it measures.

- [ ] **Step 6: `explain_resolves_the_targets_tags_itself`** — decide deliberately
whether `tags` stays on the response, and what it now means. It cannot mean "input
to rule selectors" any more. Do not let it rot into a field nobody can explain.

- [ ] **Step 7: Confirm `explain_writes_nothing` and
`policy_read_permits_the_reads_and_refuses_the_writes` still pass unchanged.** The
second one matters: the new path adds `PolicyStore` reads on a `policy:read` grant,
and no additional permission may leak in.

- [ ] **Step 8: Commit.**

---

### Task 7: Regenerate the API surface

**Files:**
- Modify: `openapi.json`
- Modify: `frontend/src/api/generated/**`

- [ ] **Step 1: Regenerate the backend document.**

```bash
cargo run -q -p egret-bin -- openapi > openapi.json
```

Never hand-edit it.

- [ ] **Step 2: Verify the drift gate, with `--lib`.**

```bash
cargo test -p egret-server --lib committed_openapi_json_is_current
```

Without `--lib` this reports "0 passed, 1 filtered out" and exits 0 — a false green.

- [ ] **Step 3: Regenerate the client.**

```bash
cd frontend && npm run gen:api
```

Never bare `orval`: the second step of that script restores `encodeURIComponent`,
and a bare run drops it.

- [ ] **Step 4: Commit exactly what the generators produced.**

---

### Task 8: The page answers per profile, and stops claiming a limit it no longer has

**Files:**
- Modify: `frontend/src/routes/governance/ExplainAccess.tsx`
- Modify: `frontend/src/routes/governance/__tests__/ExplainAccess.test.tsx`

- [ ] **Step 1: Write the failing tests.**

1. Two profiles render as two results, each naming its profile and engine.
2. A catalog no engine serves renders its own state, visually distinct from a
   denial.
3. An `Undecidable` verdict renders its **named cause**, not a bare "unknown"; a
   profile carrying several causes shows all of them.
4. A raw policy with no model attribution renders its Ranger name and id, with no
   link, marked as outside the permission model.
5. A seeded `egret-admin` policy renders as the reassuring explanation it is, not
   as an alarm.
6. **`clearance_required` renders a real label, and renders `tag` and
   `required_roles`** — the pre-existing defect of D10. Verified 2026-09-29:
   `REASON_META` has six kinds and this is not one of them, so today it shows the
   raw string with "Unrecognized reason class." and drops both fields.
7. The honest-limits block no longer claims explain cannot see hand-written policy.

- [ ] **Step 2: Run, watch fail. Step 3: Implement.**

The summary badge is **removed**, not recoloured — there is no honest aggregate
over `{Allowed, Denied, Undecidable}` × N profiles. `ScopePicker` and
`SubjectUserField` are untouched. The `?grant=` / `?rule=` deep links move onto
the per-profile reason rows. New badge colours follow `badge.tsx`'s existing
pattern; no raw colour, no `hsl(var(--…))`.

- [ ] **Step 4: `npx tsc --noEmit`, `npx eslint .`, and the file's own vitest run.**
Note that three-plus suites in this repo trip a load-dependent 5000ms timeout under
parallel load; run the file alone before concluding anything about a failure.

- [ ] **Step 5: Commit.**

---

### Task 9: Open the page

**Files:**
- Create: `docs/en/plans/2026-09-29-sp4d-2a-browser-result.md`

**This task is not optional and it is not a formality.** The `clearance_required`
defect in D10 shipped through SP4c's live engine gate, 35 commits and a full
review — because nobody opened the page. The predecessor UI package found five
defects in a browser that 5500 frontend tests did not. Every defect that mattered
in this programme was found by running a real engine or a real browser.

- [ ] **Step 1: Bring up the stack and sign in.** Recipe:
`docs/en/testing/` and the local-stack notes; admin credentials are in the repo's
dev docs.

- [ ] **Step 2: Drive the page for each case Task 8 tests**, plus at least one the
tests do not cover — a target whose catalog is served by both live profile types,
if the environment allows it.

- [ ] **Step 3: Record what you SEE**, not what the code should do: screenshots or
verbatim rendered text per case, and the network payload beside it. A prediction
made after seeing the result proves nothing, so write the expectation down first.

- [ ] **Step 4: Record every disagreement as a finding**, not as a test to adjust.

- [ ] **Step 5: Commit the result doc.**

---

## Definition of done

- `explain` answers per profile over the served artifact; the no-profile case is
  distinct from a denial; `Undecidable` carries a named cause.
- `roles` and `groups` reach the evaluator empty, and the page says why.
- `tagPolicies` tiers (a) and (b) are decidable; tier (c) refuses with a specific
  cause; **`ServiceConfig` still refuses**, asserted by a test.
- A real captured tag-policy fixture is in the tree, not an invented one.
- The two-profile fixture exists and a test asserts it really has two.
- `explain_writes_nothing` and `policy_read_permits_the_reads_and_refuses_the_writes`
  pass unchanged.
- `evaluate` is internal; the three agreement tests are renamed and a fourth added.
- `cargo test -p egret-ranger-eval -p egret-perm -p egret-perm-compile -p egret-policy`
  green against `egret_test`; `cargo fmt --check`, clippy, and
  `cargo check --workspace` clean; the OpenAPI drift gate green **run with `--lib`**.
- Frontend `tsc`, `eslint`, and vitest green.
- The browser result doc records a prediction made before each observation.
