# SP4d debt and guardrails — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** close the measured debt SP4d-1 / SP4d-2a / SP4d-2b left behind, and put a guardrail in front of the two states a supported API call can create today.

**Architecture:** no new subsystem. Every item here was measured against a real engine, a real Java evaluator, a real Postgres or a browser during SP4d, and each carries its reproduction so nobody re-derives it. Three items are write-path validation, two are schema or type constraints, one is test infrastructure that blocks verification.

**Tech Stack:** Rust (`egret-policy`, `egret-perm-compile`, `egret-test-db`), Postgres migrations, the Java differential harness at `tests/ranger/`.

## Global Constraints

- egret IS the Ranger Admin. No Apache Ranger server exists; the plugin in each engine downloads policy straight from egret. **Trino 470 runs `ranger-plugins-common-2.5.0`, StarRocks fe-ubuntu 4.1 runs `2.8.0`, and the two DISAGREE on at least one shape** — any claim about Ranger behaviour must name the plugin version.
- **A resource hierarchy is a property of the ENGINE, not a constant.** Anything emitting Ranger resource keys must take the engine or its service-def as an input.
- Every unmodelled or unreproducible construct REFUSES. Over-refusal is a real cost that produces no failing test, so each refusal states the population that reaches it **by code path**, and what would falsify that.
- `crates/egret-perm-compile/src/explain_wire.rs` mirrors the evaluator's types and must have **no wildcard match arms**. A new variant there is a compile error by design — **it is a shared chokepoint, so it has one owner per round, not a lane.**
- Drift gate: `cargo test -p egret-server --lib committed_openapi_json_is_current` — the `--lib` matters, or the test filters itself out and exits 0. Client regeneration is `npm run gen:api`, never bare `orval`.
- `just check` must stay green, and it now reaches `just test-ranger-gates` (measured 16.6 s + 105.6 s — the vacuous form of those two is `0.00 s`).
- Never run tests against the dev DB `egret`. Never use `git stash` (the stack is shared across worktrees).
- Mutations: `assert count == 1`, confirm each mutant COMPILES, restore by a plain write plus a SHA-256 compare. `cp -f` does **not** defeat the `cp -i` alias — it sits at the overwrite prompt, is auto-backgrounded, and the file stays mutated.

## Decided, not open

**Item 0 (`accessTypeResults`) stays REFUSED and is not modelled** (user, 2026-10-01). The mechanism is measured but the candidate model is a one-token change that the two-case fixture AGREES with — a fixture cannot tell a correct model from a lucky one — and `setAccessResultFrom` copying `policyPriority` unconditionally can also **lower** it, a direction nothing has measured. Cost of the refusal: 0 of 1148 composed-random cases.

**Item A is split** (user, 2026-10-01): block now, map later. Task 2 is the block. The per-engine mapping is a separate, fully measured piece of work and is **not** in this plan.

---

### Task 1: Stop the shared test template colliding across branches, then run the five unverified tests

**Files:**
- Modify: `crates/egret-test-db/src/lib.rs:80` (`const TEMPLATE_DB`)
- Test: the five DB-gated assertions named in Step 3

**Why this is first:** it blocks verification of everything else. `TEMPLATE_DB` is a hard-coded `const`, so every worktree on every branch shares one template and whichever branch ran last wins. Measured twice on 2026-10-01: `VersionMissing(160)` when this tree was at `0159` and another was at `0160`, then `VersionMissing(163)` **mid-run** when a third advanced it to `0163` while `main` was at `0162`. The failure is indistinguishable from a product defect by its message, and a controller once mislabelled ten such failures as environmental.

- [ ] **Step 1: Derive the template name from the migration set.** Name it from a stable digest of the migration **versions and checksums** `sqlx` already knows, not from `max(version)` — two branches can share a head and differ in the middle. Keep the old name working for nobody: the point is that a branch at a different migration set gets a different template, never a wrong one.

- [ ] **Step 2: Prove it cannot collide.** A test that builds two template names from two different migration sets and asserts they differ, plus one asserting the same set yields the same name (or the template is rebuilt on every run, which is the thing this avoids).

