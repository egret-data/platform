# W4 — Runner & Container Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the three holes W3b handed forward — the `RETRYING` waker, task-level `timeout:`, and the kill path. My original figure of **20 baselined entries** was a union of three declaration greps; Tasks 1 and 2 measured the reachable count at **~0** (see the spec's §1.1), so this package closes real defects in the shipping path that **the oracle cannot see** — the position `egret.sql` was in. Then build `TaskCommands`, `egret.container` and `egret.script.python`, which reach **zero** corpus entries and carry their own tests. (**Not `egret.python`** — the spec's §6.4 gives that name to the notebook `KernelLauncher`, and `TASK_TYPES` has held it since W1 with a required `notebook` reference. Task 5 collided with it.)

**Architecture:** `crates/egret-workflow/src/worker/` gains a scheduler term, a timeout budget that comes from the flow rather than a constant, and a cancellation path. The runner lives behind `TaskCommands`, the contract Kestra's own `TaskRunner` takes — a name this repo deliberately does not reuse for the handler trait.

**Tech Stack:** Rust, `sqlx` against Postgres, Docker for the runner's own tests, `kestra/kestra@sha256:28f59bf1…` pinned by digest for anything measured against the artifact.

**Spec:** `docs/en/specs/2026-09-07-workflow-w4-runner-design.md`

## Global Constraints

Every task's requirements implicitly include this section.

- **`advance()` does not change.** W3a's function is pure over `(plan, state, now)` and eight tasks of oracle work judged it; W3b held it byte-unchanged across seventeen commits, verified by blob hash at each. A diff touching `src/executor/` needs a sentence saying why the oracle did not catch what you are fixing.
- **Result, `advance()` and successors stay in one transaction; the claim commits separately.** W3b's report says why: a lease that is never committed cannot expire.
- **Both writers of `task_runs.state` carry a lease fence.** W3b's Critical was a reaped worker committing over its replacement — two terminals on one attempt, `ended_seq` wiped, and an execution reaching `FAILED` with a live successor. The fences are **alternatives**, each sufficient alone, so every single-hunk mutation survives; that is measured and documented, not an accident.
- **Over-acceptance must not ship**, and W3b's whole-branch review opened a **runtime** register with eleven rows. A runner that starts a process Kestra would have killed is the next one.
- **A guard nobody has seen fail is a guard nobody has seen** — ten across W3a, W3c and W3b. Every guard gets a mutation, built first and asserted to compile, run against the whole crate; report the **message**.
- **A mutation table that cannot be replayed is a claim, not a record.** Literal `-` → `+` per row, anchored to `git show HEAD:<path>`; a killed replay is discarded whole, never topped up.
- **Derive every count; assert the total beside the parts.** Nine instances of a number written beside a derivation.
- **A check whose output does not depend on what it checks is not a check.** `cp -i` refuses and returns success; `tar xf` preserves mtimes so cargo rebuilds nothing; a snapshot taken mid-mutation verifies against itself; `git status --porcelain && echo CLEAN` prints CLEAN unconditionally — use `if [ -z "$(git status --porcelain)" ]`.
- **A suite that self-skips is not coverage.** `tests/sql_starrocks_live.rs` printed `7 passed` with no engine present, identical to a run with one; only `0.00s` versus `0.88s` distinguished them. A missing dependency must **fail**, and a skip must be typed (`EGRET_ALLOW_ENGINE_SKIP=1`).
- **Tests:** export `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`, `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399` and `EGRET_TEST_STARROCKS_URL`. **Run the FULL `cargo test --workspace --no-fail-fast`**, exit code read from the command itself, **never while another suite is running** — they share `egret_test`.
- Commits use `git -c user.email=hoangquocvietbk2.13@gmail.com`. Never commit `.superpowers/`, a scratchpad, or `__pycache__`. **No PRs** — the controller opens them.

## File Structure

```
crates/egret-workflow/src/worker/
  claim.rs        gains the RETRYING term; its "advance() enforces the delay" comment is WRONG and goes
  timeout.rs      the budget, from the flow, replacing sql.rs's 300s constant
  cancel.rs       the kill path — must trip egret.sql's deliberate refusal
  commands.rs     TaskCommands: the contract Kestra's TaskRunner takes
  handlers/
    container.rs  egret.container
    python.rs     egret.script.python  (NOT egret.python: the spec's own type table gives that
                  name to the notebook KernelLauncher, and TASK_TYPES has held it
                  since W1 with a required `notebook` ref. Task 5 collided with it.)
```

