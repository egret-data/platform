# W3b — Durable Executor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `advance()` run for real — a claim loop over Postgres, a reaper, five handlers, and an executor for `SpawnExecution` — and then re-point W3a's differential oracle at that driver, so what the ratchet judges is the system that ships.

**Architecture:** `crates/egret-workflow/src/worker/` holds the claim loop, the reaper, and the handler registry. `advance()` is unchanged and W3b is a *caller*. `oracle-exec/` keeps the same corpus and golden; only the thing behind them changes.

**Tech Stack:** Rust, `sqlx` against Postgres, `kestra/kestra:v2.0.0-rc13` pinned for any re-harvest, the existing `egret-automation` claim statement.

**Spec:** `docs/en/specs/2026-09-06-workflow-w3b-durable-design.md`

## Global Constraints

Every task's requirements implicitly include this section.

- **`advance()` does not change.** W3a's function is pure over `(plan, state, now)` and eight tasks of oracle work judged it. If the real driver appears to need different semantics, that is a **finding to measure and report**, not a refactor to perform. A diff that edits `src/executor/` needs a sentence saying why the oracle did not catch what you are fixing.
- **Reuse `egret-automation`'s claim statement.** `crates/egret-automation/src/scheduler.rs` already does `UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED LIMIT n) RETURNING …`, with its own comment explaining why. Do not invent a second one.
- **The worker's result, `advance()`, and the successors go in ONE transaction.** No window may exist where a task is finished and the execution has not moved.
- **Over-acceptance must not ship.** W3a closed two, W3c a third, and W3c's own fix produced a fourth that it then closed. A durable driver has more places to run something Kestra did not.
- **A guard nobody has seen fail is a guard nobody has seen.** Eight shipped or nearly shipped across W3a and W3c. Every guard gets a mutation, built first and asserted to compile, run against the whole crate, and you report the **message**.
- **A mutation table that cannot be replayed is a claim, not a record.** Literal `-` → `+` per row, anchored to `git show HEAD:<path>`, counts from a replay of the final tree. A killed replay is discarded whole.
- **Derive every count; assert the total beside the parts.** Eight instances of a number written down beside a derivation.
- **A check whose output does not depend on what it checks is not a check.** `cp -i` refuses and returns success; `tar xf` preserves mtimes so cargo rebuilds nothing; `git status --porcelain && echo CLEAN` prints CLEAN unconditionally. Use `if [ -z "$(git status --porcelain)" ]`.
- **Tests:** export `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` and `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`. Never the dev database.
- **Run the FULL `cargo test --workspace --no-fail-fast`** before reporting DONE, reading the exit code from the command itself. **Never while another suite is running** — they share `egret_test`.
- Commits use `git -c user.email=hoangquocvietbk2.13@gmail.com`. Never commit `.superpowers/`, a scratchpad, or `__pycache__`. **No PRs** — the controller opens them.

## File Structure

```
crates/egret-workflow/src/worker/
  mod.rs        the loops' public entry; no business logic
  claim.rs      the claim predicate and the one statement
  reaper.rs     working_timeout_s reclamation
  registry.rs   type_id -> handler dispatch
  handlers/
    log.rs           core.log.Log
    ret.rs           core.debug.Return
    fail.rs          core.execution.Fail
    sleep.rs         core.flow.Sleep      (a RunnableTask despite the module)
    sql.rs           egret.sql            (no oracle coverage — see the spec)
  spawn.rs      the SpawnExecution executor
oracle-exec/
  driver.rs     the real driver behind the ratchet, replacing the test harness
```

---

## Task 1: How much of the corpus can a real handler actually reproduce?

**Files:** Create `crates/egret-workflow/oracle-exec/handler-surface.txt`; modify `oracle-exec/README.md`.

The spec's Risk 1: the corpus was harvested from a Kestra that **ran** tasks, and egret's handlers are not Kestra's. `Return` renders a format string; `Log` writes a message; `Fail` throws. Where the golden's terminal depends on what a task *did* rather than that it ran, egret must reproduce the doing.

- [ ] **Step 1: Derive the population.** Of the 266 comparable `runs` entries, **194 use only `Log`, `Return`, `Fail`, `Sleep`** as non-flowable tasks. Re-derive that number rather than quoting it; if it differs, believe yours and say so.
- [ ] **Step 2: Measure the dependency on outputs.** For each of the 194, determine whether any expression in the flow reads a task's **output** (`{{ outputs.… }}`, `{{ taskrun.… }}`, `tasksWithState()`). Those are the entries where "the handler ran" is not enough. Publish the split, derived, with the total asserted beside the parts.
- [ ] **Step 3: Say what each handler must produce.** `Return` has an output (`value`); `Log` does not; `Fail` produces a failure with a message; `Sleep` produces nothing. Read Kestra's classes for each and record the output shape, with the file and line.
- [ ] **Step 4: Publish the reachable ceiling.** How many of the 294 comparable entries could bank if the driver were perfect and the handlers exact? That number bounds everything the rest of this plan can claim, and no later task may report a figure above it without moving it here first.
- [ ] **Step 5: Commit.**

