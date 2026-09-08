# W6 — Subflow, Replay & Triggers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bank the 13–15 baselined entries `Subflow` and the
`CREATE_NEW_EXECUTION` replay can reach, then build cron, webhook, flow triggers
and backfill — which reach **zero** baselined entries and are judged only by
their own tests.

**Architecture:** `Subflow` gets its **own link column**, not
`parent_execution_id`, because `driver.rs` builds the oracle's `spawned` field
from that column and all 41 golden `spawned`-carrying entries have zero `Subflow`
children. The replay needs the same schema change, so it is one migration. The
oracle driver must publish the corpus into `workflow.flows` before a `Subflow`
can resolve `(namespace, flowId)` at all.

**Tech Stack:** Rust, sqlx/Postgres, the `oracle-exec` differential harness, the
pinned image `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`.

## Global Constraints

Every task's requirements implicitly include this section.

- **`crates/egret-workflow/src/executor/` has been byte-unchanged since W3a** — 11 files, blob-identical to `ad93ef2d`. **W6 may be the package that legitimately opens it** (W4's coverage statement named the trigger: the first package that needs `executor/retry.rs` for the `CREATE_NEW_EXECUTION` replay, or W8). If you open it: say why in the commit, treat it as a **finding** rather than a step, and **fix the two knowingly-wrong doc pointers recorded there in the same commit that opens the directory** — that is the condition under which they were left.
- **`advance()` is pure over `(plan, state, now)`** — no I/O, no clock. A change to its *contract* is a finding, not a refactor.
- **Result, `advance()` and successors stay in one transaction; the claim commits separately** — a lease that is never committed cannot expire. The tick applies **one handler result per transaction** (W4's ruling; `driver::BATCH` = 1).
- **`Subflow` must not be linked by `parent_execution_id`.** Measured: `driver.rs` builds `spawned` from it, and all 41 golden `spawned`-carrying entries have zero `Subflow` children. Kestra sets `parentId` for a loop iteration and for a replay, and leaves it NULL for a subflow child.
- **Every banked entry must be attributed.** An entry that moves for an unattributed reason is a coincidence, not a bank. Publishing the corpus into the driver widens the resolution surface and *will* move entries for unrelated reasons — that is W3b's §4.4 failure with a new cause.
- **Over-acceptance must not ship.** A trigger that fires where Kestra would not is the next instance, and **the oracle cannot catch it**.
- **A guard nobody has seen fail is a guard nobody has seen** — ten across W3a/W3c/W3b. W4 added the inverse: a *redundant* guard and a weak one look identical in a mutation table; measure the difference and write it down.
- **A figure is safe only if a running program compares it.** W4's sweep took five passes and 23 corrections to reach that rule. It applies to anything W6 writes, including this plan.
- **A check whose output does not depend on what it checks is not a check.** Four variants bit W4: `cp -i` refuses and returns success; `git status --porcelain && echo CLEAN` prints CLEAN unconditionally (use `if [ -z "$(git status --porcelain)" ]`); `ps aux | grep -c '[c]argo test'` returns **1 when nothing runs**; `ps -axo comm | grep -c '^cargo$'` returns **0 when something does**; and a **third** variant — an anchored match on the `comm` column of `ps -axo pid,comm,args` reports 0 with a live `cargo test` in front of it. Two mechanisms combine, and the first draft of this line got them wrong: `ps -axo comm` **alone is not truncated** (measured at 277 chars), it is the **multi-column** form that pads `comm` to 16; and macOS `comm` is a **full path**, so `^cargo$` could never match regardless. **Match the `args` column**, and try any such check in **both** states — with a suite running and without — before trusting it.
- **Mutations:** every guard gets one, built first and asserted to compile, with a **unique** anchor verified against the whole file (a W4 anchor matched a copy inside a test fixture and the harness reported green on unmutated code). Use `set -e`; assert the file actually changed. **Commit before you deliberately break something.**
- **A suite that self-skips is not coverage.** A missing dependency must **fail**; a skip must be typed; and a skipped suite must report `0 passed; N ignored`, **never `N passed`**.
- **Tests:** export `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`, `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`, `EGRET_TEST_STARROCKS_URL`. Run the **full** `cargo test --workspace --no-fail-fast`, exit read from the command, **never while another suite runs** — they share `egret_test`. Baseline gate: **257 suites / 3233 passed / 0 failed / 5 ignored**, exit 0.
- **Do not re-harvest and do not rewrite `baseline.txt`'s reference point.** Making triggers measurable is a separate package; its cost is a re-harvest plus a rewritten baseline, which is the reference every published W1–W4 number stands on.
- Commits use `git -c user.email=hoangquocvietbk2.13@gmail.com`. Never commit `.superpowers/`, a scratchpad, or `__pycache__`. **No PRs** — the controller opens them.

## File Structure

```
crates/egret-workflow/
  migrations/…                     ONE migration: subflow's own link column + the replay's
  src/model/…                      Subflow's spec shape; trigger declarations
  src/worker/subflow.rs            resolve (namespace, flowId) -> publish -> spawn -> link
  src/worker/replay.rs             CREATE_NEW_EXECUTION
  src/trigger/
    mod.rs                         the trigger contract; an INJECTED clock, never SystemTime
    schedule.rs                    cron + backfill (same clock)
    flow.rs                        fires on another execution's terminal state
    webhook.rs                     the HTTP surface
  oracle-exec/driver.rs            publishes the corpus into workflow.flows
  oracle-exec/README.md            the coverage statement + the four corrections
```

---

## Task 1: The driver publishes the corpus, and everything that moves is attributed

**Files:** modify `oracle-exec/driver.rs`; modify `oracle-exec/README.md`.

`driver.rs` calls `start_execution(..., None, source, inputs)` — it **never
publishes the corpus into `workflow.flows`**, so there is no table for a
`Subflow` to resolve against and egret cannot execute one at all.

- [ ] **Step 1: Ratchet before.** Record 143 and the per-entry list. This is the reference for every later attribution.
- [ ] **Step 2: Publish the corpus into `workflow.flows` from the driver**, using the same publish path the product uses — not a test-only insert. If the product's publish path refuses any corpus flow, that refusal is a **finding** (W1 shipped a `build_plan` that never called `spec::validate`, and seven flows were refused at publish for a year).
- [ ] **Step 3: Ratchet after, and attribute every single move.** Expect moves unrelated to `Subflow`: the resolution surface just widened. For each moved entry, name the mechanism. **An entry that moves for a reason you cannot name is a finding, not a bank** — do not bank it in this task.
- [ ] **Step 4: Full gate; commit.**

---

## Task 2: One migration — Subflow's own link, and the replay's

**Files:** create `migrations/0138_workflow_subflow_link.sql`; modify the model.
(Verified: `0137` is the highest today, so `0138` is the next number. The constraint to read is `workflow_executions_loop_run_all_or_nothing`, added at `migrations/0135_workflow_loop_sub_executions.sql:41`, whose own comment says it is *"a CHECK rather than a rule every writer must"* remember — the same principle this task must preserve.)

Measured: migration `0135`'s all-or-nothing CHECK makes a child with a parent but
**no loop index** unrepresentable, which blocks both `Subflow` children and the
four replay entries. Kestra links a subflow child by `trigger_execution_id` and
leaves `parentId` NULL; it sets `parentId` for a loop iteration **and** for a
replay.

- [ ] **Step 1: Read `0135` and state what its CHECK actually forbids**, quoting it. Then state which of the three child kinds (loop iteration, subflow child, replay) each existing column can and cannot express.
- [ ] **Step 2: Write the migration** so all three are representable and the wrong combinations remain **unrepresentable** — a CHECK, not a convention. This programme's rule: unrepresentable beats unforgettable.
- [ ] **Step 3: A test that fails if the CHECK is dropped.** Insert each illegal combination and assert the database refuses it. A migration whose constraint no test exercises is a guard nobody has seen fail.
- [ ] **Step 4: Verify `driver.rs`'s `spawned` query is unchanged in behaviour** — it reads `parent_execution_id`, and a subflow child must **not** appear there. Assert that with a test, because this is the constraint the whole package's banks depend on.
- [ ] **Step 5: Full gate; commit.**

---

## Task 3: `Subflow` executes, and the `src/executor/` decision

**Files:** create `src/worker/subflow.rs`; possibly modify `src/executor/plan.rs`.

`build_plan` plans `Subflow` as an **opaque `TaskKind::Runnable`** today
(`plan.rs:615-616`), which is why W4's kill-path measurement had to be corrected.

- [ ] **Step 1: Measure what Kestra does**, on the pinned artifact, not from the name: what states does a parent in a `Subflow` reach and when, what does the child's trace look like, and what does the parent see of the child's outputs. Record it with the flow ids you used.
- [ ] **Step 2: Decide whether `Subflow` stays opaque.** If the golden can be reproduced with `Subflow` as an opaque runnable whose handler spawns and waits, **`src/executor/` does not open** — that is the cheaper answer and it preserves a constraint that has held three packages. If it cannot, opening `src/executor/` is a **finding**: say what the oracle could not otherwise reproduce, and fix the two recorded stale doc pointers in the same commit.
- [ ] **Step 3: Implement**, linking the child by the column from Task 2 and **never** by `parent_execution_id`.
- [ ] **Step 4: The unresolvable target.** `valids/task-runif`'s `Subflow` target is **not in the corpus**. Decide what egret does when `(namespace, flowId)` does not resolve, measure what Kestra does, and make them agree — this is an over-acceptance risk, and the oracle only covers it for the one entry.
- [ ] **Step 5: Mutation** — break the link column so a child is attached the wrong way, and confirm a `spawned` assertion fails. Report the message.
- [ ] **Step 6: Ratchet before/after with attribution; full gate; commit.**

---

## Task 4: `CREATE_NEW_EXECUTION` replay — the four entries

**Files:** create `src/worker/replay.rs`; possibly modify `src/executor/retry.rs`.

These four are the **only** row `oracle-exec/README.md`'s residual table assigns
to W6. Their golden `spawned` is `parent_run: null, value: null`.

- [ ] **Step 1: Establish where the replay is decided.** W3a's `retry::policy_for` already handles `CREATE_NEW_EXECUTION` as a policy kind that emits no `ScheduleRetry`. Determine whether the new execution can be spawned by a caller without changing `advance()`'s output. If it cannot, that is the finding that opens `src/executor/`.
- [ ] **Step 2: Implement the replay**, setting `parentId` (Kestra does, for a replay) and the loop index NULL — the combination Task 2's migration made representable.
- [ ] **Step 3: A test per entry**, named for the entry it banks.
- [ ] **Step 4: Correct `ENTRIES_LEFT_UNFINISHED`'s classifier.** Measured: its assertion message says every one of the 84 is a task type with no handler, but these four declare only `core.execution.Fail` — which **has** a handler — and end in `RETRIED`, which `is_stalled` omits from its terminal list although `RunState::Retried`'s own doc and Kestra's `State.isTerminated()` both call it terminal. Fix the classifier **and** the message; report the new count derived, beside its parts.
- [ ] **Step 5: Ratchet before/after with attribution; full gate; commit.**

---

## Task 5: The schedule trigger and backfill — one clock

**Files:** create `src/trigger/{mod.rs,schedule.rs}`; create their tests.

**Nothing in the corpus exercises this.** Seven flows declare a cron and one is
baselined; **zero** are reachable, because `descendants()` keys on `parent_id`
and Kestra leaves it NULL for a `Schedule`-fired execution. Backfill has **zero
live declarations in 345 flows** — two commented-out lines.

- [ ] **Step 1: Read Kestra's `Schedule` on the pinned artifact** — the plugin classes live in per-plugin jars under `/app/plugins`, not `/app/kestra`, and the image ships a JRE with no `javap`: `docker cp` the tree out and use the host JDK. Record the cron dialect, the timezone rule, the late/missed-tick rule and the backfill window semantics, each with the class and method.
- [ ] **Step 2: The clock is injected.** Never `SystemTime::now()` in the trigger path. This programme has lost whole gate runs to wall-clock assertions (`connector_trino_pool.rs` has three), and a scheduler is nothing but a clock.
- [ ] **Step 3: Write the tests first**, driving the injected clock: a tick fires once and only once; a missed window behaves as the artifact says; a backfill over a past window produces the executions the artifact produces, in the order it produces them.
- [ ] **Step 4: Implement.**
- [ ] **Step 5: Mutation** — including one that makes the scheduler report success without creating an execution. If that kills nothing, say so plainly rather than move on.
- [ ] **Step 6: Ratchet (expect 0 movement, say so); full gate; commit.**

---

## Task 6: The flow trigger

**Files:** create `src/trigger/flow.rs`; create its tests.

Seventeen corpus flows declare one, **zero** baselined entries are reachable. Six
of the 26 `not-standalone` corpus flows exist only to be reached through a
trigger and have **no golden entry at all**.

- [ ] **Step 1: Measure the firing condition on the artifact** — which terminal states fire it, how the namespace/flow conditions filter, and what the fired execution's `trigger_execution_id` and `parentId` are. The last is the fact the whole package's link design rests on.
- [ ] **Step 2: Tests first**, including the negative: a terminal state that must **not** fire it, and a condition that must **not** match. Over-acceptance is the risk here and the oracle cannot see it.
- [ ] **Step 3: Implement**, reusing Task 2's link column.
- [ ] **Step 4: Mutation on the filter** — a filter that admits everything must fail a test.
- [ ] **Step 5: Full gate; commit.**

---

## Task 7: The webhook trigger

**Files:** create `src/trigger/webhook.rs`; the HTTP route; create their tests.

Fifteen corpus flows declare one; **zero** reachable, and `harvest.py` **never
calls** `POST /executions/webhook/…` — the string appears once in that file, as
the *reason* `valids/webhook-inputs` is excluded.

- [ ] **Step 1: Measure the artifact's contract** — the URL shape, the key, what the body becomes in the render context, and the response.
- [ ] **Step 2: Authorization is the whole risk.** A webhook is an unauthenticated public surface by construction. Decide how the key is stored and compared, write the negative tests first (wrong key, missing key, key from another workspace), and check the cross-workspace leak class this repo has already been bitten by.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Do NOT populate `trigger.*` into the render context without measuring** — the golden was harvested with **no** trigger context, and populating it would make `valids/webhook-plaintext` diverge further. Measure the ratchet before and after.
- [ ] **Step 5: Mutation on the key comparison.** Report the message.
- [ ] **Step 6: Full gate; commit.**

---

## Task 8: The four corrections and the coverage statement

**Files:** modify `oracle-exec/README.md`; modify `tests/executor.rs`.

- [ ] **Step 1: Correct the three false claims in the record**, each already measured: (a) *"A sub-execution … carries `parentId`"* is **false** for a `Subflow` child — `parentId` is NULL and the link is `trigger_execution_id`; (b) `descendants()` is described as unioning **two independent** discoveries, but the log-scrape arm's regex matches neither the raw JSON body nor the decoded message — **it is dead code, there is one arm**; (c) `ENTRIES_LEFT_UNFINISHED`'s message (done in Task 4 — verify it landed).
- [ ] **Step 2: Write the coverage statement**, separating the 13–15 entries the ratchet speaks for from the four trigger constructs it cannot. State the **mechanism** of the blindness, not just the verdict: a predicate evaluated on every one of the 268 executed flows, at every settle poll, that structurally cannot select a trigger-fired execution.
- [ ] **Step 3: Sweep under formulation 3** — *a figure is safe only if a running program compares it; every other figure, in any file, at any distance from any derivation, is stale until re-derived.* W4 needed five passes and 23 corrections; every earlier formulation was a **locality** rule, and a locality rule cannot be applied to a file nobody has opened. **Sweep the rules as well as the figures** — three of W4's late findings were rules falsified by the work that satisfied them.
- [ ] **Step 4: Full gate; commit.**

---

## Final review

Dispatch the whole-branch review on the most capable model. W4's whole-branch
review found a **Critical that was cross-fix-round, not cross-task** — a defect
reintroduced by a later fix round undoing an earlier one, which no task reviewer
and no scoped re-reviewer was positioned to see. Assume this branch has one too.

The seams here: the link column (three child kinds, one CHECK) against
`driver.rs`'s `spawned` query; the driver publishing the corpus against every
entry's resolution surface; and the trigger half, where **nothing in the ratchet
would notice if it broke**.