---

## Task 1: The `RETRYING` waker, and the sentence that is false

**Files:** modify `src/worker/claim.rs`; modify `tests/worker.rs`.

Twelve baselined entries declare a `retry:` with a non-zero `interval:`. Today a retry on a run with **no concurrently-live sibling** sits `RETRYING` for ever: the claim excludes `RETRYING`, `reap` excludes it, `reopen_due` fires only inside a `settle` whose `now` is fixed for the transaction, and `resume`'s only caller is the oracle driver via `pass_time`, which **rewinds `executions.created_at`** to fake the delay.

- [ ] **Step 1: Reproduce the stall before fixing it.** A one-task flow with `retry: {constant, interval: PT1S, maxAttempts: 2}` and no sibling. Assert it is still `RETRYING` after the interval has passed — and assert *what* is stuck, not merely that nothing happened, because "no rows claimed" is also what a correct idle loop returns.
- [ ] **Step 2: Run it and watch it hang**, then implement the waker. `claim.rs:24-25`'s justification for omitting `not_before` is false and goes with the fix; replace it with what is actually true.
- [ ] **Step 3: Mutation** — remove the `RETRYING` term. It must fire, and the message must name the run, not the count.
- [ ] **Step 4: Report the ratchet before and after**, attributing every moved entry. Twelve entries are *reachable*; how many actually move is a measurement, and an entry that banks for an unattributed reason is a coincidence.
- [ ] **Step 5: Full gate, commit.**

---

## Task 2: `timeout:` reaches the handler

**Files:** create `src/worker/timeout.rs`; modify `src/worker/claim.rs`, `handlers/sql.rs`.

W3a Task 3 established against the artifact that Kestra's **worker** enforces `timeout` (`WorkerTaskCallable`; its validator warns the property *"is only relevant for runnable tasks"*), correcting an earlier brief of mine that had called it `advance()`'s. W3b then measured that a task-level `timeout:` is **stripped before a handler sees it**, so `egret.sql` runs under a 300 s constant unrelated to what the author wrote. Four baselined entries declare one.

**Task 1 left you a named failure mode, and it is silent.** `advance()` never clears `not_before`, so the **state term is the waker's only defence**: a `RETRYING` run is not runnable until `advance()` returns it to `CREATED`. If your timeout budget reuses that column, the waker breaks **without any test saying so**. The clean fix — clearing it in `reopen_due` — is an `advance()` change, which Task 1 was not allowed to make and you are not either without the sentence the Global Constraints demand. Pick a different column, or make the case for touching `advance()` and measure it.

Also correct before you rely on it: the reachable population is **14 entries with a `retry:` block, of which 4 are `CREATE_NEW_EXECUTION` and never write a `not_before` at all** — so 10 could ever have stalled, not the 12 my Task 1 brief said.

- [ ] **Step 1: Write the failing test** — a flow whose task declares `timeout: PT1S` against a handler that sleeps longer, asserting the run's **terminal state and message**, not that an error occurred.
- [ ] **Step 2: Run it and watch it pass wrongly** (the 300 s constant lets it finish), which is the interesting failure: the test must be written so that it fails *now*.
- [ ] **Step 3: Implement**, and read Kestra's own behaviour for the boundary rather than choosing one — `javap` the pinned image or run a flow through it, because the image and the checkout are different builds and that has produced two wrong readings already.
- [ ] **Step 4: Mutation** — make the budget the old constant again. Report the message.
- [ ] **Step 5: Ratchet before/after with attribution; full gate; commit.**

---

## Task 3: The kill path, and the tripwire it must trip

**Files:** create `src/worker/cancel.rs`; modify `handlers/sql.rs`, `src/worker/mod.rs`.

W3b Task 6 did **not** implement `TaskHandler::cancel` for `egret.sql`. It made it **refuse**, with the reasoning recorded: implementing a method nobody invokes produces a guard nobody can see fail. That refusal is a tripwire at the one site that must change.