- [ ] **Step 3: Run the five assertions that have never run, and report each by name.**
`crates/egret-policy/tests/tag_resolution.rs` (6 tests — **the enricher-equivalence argument rests on this file**; 6/6 passed on 2026-10-01 in a window before the template moved, so this is a re-confirmation, not a first run), `explain_reports_the_tag_set_the_evaluator_was_handed` (the only automated assertion of the F4/F5 property, that the reported list IS the evaluator's input), the three end-to-end assertions at `crates/egret-perm-compile/tests/routes.rs` that the refusal lifts and the decline replaces it, and `tag_service_routing.rs:186-192`. **If any fails for a reason other than the template, that is a finding — report it before fixing it.**

- [ ] **Step 4: Commit.**

---

### Task 2: Refuse to bind a tag service to a StarRocks profile

**Files:**
- Modify: the `tagService` write path (`crates/egret-policy/src/store.rs`, the sole writer is `bind_trino_profile`; the reachable surface is the policy-admin service-create route)
- Test: a DB-free test on the validation, plus one on the route

**The measurement:** `ServiceTags::from_assignments` hard-codes `catalog`/`schema`/`table`/`column` and `download_tags` takes **no engine argument**, while StarRocks' service-def has **no `schema` resource** — so a StarRocks profile given a `tagService` downloads attachments its own enricher drops and **every tag policy in its block is silently inert**: no enforcement, no error. Nothing auto-binds it today (`bind_trino_profile` filters `tp.type = 'trino'`), but the policy-admin service-create route takes an arbitrary config, so the state is hand-creatable.

- [ ] **Step 1: Write the failing test** — creating or updating a service so that a StarRocks profile carries a `tagService` is refused, with a message naming the level StarRocks lacks.
- [ ] **Step 2: Run it, watch it fail. Step 3: Implement the refusal. Step 4: Confirm green.**
- [ ] **Step 5: Check the other direction.** Changing a profile's **type** to `starrocks` while it already carries a `tagService` must be refused too, or the guard has a hole the obvious test cannot see. Two guards that must agree have produced a defect in this programme four times.
- [ ] **Step 6: Mutate the refusal and confirm a test dies. Step 7: Commit.**

---

### Task 3: Constrain what `catalog_tag_assignments` can store

**Files:**
- Create: a migration adding the `CHECK`
- Modify: `crates/egret-policy/src/store.rs` (the three-class gate this makes redundant)

**The measurement:** nothing constrains the stored levels. `PUT …/resource-tags {"schema":"","table":""}` stores empty levels, and the download then ships a `{catalog}` or `{catalog, schema}` service resource that Ranger admits as an ANCESTOR while the resolver's exact predicate can **never return it** — so the set egret passes the evaluator is missing a tag the engine enforces, and an empty set licenses skipping the tag phase: a wrong confident verdict. SP4d-2b closed this with a three-class gate (`EmptyLevel`, `WildcardMetacharacter`, `FoldDivergent`). **Precedent for the cheap fix:** `registry.catalogs.name` already carries `CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$')`, one level up.

- [ ] **Step 1: Write the failing test** — the empty-level insert is rejected by the database.
- [ ] **Step 2: Add the `CHECK`.** Note `column_name` keeps its `''` sentinel — the constraint must not break it. Say in the migration which of the three classes the `CHECK` retires and which it does not: a `CHECK` can forbid an empty level and a wildcard metacharacter; it cannot decide fold divergence, which stays gated.
- [ ] **Step 3: Narrow the gate to what remains**, and update its reach rows so they describe the code path after the `CHECK`, not before.
- [ ] **Step 4: Confirm `just test-ranger-gates` still green. Step 5: Commit.**

---

### Task 4: Constrain `explain`'s target

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs` (the handler's validation, currently only the level skip)

**The measurement:** `ExplainRequest.target` is an `egret_perm::Scope` of plain `Option<String>` with no validator; the handler's 400 tests an *absent* level, never an empty one, and **does not check that the catalog exists, belongs to the caller's workspace, or is served by any profile**.

- [ ] **Step 1: Write the failing tests** — an empty level is a 400; a catalog id outside the caller's workspace is refused.
- [ ] **Step 2: Implement. Step 3: Confirm green.**
- [ ] **Step 4: Decide and record what an unserved catalog does.** It is a legitimate question — `no_profile_serves_the_catalog` is a real answer a steward wants. So this step is about the **workspace** boundary, not about refusing an unserved catalog; say so explicitly so the next reader does not "tidy" the outcome away.
- [ ] **Step 5: Commit.**

---

### Task 5: Refuse the shape that makes a table unreadable

**Files:**
- Modify: the rule write path and the tag-assignment write path (`crates/egret-perm-compile`, `crates/egret-policy`)
- Test: both directions, DB-free where the seam allows

**The measurement (live, Trino 470):** a tag-addressed `MASK` rule plus a **table-level** tag assignment makes Trino type-check the mask against **every column of the scanned table**, so `SELECT item`, `SELECT id` and even `count(*)` all fail — the table is **unreadable**, not masked. Reachable from two supported surfaces: `PUT …/resource-tags` with `column` absent, and `POST …/perm/rules` with a tag selector (which is how live policy 8799 exists). Not live today only because every live assignment is column-level on a `varchar`, every `id` is a `bigint`, and no `id` is tagged — **one assignment away**. The obvious explanation was measured and killed: a clean table-level MASK with no row filter fails identically, so the row filter is not the cause.

- [ ] **Step 1: Write the failing tests, both directions.** A tag-addressed MASK rule is refused when the tag is already attached at table level; **and** a table-level attachment is refused when a tag-addressed MASK rule already names that tag. The second is the one the obvious implementation misses.
- [ ] **Step 2: Run, watch both fail. Step 3: Implement. Step 4: Confirm green.**
- [ ] **Step 5: Say what the error says.** The steward needs the remedy, not the mechanism: attach the tag to the column, or make the rule resource-addressed.
- [ ] **Step 6: Mutate each direction separately and confirm a different test dies for each. Step 7: Commit.**

---

### Task 6: The two remaining correctness nits

**Files:**
- Modify: `crates/egret-policy/src/admin/ranger_json.rs:210`, `:273`
- Modify/Test: `crates/egret-policy/src/store.rs` (`resolved_tags_for_resource_via` vs `tag_assignments_for_service`)

- [ ] **Step 1: The ambiguous priority.** Both sites carry a comment saying two policies can land at one priority with different expressions and "**Ranger defines no order between them**" — for a mask or a row filter. That is a compiler emitting an artifact whose outcome is undefined. Either make the compiler not emit it, or refuse at compile time; do not leave a comment where a guard belongs. The evaluator already has `EvalOrderAmbiguous` for the read side — this is the write side.
- [ ] **Step 2: The resolver/writer coincidence.** `resolved_tags_for_resource_via` scopes on `a.warehouse_id = c.warehouse_id`; the download writer additionally requires `engine_catalog_attachments` and `authz_service_id`. They agree today only because `catalogs_for_service_via` happens to apply the same join with the same predicate — **compared by no test, no `CHECK` and no shared helper**, with a comment already noting the two binds differ in intent. One shared helper, or one set-equality test. A change to either join silently re-opens it in the over-deny direction.
- [ ] **Step 3: Confirm the gates. Step 4: Commit.**

---

### Task 7: two assertions merged in #611 that pin shapes the same PR changed

**Files:**
- Modify: `crates/egret-perm-compile/tests/routes.rs` (`explain_reports_the_tag_set_the_evaluator_was_handed`)
- Modify: `crates/egret-perm-compile/src/routes.rs` (`resolved_tags_for_display`'s doc link)

**Found by Task 1, and it is a defect merged in #611.** That test reads `body["tags"]` and
gets `Null`, for two independent reasons, neither environmental: the response-level `tags`
field **no longer exists** — the H3 fix moved the set to `ExplainProfile::tags` — and
`git show f5e2e84d` shows the test being **added** in the same squashed PR that moved it, so
the assertion was never re-pointed. It is DB-gated, so the broken shared template hid it.

**Consequence: the F4/F5 property — the reported list IS the evaluator's input — has no
passing automated assertion on `main`.** The three e2e tests that pass read decisions and
decline reasons; none reads `profiles[].tags` at all.

- [ ] **Step 1: Do NOT re-point it to expect `[]`.** The current fixture (`setup()`, one
StarRocks profile, no bound tag service) has `tag_policies_in_download: 0`, so
`tags_for_profile` returns empty by construction. Asserting `[]` there would be a green from
a population that cannot exercise the behaviour — the exact shape this programme has been
caught by five times — and would assert nothing about name-not-key or `attached_at_column`.

- [ ] **Step 2: Build a fixture that can fail.** `two_profile_catalog` plus a tag assigned at
a known coordinate plus a bound tag service carrying one policy. The assertion must pin all
three things the field exists for: the **name** rather than the `tags.key`, `attached_at_column`
matching the coordinate the assignment actually used, and the list being **non-empty for the
profile that resolves it and empty for the profile that does not** — which is also H1's
property, so one fixture covers both.

- [ ] **Step 3: Make the key assertable as absent.** The helper must let the test say the
`tags.key` does **not** appear, or name-not-key is pinned only by the happy path.

- [ ] **Step 4: Mutate three ways** — return the key instead of the name, invert
`attached_at_column`, and return the request-level set for every profile — and confirm a
different assertion dies for each.

- [ ] **Step 5: Fix the stale doc link.** `resolved_tags_for_display` links
`[ExplainResponse::tags]`, a field that no longer exists.

- [ ] **Step 6: The second one, same class, found by Task 2.**
`cause_access_priority_composite_rides_the_arbitration_wire_shape`
(`crates/egret-perm-compile/tests/wire_dtos.rs:593`) fails. It pins
`AccessPriorityCompositeNotModelled` as riding `tag_priority_arbitration_not_modelled`'s three
**positional** lists — and that cause now serialises as its **own named variant**, because the
final SP4d round split it deliberately (`explain_wire.rs`'s own doc records the split and names
the defect it fixed: the page was calling a resource policy a tag policy). **So the test is the
stale side, not the code** — but confirm that from `explain_wire.rs`'s contract before changing
it, because rewriting another package's pinned assertion on the strength of a comment is how a
real defect gets papered over.

**And note what this one proves, because it is worse than Task 7's half:** this test is
**DB-free**. The broken shared template never hid it. It reached `main` because **CI is out of
quota** and `just check` was the only gate — which is plan item 0b's cost, arriving. Say that
in the commit message; it is the evidence for why that item matters.

- [ ] **Step 7: Commit.**

---

### Task 8: `just check` forces a developer to disable 14 tests, permanently

**Files:**
- Modify: `justfile` (the `check` recipe's variable block at `:60`), and/or
  `docker-compose.dev.yml`
- Reference: `crates/egret-dataapi/tests/serving_shared_store.rs:54`

**Found by Task 1's review, and it is the same defect class as the hole Task 1 fixed — but
worse-shaped, because the recipe forces its own defeat.** `just check` decides the DB, ENGINE
and DOCKER axes and leaves **`EGRET_ALLOW_STORE_SKIP` undecided**. That variable gates **all
14** tests in `serving_shared_store.rs`. There is no Valkey in `docker-compose.dev.yml` and
none in `.env`; only `ci.yml:28` supplies the URL — and the justfile itself records CI as out
of quota, so `just check` **is** the gate. So without a Valkey the suite panics on 14 tests, a
developer exports the opt-out to get work done, and from that moment **every** `just check`
silently skips those 14 with the `eprintln!` swallowed.

This is outside DG, and it is in scope anyway: `just check` is the gate that protects
everything this plan touches, and a gate that makes its user disable part of itself does not
protect anything. **Do not fix the dataapi tests** — that is a different piece of work.

- [ ] **Step 1: Decide the axis in the recipe, the way the other three are decided.** Either
provide a Valkey for the run, or set the opt-out explicitly in `check` so the skip is the
recipe's stated choice rather than a developer's permanent workaround.
- [ ] **Step 2: Whichever you choose, make the skip impossible to misread.** A swallowed
`eprintln!` behind a green is the shape this programme has been caught by four times. If the
tests are skipped, the recipe says so where a human reads it.
- [ ] **Step 3: Check the remaining axes the same way.** The review read all eight `cargo test`
lines in the justfile and found no DB-axis hole left — but it also noted the guard test matches
the literal string `cargo test`, so a future `cargo nextest` recipe would slip past it. Widen
the guard or record the bound.
- [ ] **Step 4: Commit.**

---

### Task 9: the same silently-inert symptom, on the legitimate type

**Files:**
- Modify: `crates/egret-policy/src/admin/mod.rs:1031-1037` (the `{profile}_tag` fallback), the
  service write path's value shape
- Reference: `crates/egret-policy/src/ranger/api/download.rs:55-58`

**Found by Task 2's review.** Task 2's guards test key **presence**, deliberately and correctly,
so `POST /policies/services {"name":"p","type":"trino","config":{"tagService":""}}` is accepted
200. Then the two readers of that config **disagree**: `download.rs` does `.and_then(as_str)`
with **no fallback**, while `admin/mod.rs` falls back to `{profile}_tag`. So the tag editor files
policies into `p_tag` while the plugin receives **no `tagPolicies` block** — every one of those
tag policies silently inert, which is Task 2's own symptom **on the supported type**. It needs a
stranded `p_tag` row, and `engines.rs:2595` documents that as a real state ("Left by a deleted
same-name engine: reuse, never delete"). The compiler path is safe — `trigger.rs:404-418` warns
and skips.

- [ ] **Step 1: Do NOT fix it inside `carries_tag_service`.** That predicate is written to match
the migration's `->>` shape on purpose, and changing it breaks the agreement Task 2 built and
mutation-tested.

- [ ] **Step 2: Measure whether anything relies on the `{profile}_tag` fallback** before removing
it. If something does, that is the finding and it changes the fix.

- [ ] **Step 3: Make the two readers agree.** The root is the fallback: one reader invents a
default the other does not. Removing it makes the editor **refuse** rather than mis-file.

- [ ] **Step 4: Guard the value shape at the write path** so an empty `tagService` cannot be
stored at all — presence is the right test for the type rule, and it is not the right test for
whether the value names anything.

- [ ] **Step 5: Mutate each half separately** and confirm a different test dies for each.
- [ ] **Step 6: Commit.**

---

## Open, not in this plan — named so it is not mistaken for closed

**A StarRocks engine bound to a `trino`-typed profile that carries a `tagService` still
produces a wrong artifact, and is not refused at bind time.** Task 2's guard keys on the
profile's own `type`; this is a different invariant, cross-table — `engines.authz_service_id`
against `authz_services.type`. Today it is only **reported**, through
`UndecidableCause::EngineMismatch`. Closing it needs its own measurement, so it is not folded
into Task 2.

## Verification for every task

`cargo test -p egret-ranger-eval --no-fail-fast` (baseline **262/0 across 10 binaries**; the flag matters, or cargo stops at the first failing binary and undercounts) · `cargo test -p egret-perm-compile` with `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` passed **explicitly** · `cargo check --workspace --all-targets` · `cargo fmt --all -- --check` · scoped `clippy -D warnings` (note `cargo clippy --workspace` is rc 101 on `crates/egret-workspace/src/members.rs:191,196`, byte-identical to `origin/main` — pre-existing, not yours) · the `--lib` drift gate · `just test-ranger-gates` · and in `frontend/`: `npx tsc --noEmit`, `npx eslint .`, and the ExplainAccess suite by its real path `src/routes/governance/__tests__/ExplainAccess.test.tsx` (a path missing `__tests__/` exits 0 with "No test files found").

Never pass `-q` to anything — empty output is not a pass. Capture exit codes. Commands over ~120 s are auto-backgrounded and the completion notification IS the result; commit before starting a long suite.