---

## Task 2: The claim loop, and the durability properties the ratchet cannot see

**Files:** Create `src/worker/{mod,claim,reaper}.rs`; create `tests/worker.rs`.

**Interfaces produced:**
```rust
/// Claims up to `n` runnable task runs and hands each to `run`, committing the
/// handler's result, `advance()`'s commands and the successors in ONE transaction.
pub async fn tick(pool: &PgPool, n: usize, reg: &Registry) -> Result<usize>;
/// Reclaims runs whose lease expired. Separate from `tick` so a test can drive it.
pub async fn reap(pool: &PgPool, timeout: Duration) -> Result<usize>;
```

- [ ] **Step 1: Write the failing durability tests first.** These are the ones the oracle can never see:
  - two concurrent `tick`s never claim the same run — assert on the claimed ids, not on the absence of an error;
  - a process killed between the handler and the commit leaves **no** half-applied execution — drive it by dropping the transaction, and assert the run is claimable again;
  - `reap` reclaims exactly what `working_timeout_s` says: one run past the lease, one inside it, and assert **both** directions.
- [ ] **Step 2: Run them and watch each fail**, naming what it wanted.
- [ ] **Step 3: Implement `claim.rs` on `egret-automation`'s statement.** Its `scheduler.rs:265` comment explains why `FOR UPDATE SKIP LOCKED` is there; cite it rather than restating it.
- [ ] **Step 4: Implement `reap`.**
- [ ] **Step 5: Mutation per property** — remove `SKIP LOCKED`, widen the lease predicate, move the `advance()` call outside the transaction. Each must fire; report the message.
- [ ] **Step 6: Full gate, commit.**

---

## Task 3: The four handlers the oracle needs

**Files:** Create `src/worker/registry.rs`, `src/worker/handlers/{log,ret,fail,sleep}.rs`.

**Interfaces consumed:** Task 2's `Registry`. **Produced:** `TaskHandler` with `type_id`, `run`, `cancel` — the trait the programme spec §6.4 already names, deliberately **not** called `TaskRunner`, because in Kestra that name means a shell-command executor.

- [ ] **Step 1: Read each Kestra class and record the output shape** with file and line, from Task 1 Step 3's table. Do not infer `Return`'s output from its name.
- [ ] **Step 2: Write one failing test per handler** asserting the output shape, not merely that it ran.
- [ ] **Step 3: Implement.** `Sleep` is a `RunnableTask` (`Sleep.java:47`) — a handler, not a flowable. **On its duration, measure rather than inherit:** an earlier brief of mine said `PT0S` is refused, citing the W3a spike, and Task 1 could not confirm it from the class — `Sleep.java` carries only `@NotNull`, no positivity constraint, and `sleep(0)` does not throw. The refusal I saw came from somewhere else in the publish path. Find where, or find that it does not happen.
- [ ] **Step 4: Mutation per handler**, including one that makes `Return` produce no output — if that kills nothing, the output is not yet compared and you must say so rather than move on.
- [ ] **Step 5: Full gate, commit.**

---

## Task 4: `SpawnExecution`, and the 39 entries that differ on nothing else

**Files:** Create `src/worker/spawn.rs`; modify `oracle-exec/README.md`.

W3a emits this command for a **loop iteration** and nothing executes it. Closing it is what makes 39 entries and 227 ordered pairs comparable.