- [ ] **Step 1: Trip it.** Wire the kill path and watch `egret.sql`'s `cancel` refuse. **If it does not refuse, that is a finding** — it means the path you wired does not reach the site W3b marked, and you must say where it goes instead before continuing.
- [ ] **Step 2: The lease fence is the other half.** W3b's Critical was a reaped worker committing over its replacement; cancellation is the same seam from the other side. **A killed handler must not commit either, and the fence should already refuse it — verify that rather than assume it**, with a test that would fail if the fence were absent.
- [ ] **Step 3: Implement `cancel` for the handlers that can honour it**, and leave a refusal where a handler genuinely cannot, with the reason.
- [ ] **Step 4: Mutation** — remove the fence's read on the cancel path. Report the message.
- [ ] **Step 5: Ratchet before/after with attribution; full gate; commit.**

---

## Task 4: The one-run-per-tick ruling

**Files:** modify `src/worker/claim.rs` or record the decision; modify `oracle-exec/README.md`.

W3b ruled that the tick finishes one run because widening it changes the transaction boundary the durability properties rest on, and handed the decision here on the grounds that W4 introduces a real worker pool. Four entries — `valids/dag`, `valids/if`, `valids/parallel-disabled-tasks`, `valids/full` — differ on the happens-before relation **alone** because of it.

- [ ] **Step 1: Measure both sides before choosing.** What does widening cost in transaction shape, and what does it buy in entries? W3b's four durability properties plus the fifth are the things that must survive either way — run them under the widened tick.
- [ ] **Step 2: Choose, and write the cost down.** Either answer is acceptable **with its cost measured**. What is not acceptable is choosing by how many entries it banks — W3b measured that reversing a tiebreak banks `valids/dag` and loses `parallel-nested`, and refused it on those grounds, because **choosing an order by what it banks is how an oracle gets tuned to its answer**.
- [ ] **Step 3: Full gate, commit.**

---

## Task 5: `TaskCommands`, `egret.container`, `egret.script.python`

**Files:** create `src/worker/commands.rs`, `handlers/{container,python}.rs`; create their tests.

**Nothing in the corpus exercises any of this** — measured, zero script, container, shell or command task types in 345 flows. The ratchet will not move and that is not evidence of anything.

- [ ] **Step 1: Read Kestra's `TaskRunner` and `TaskCommands` first** and record the contract with file and line, from the pinned image where behaviour is at stake. The trait this repo already has is `TaskHandler`, deliberately not `TaskRunner`; this is the task where both names exist and the distinction must hold.
- [ ] **Step 2: Write the tests first, against a real daemon.** A missing Docker must **fail**, and a skip must be typed — `egret.sql`'s suite printed `7 passed` with no engine until that rule was applied to it. Note the cost is higher here: more machines lack Docker than lack a database, so the failure message must say exactly what to install or export.
- [ ] **Step 3: Implement.** Log streaming and output files are part of the contract, not extras.
- [ ] **Step 4: Mutation per handler**, including one that makes the runner report success without running anything. If that kills nothing, say so plainly rather than move on.
- [ ] **Step 5: Full gate, commit.**

---

## Task 6: The coverage statement

**Files:** modify `oracle-exec/README.md`, `oracle-exec/runnable.txt`.

- [ ] **Step 1: Re-derive** every number the statement carries. W3a's was found over-claiming by 227; W3b's own Task 7 then cut the same claim by a further 750, on the grounds that a pair on a *baselined* entry is compared and cannot be falsified. Check whether that argument now applies anywhere new.
- [ ] **Step 2: Separate what the ratchet speaks for from what it cannot.** ~~Twenty entries were reachable by the holes~~ — **~0 were; see the spec's §1.1**, and this line kept the retracted union figure while the Goal at the top of this file already carried its correction, which is how it reached Task 6's brief and had to be re-derived there. The runner reaches zero and rests on its own tests; and durability rests on five properties, one of which the ratchet can never see because a task run twice producing the same terminal produces an identical trace.
- [ ] **Step 3: Sweep for a stale twin.** W3a's README carried two true-at-different-moments figures for one quantity with only prose to say which; W3b's sweep found thirteen cited test names that no longer existed, nine of them deleted with the harness it replaced.
- [ ] **Step 4: Full gate, commit.**

## Final review

Dispatch the whole-branch review on the most capable model. The seams here are the lease fence (approached from cancellation rather than reaping this time), the transaction boundary if Task 4 widens it, and the coverage statement's before/after — where a number that moved for two reasons at once will look like it moved for one. W3b's whole-branch review found its Critical *between* two correct durability properties; assume this one is between two correct tasks.
