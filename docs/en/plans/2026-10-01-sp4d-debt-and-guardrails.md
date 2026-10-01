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

## Verification for every task

`cargo test -p egret-ranger-eval --no-fail-fast` (baseline **262/0 across 10 binaries**; the flag matters, or cargo stops at the first failing binary and undercounts) · `cargo test -p egret-perm-compile` with `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` passed **explicitly** · `cargo check --workspace --all-targets` · `cargo fmt --all -- --check` · scoped `clippy -D warnings` (note `cargo clippy --workspace` is rc 101 on `crates/egret-workspace/src/members.rs:191,196`, byte-identical to `origin/main` — pre-existing, not yours) · the `--lib` drift gate · `just test-ranger-gates` · and in `frontend/`: `npx tsc --noEmit`, `npx eslint .`, and the ExplainAccess suite by its real path `src/routes/governance/__tests__/ExplainAccess.test.tsx` (a path missing `__tests__/` exits 0 with "No test files found").

Never pass `-q` to anything — empty output is not a pass. Capture exit codes. Commands over ~120 s are auto-backgrounded and the completion notification IS the result; commit before starting a long suite.