- [ ] **Step 1: Re-derive both counts** — entries differing only on `spawned`, and ordered pairs inside `spawned` — before changing anything. The published figures are 39 and 227; if yours differ, believe yours.
- [ ] **Step 2: Write the failing test:** a `Loop` over two values spawns two executions, ordered by `loopRun.index` and **not** by creation instant — ordering by instant made nine flows fail W3a's stability check.
- [ ] **Step 3: Implement**, respecting `concurrencyLimit`: the default is **1**, and **0 means unlimited** (`Loop.java:233`, `:422`; established from Kestra's own traces, not from the schema).
- [ ] **Step 4: Report both counts again**, and the ratchet before and after. Do not describe the hole as closed; report what moved.
- [ ] **Step 5: Full gate, commit.**

---

## Task 5: Re-point the oracle at the real driver

**Files:** Create `oracle-exec/driver.rs`; modify `tests/executor.rs`, `oracle-exec/README.md`.

This is the package's deliverable. The harness's four tiers go; the driver replaces them.

**Task 1 measured that this step can LOSE entries, and that is the decision this task owns.** The ceiling is **172**; today's ratchet matches **150**; and the move is **139 kept, 11 LOST, 33 gained**. Those 11 match today only because W3a's harness *feeds* a terminal for a task type W3b has no handler for — `OutputValues` ×6, `PurgeLogs` ×5, `Assert` ×1. Keeping that feed raises the bound to 183 and keeps the 11; dropping it is honest and costs them. **Either way this task reports it as a decision with its cost, never as a number that happened.** The working expectation is **167**, because five otherwise-bankable entries carry a W3a cause Task 1's derivation cannot see.

- [ ] **Step 1: Record the before state** — ratchet, the fed-terminal split, the ordered-pair split, and the circular region as W3a's README derives them. Every one of these should move, and the ones that do not are as interesting as the ones that do.
- [ ] **Step 2: Expect the deleted guard back.** W3a's Task 4 deleted Kestra's "first created, leave" guard as unreachable and named its trigger: *"a caller that applies a batch partially — W4's worker."* That caller is now yours, and it was measured to fire on `valids/dag`, `valids/if` and `valids/parallel-disabled-tasks`. If it does not fire, find out why before assuming it is gone.
**Two corrections from Task 3 that this task must not inherit wrong.**

- **"No output" is `{}`, not an absent key.** I wrote the opposite into Task 3's
  brief and labelled it load-bearing for exactly this step. Measured on the
  artifact: `{{ outputs.a }}` renders `{}` for a `Log`, a `Sleep`, a falsy `Fail`
  **and** a format-less `Return`; `{{ outputs }}` over two tasks renders
  `{"a":{},"b":{}}`; `{{ outputs.a.value }}` fails the run in all four. Map "no
  output" to `{}` and never omit the key.
- **Nothing persists `Outcome::outputs` yet** — there is no `task_runs.outputs`
  column, so Task 3's guard on `Return`'s output dies only to in-process tests.
  This task needs the column as well as the rule.

- [ ] **Step 3: Attribute every moved entry.** The render surface widens because the driver has `outputs`, so entries baselined for reasons unrelated to durability will move. Each must be attributed to a cause — a handler, the spawner, or the render surface. An entry that banks for an unattributed reason is not a bank, it is a coincidence.
- [ ] **Step 4: The circular region.** With terminals coming from handlers that ran, re-derive it. If it is now empty, prove that by corrupting a golden terminal and showing the ratchet fails — W3a's first attempt at exactly this demonstration was vacuous because the chosen entry could not match under any value.
- [ ] **Step 5: Full gate, commit.**

---

## Task 6: `egret.sql`, which ships with no oracle coverage

**Files:** Create `src/worker/handlers/sql.rs`; modify `oracle-exec/README.md`.

- [ ] **Step 1: Write the tests first**, against a real engine, the way `egret-dataapi` does. There is no corpus flow to fall back on — measured, `egret.sql` reaches **zero** entries.
- [ ] **Step 2: Implement** on `QueryExecPort`, which is already fail-closed on workspace scoping, and the `bind_formatter` path.
- [ ] **Step 3: Say so in the coverage statement.** The README must separate what the ratchet proves from what this handler's own tests prove, so a green ratchet is not read as coverage it does not have.
- [ ] **Step 4: Full gate, commit.**

---

## Task 7: The coverage statement, again

**Files:** Modify `oracle-exec/README.md`, `oracle-exec/runnable.txt`.

W3a's statement is now wrong in every direction that matters, and the point of this task is that the new one is **derived**, not edited.

- [ ] **Step 1: Re-derive every number** the statement carries: comparable, matching, baselined, never-compared, ordered pairs asserted versus kept, the fed split, the residual baseline by construct with its owners.
- [ ] **Step 2: State what the real driver still does not prove.** Durability is tested by Task 2's properties and not by the corpus; `egret.sql` by its own tests; and the corpus remains Kestra's own test suite, which is a sample of an infinite space.
- [ ] **Step 3: Check for a stale twin.** W3a's README ended up carrying two true-at-different-moments figures for the same quantity with only prose to say which. Sweep for that shape.
- [ ] **Step 4: Full gate, commit.**

## Final review

Dispatch the whole-branch review on the most capable model. The seams here are `advance()` (unchanged by rule — verify it really is), the transaction boundary (one window is one bug), and the oracle's own before/after, where a number that moved for two reasons at once will look like it moved for one.
