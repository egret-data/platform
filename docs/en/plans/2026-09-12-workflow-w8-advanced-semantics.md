# W8 — Advanced Semantics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bank the **18 baselined entries** `Pause`, `LoopUntil`, `allowFailure`
and `sla`'s two assertions can reach, delete a residual-table row that was never
reachable, and record the fifth mechanism by which this oracle is blind.

**Architecture:** `Pause` and `LoopUntil` share one non-terminal state and ship
together. `advance()` gains two `RunState` variants and **stays pure over
`(plan, state, now)`** — no new state field and no input the driver cannot supply,
because every baselined `Pause` bounds itself with `pauseDuration:`/`timeout:` and
`driver::pass_time` already moves the clock with no state term.

**Tech Stack:** Rust, sqlx/Postgres, the `oracle-exec` differential harness, the
pinned image `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`.

## Global Constraints

Every task's requirements implicitly include this section.

- **`advance()` is pure over `(plan, state, now)`** — no I/O, no clock. A change to its **contract** is a **finding**, not a step.
- **`src/executor/` was byte-unchanged from W3a until W6 Task 3 opened it with evidence.** W8 opens it again for two `RunState` variants. The standard is W6 Task 3's: say exactly what the oracle could not otherwise reproduce, and fix any stale doc pointers in that directory **in the same commit that opens it**.
- **Every banked entry must be attributed by a running program**, not by prose. W6's standard: a **one-line mutation** that reverts the construct returns the ratchet to its old value **naming exactly the banked ids**. An entry that banks for an unattributed reason is a **finding**, not a bank.
- **Over-acceptance must not ship**, and the oracle cannot catch it here. A `Pause` that resumes itself where Kestra waits for a human is the next instance.
- **A guard nobody has seen fail is a guard nobody has seen**, and **a two-sided mutation pair on an OPERATOR says nothing about where the OPERAND came from** — W6 found **nine** fields of that shape in one module after being handed four, and seventeen unpinned claims in another. For every field a test asserts, ask what other value the fixture could have supplied, and supply it.
- **A fixture answers the question you built it for and stays silent about the one you then generalise to.** Seven instances in one W6 task. **Before any general sentence, name the element of the class the fixture does not contain and add one.** The method that works: **name the competing rules first, then one discriminating row per rule and one mutation per row.** Re-reading has never caught one.
- **A one-run mutation verdict is a coin flip reported as a measurement.** A W6 mutation reported dead survived **10 of 18**. **Run count belongs beside every verdict**, and **a control on the unmutated binary is what catches a lying harness** — one lied twice.
- **Sweep by concept, not by phrase, and sweep predicates as well as figures.** W6's sweep fixed sixteen stale figures and **could not have found its own Critical, because it swept figures and that was a predicate.** A concept sweep over *"what walks the execution chain"* finds it in one query.
- **A wrong assertion is worse than none** — it converts a divergence into a defended invariant. Two shipped in W6 and each cost a round.
- **A skipped suite must print `0 passed; N ignored`, never `N passed`.** Note `egret-test-db`'s opt-out currently violates this and `egret-query/tests/connector_trino_params.rs` self-skips in 0.01 s — **pre-existing, hand on, do not block**.
- **Reconcile a gate by test-name-set diff, not arithmetic.** A W6 gate read `+2` where `+1` was expected and only a name-set diff could say the extra was a foreign probe; arithmetic would have reconciled and stayed silent.
- **Tests:** export `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`, `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`, `EGRET_TEST_STARROCKS_URL`. Run the **full** `cargo test --workspace --no-fail-fast -j 3`, exit read from the command, **never while another suite runs**. Baseline gate, **re-measured at W8 Task 4 and dated on purpose — every earlier figure in this line was stale within one task of being written**: **259 result lines / 3463 passed / 0 failed / 5 ignored**, exit 0, ratchet **118**, md5 `375c5c88276cc62606205e423a42ea25`. **Re-measured at W8 Task 5 and this line's figures are W8 Task 4's, kept dated:** at `f92f93d5` the gate is **259 result lines / 3479 passed / 0 failed / 5 ignored**, exit 0, ratchet **111**, md5 `d2d3ef3904df2bd8e0d33d947f903227`; the +9 over Task 4's 3470 is **9 test names added and 0 removed** over `src/`, `tests/executor.rs` and `oracle-exec/driver.rs`, derived rather than subtracted. The arc, so a reader can tell a stale number from a wrong one: **3426** (as planned) → **3433** at `c41ecf0b` after Tasks 1–2, ratchet 127 / `a3d887ae72595e61c8bc16e8530f9951` → **3448** at `ed48c5b2` → **3450** at `520783e8` where Task 3 closed, ratchet 119 / `e5164b3fca309af2a11e2208a02d4f29` → **3463** now, +13 for W8 Task 4's tests. **Reconcile a move by test-NAME-SET diff over `src/`, `tests/executor.rs` and `oracle-exec/driver.rs`, never by arithmetic**: Task 4's +13 is 13 names added and 0 removed, and its integration-test name set is byte-identical. `sql_starrocks_live`'s **duration** ≈0.7–0.9 s, not its pass count. **Never write a temporary test into the shared worktree while a gate may be running** — that contaminated a W6 gate.
- **Check for a running gate without naming the string you search for, and exclude your own process tree:** `ps -axo pid,ppid,args | awk -v me=$$ '$1!=me && $2!=me && /cargo/ && /test/' | wc -l`. Five earlier formulations were each wrong in a different way.
- `cp`/`mv` hit `-i` aliases and **refuse while returning success** — `/bin/cp -f` and grep to confirm. Use `if [ -z "$(git status --porcelain)" ]`, never `&& echo CLEAN`. Run `bash -n` on generated scripts. **Commit before you deliberately break something**, and commit early — three agents died mid-task on the previous branch.
- **`baseline.txt` moves only by a bank**, and its md5 must be reported before and after. Read the ratchet with `--nocapture` — a passing ratchet prints nothing.
- Commits use `git -c user.email=hoangquocvietbk2.13@gmail.com`. Never commit `.superpowers/` or a scratchpad. **No PRs** — the controller opens them.

## File Structure

```
crates/egret-workflow/
  src/executor/state.rs        RunState gains Paused (non-terminal) and Restarted
  src/executor/flowable/
    pause.rs                   Pause: the wait, its bound, and resume
    loop_until.rs              LoopUntil / WaitFor — the SAME state as pause.rs
  src/executor/allow_failure.rs  allowFailure / allowWarning terminal mapping
  src/sla.rs                   EXECUTION_ASSERTION only; MAX_DURATION recorded unbankable
  src/worker/claim.rs          due_retries must wake a PAUSED run, not only RETRYING
  oracle-exec/README.md        the concurrency row deleted; the fifth blindness mechanism
```

---

## Task 1: The three corrections, before anything is counted

**Files:** modify `oracle-exec/README.md`; modify `src/executor/plan.rs` (doc only); modify the residual-table derivation in `tests/executor.rs`.

Doing these first makes every later attribution honest, because two of them change what the residual table says W8 is worth.

- [ ] **Step 1: Delete the `concurrency:` row — do not lower it.** Measured: the table says flow `concurrency:` → **`CANCELLED`**, and `CANCELLED` is in **three** golden entries — **two are `sla:`'s, one is `Exit`'s, none is a limit's**. This is the defect W3c fixed by moving `SKIPPED` off `disabled:`: a state attributed to the wrong construct, which makes a row look reachable because the state really is there, for another reason. **Re-derive the three `CANCELLED` entries yourself** before deleting, and record the misattribution as the W3c-shaped defect it is.
- [ ] **Step 2: Record why it can never be reached**, because "0" without a mechanism is the thing this programme keeps correcting: `harvest.py` drives flows **strictly serially**, so `limit: 1` is never contended; **7 of 13 declarers already match**; and the corpus's one genuinely contended population is **already banked by W6**, with its `QUEUE` effect in **no compared field**.
- [ ] **Step 3: `plan.rs`'s opacity doc names `AllowFailure`, `Pause` and `WorkingDirectory` and omits `LoopUntil`**, which is opaque too and stalls nine baselined entries. Fix it, then **sweep by concept for other omissions from that list** rather than adding the one you were handed.
- [x] **Step 4: Settle `allowFailure`'s two owners.** It is W8 in the programme spec §9 and **"unclaimed" in the residual table**, over 12 entries. **The residual table was the loser** — corrected at W8 Task 1, and W8 Task 5 settled the ruling by measurement rather than assertion: all twelve reach the construct and **seven bank inside W8 alone**, the cheapest set in the package. A row whose entries a package can bank is not unclaimed.
- [ ] **Step 5: Full gate; ratchet unchanged at 127; commit.**

---

## Task 2: `RunState::Paused` and `Restarted`, and the executor opening

**Files:** modify `src/executor/state.rs`; modify `src/executor/plan.rs` if planning changes.

**No banks are expected from this task.** It is the state `Pause` and `LoopUntil` both need, and splitting them across packages would block each on the other's variant.

- [ ] **Step 1: Measure what the artifact does**, on the pinned digest — core is in the fat `/app/kestra`, class-file **v69**, so **Corretto 25**; `docker cp` the tree out; own compose project, `down -v`. Record which states a paused execution passes through, whether `PAUSED` is terminal, and what `Restarted` means, each with the class and method.
- [ ] **Step 2: Add the two variants**, keeping `advance()` **pure**. State in the commit exactly what the oracle could not otherwise reproduce, and **fix any stale doc pointers in `src/executor/` in the same commit** — that is the condition W6 Task 3 established.
- [ ] **Step 3: Enumerate every terminal-state predicate in the crate and check each against `Paused`.** W6's `guess_state` is already known to narrow the set from `{FAILED, PAUSED, KILLED, WARNING}` to `{Failed, Killed}`; **the rest have not been enumerated.** *(W8 Task 5 fix round 1: `WARNING` is restored to that set and `PAUSED` is not — a paused child execution has not terminated, so it never reaches the function. The two disjuncts read as one narrowing and are two different facts.)* This is a concept sweep — *"what decides whether a run is finished"* — not a phrase grep.
- [ ] **Step 4: Mutation per predicate**, two-sided, with run counts.
- [ ] **Step 5: Full gate; ratchet **unchanged** at 127, reported explicitly rather than silently; commit.**

---

## Task 3: `Pause` — seven entries (**measured: EIGHT**)

> **Closed.** Ratchet **127 → 119**, `baseline.txt` md5
> `a3d887ae72595e61c8bc16e8530f9951` → `2b0506e3d706a4a9c772103f3206e9d0`
> at `ed48c5b2`, and → **`e5164b3fca309af2a11e2208a02d4f29`** at `520783e8`,
> where Task 3 actually closed: its fix round 3 corrected the DATA in
> `baseline.txt` without moving the count, so the md5 changed and the 119 did
> not. Quoting the first md5 alone is the shape this branch keeps paying for.
> The estimate of seven treated a `timeout:` on a flowable as W4's; that is what
> `FlowService.java:574` says for every non-`RunnableTask` **except `Pause`**,
> so `valids/pause-timeout` banks too. Step 2's question is answered NO: the
> `pauseDuration:` expression reuses `TaskRun::rendered`, so `advance()`'s
> contract is unchanged. Step 3's measurement: the artifact leaves an unbounded
> pause `PAUSED` for the full 90 s of the harvest, and egret does the same.
> See `oracle-exec/README.md`, *What W8 Task 3 had to change, and what it
> banked*.

**Files:** create `src/executor/flowable/pause.rs`; modify the planner.

- [x] **Step 1: All twelve baselined `Pause` entries bound the pause with `pauseDuration:` or `timeout:`** — `TaskRun::attempt_started_at()` and `TaskRun::not_before` already exist for that measurement, and `driver::pass_time` moves the clock to any `not_before_ms` **with no state term**. Verify all three of those before relying on them.
- [x] **Step 2: Two flows need a fourth caller-filled render slot**, because their `pauseDuration:`/`behavior:` is an expression. Identify them and decide whether the slot is this task's or a finding. — They are `valids/pause-duration-from-input` (`pauseDuration: "{{inputs.delay}}"`) and `valids/pause-behavior` (`behavior: "{{inputs.behavior}}"`). **The slot is this task's and it is not a fourth field:** a `Pause`'s `pauseDuration:` is a fourth READER of `TaskRun::rendered`, beside `If`, `Switch` and `Loop`, at a run state already in `advance()`'s table. `advance()`'s contract is unchanged. `behavior:` needs no slot at all — a `Pause` declaring one is planned opaque, so `valids/pause-behavior` does not bank.
- [x] **Step 3: Implement, and refuse the unbounded case if the artifact does** — a `Pause` that resumes itself where Kestra waits for a human is the over-acceptance this task most risks, and **the oracle cannot catch it**.
- [x] **Step 4: Mutation** — including one that resumes a pause the artifact leaves waiting. If it kills nothing, say so plainly. — That mutation (M4) moves the ratchet by **zero** and is killed only by `an_unbounded_pause_never_resumes_itself`, which is the whole reason that test exists.
- [x] **Step 5: Ratchet before/after, every moved entry attributed by a reverting mutation; full gate; commit.** — 127 → 119; M1 reverts `pause_gap` and returns exactly the eight banked ids as regressions.

---

## Task 4: `LoopUntil` / `WaitFor` — three entries → **ONE banked**

**Files:** create `src/executor/flowable/loop_until.rs`.

**Six of the nine have an `OutputValues` child with no handler** — those are not W8's, and the task must report them as the strict/upper-bound separator rather than as failures.

- [x] **Step 0 (added after Task 1 measured it): `WaitFor` does not parse to `LoopUntil`, and the corpus prefers the spelling that does not.** `models::kind_for` matches `Some("loopuntil")` and **`"waitfor"` has no row anywhere in `src/`** — not in `kind_for`, not in `spec::validate`'s `KESTRA_FLOW_CONTROL` — so it falls to **`Leaf`**, which is `Pause`'s tier, not `LoopUntil`'s. Measured over the corpus: **7 flows use `core.flow.WaitFor`, 3 use `core.flow.LoopUntil`**, so the **majority** of the population takes the unparsed spelling. **Nothing pointed at the plan could have seen this**, because both tiers plan `Runnable`, so every prose list saying *"`build_plan` plans `WaitFor` opaque"* stays true. Decide whether `kind_for` gains a row or the two tiers are handled separately — and note this touches `src/models/`, which no earlier W8 task opens. **`kind_for` is not the only table missing the row**: measured, a `WaitFor` child of a `WorkingDirectory` **publishes clean** while the identical `LoopUntil` child is **refused** with `working_directory_child_not_runnable`, because `spec::validate`'s `KESTRA_FLOW_CONTROL` has no `"waitfor"` row either. **That is an over-acceptance at publish**, so "handle the tiers separately" must add that row too — a `kind_for` row alone leaves it open. Bounding the class, by a total census of all **69** distinct corpus `type:` strings through `canonical_flow_type` + `kind_for`: exactly **four** fall to `Leaf` — `Pause` (documented), `Sleep` and `WorkingDirectoryTest$LocalFiles` (both correct), and `WaitFor`. **There is no fifth.** — **Stale twice over, and corrected here rather than only in the task brief.** The `four` became **three** when Task 3 typed `Pause` (re-measured at `88be75b4` over **70** distinct strings), and **two** when Task 4 typed `WaitFor`: `Sleep` and `WorkingDirectoryTest$LocalFiles`, both `RunnableTask` at the pin and both correctly accepted. The class the census was drawn to bound — a Kestra flowable this crate silently accepts as a `WorkingDirectory` child — **is now empty**, and `spec::validate`'s `KESTRA_FLOW_CONTROL` doc holds the same statement beside the table it is about.
- [x] **Step 1: Measure the artifact's loop condition and its interaction with the `PAUSED` state** — the 18-entry `PAUSED` population is 12 `Pause` + 6 `LoopUntil`, so this construct shares Task 2's variant. — Measured from `LoopUntil.java` rather than from the traces, which is what made Step 4 a two-sided test rather than a fit: `reachedMaximums` is **strictly** `iterationCount > maxIterations` (a limit of 3 with a condition that never holds runs **four** iterations, which `valids/waitfor-max-iterations`' golden shows), `childTaskRunExecuted` filters on `isTerminatedNoFail()` and not `isTerminated()`, and the condition is evaluated **only** when no maximum has been reached. The `PAUSED` half is measured from the golden: of its 18 `PAUSED` executions, 12 carry a `RESTARTED` and 6 do not, and the 6 are exactly `LoopUntil`'s — so a resumed loop owes the execution `RUNNING`, not `RESTARTED`.
- [x] **Step 2: Tests first, including the negative** — a condition that must **not** re-enter. — Eleven tests. The negatives are `an_unbounded_loop_until_with_a_falsy_condition_never_stops` (an invented default limit would bank the same entries and ship a different product) and `a_failed_body_ends_a_loop_until_without_asking_the_condition`, which asserts the condition is **never evaluated**, not merely ignored.
- [x] **Step 3: Implement.** — `flowable/loop_until.rs`, plus `Command::RestartLoop` / `Command::RecreateRun` (Kestra's `ExecutionService.retryWaitFor`: the loop's own run keeps its history and loses its cached condition, its descendants are thrown away and rebuilt **one at a time, as the walk reaches them** — a bulk reset gives every sibling one left endpoint and turns three measured `before` pairs into `overlapping`).
- [x] **Step 4: Mutation on the condition**, two-sided. — `if true` and `if false` in place of `!is_truthy(rendered)`: 777/0 control, **774/3** and **771/6**, and on the ratchet `2 broken` and a regression naming `valids/waitfor-nested`. **A two-sided pair on the OPERATOR says nothing about the OPERAND**, so two operand mutations were run beside it: caching the rendered condition across iterations (**773/4**, and the ratchet stops settling at all) and making `iterationCount` 0-based (**774/3**, ratchet regression).
- [x] **Step 5: Ratchet before/after with attribution; full gate; commit.** — **119 → 118**, `e5164b3fca309af2a11e2208a02d4f29` → `375c5c88276cc62606205e423a42ea25`. M2 reverts `descend`'s arm and returns exactly `valids/waitfor-nested`.

> **Closed. ONE bank, not three, and the "three" was a spelling split rather
> than a separator.** Six of the nine are held by the `OutputValues` handler —
> and those six are exactly the `WaitFor`-spelled ones, which is why "3
> `LoopUntil`-spelled" and "3 bankable" looked like the same number. Of the
> other three, `valids/waitfor-nested` banks and the two that do not each differ
> on exactly ONE field for a reason that is not the loop: a Pebble-vs-minijinja
> relational-operator divergence, and a `maxDuration` whose golden iteration
> count carries the artifact's delay-poller latency (its two `PT1S` pauses took
> 1.17 s and 1.92 s). See the design doc's footnote on the `LoopUntil` row and
> `oracle-exec/baseline.txt`'s Task 4 block.
>
> **Step 0's ruling: ONE row, shared.** `LoopUntil.java` carries
> `@Plugin(aliases = "io.kestra.plugin.core.flow.WaitFor")` — an alias on the
> class, not a second class — so both spellings map to one
> `models::TaskKind::LoopUntil`, and that row is also what closes the publish
> over-acceptance (`is_runnable_child` is `matches!(kind, Leaf)`).
> `walk_task` **does** descend through `Task::children()`, which is why a
> `Leaf`-typed `WaitFor` hid its whole subtree from validation.
> **And the ratchet is blind to the whole of Step 0**: deleting the `"waitfor"`
> row leaves it green at 118, because the one entry that banks is spelled
> `LoopUntil` and the six spelled `WaitFor` are held by the handler. Only the
> `spec::validate` tests can see it.

---

## Task 5: `allowFailure` / `allowWarning` — ~~six~~ **seven** entries — **DONE**

**Files:** create `src/executor/allow_failure.rs`.

~~**Two of the six need no new state at all** — `allowWarning:` maps `WARNING → SUCCESS`, which the existing enum already expresses.~~ **Step 1 falsified that sentence twice and it is corrected here rather than left to be read again.** It is **three**, not two — `valids/task-allow-warning-{runnable,flowable,executable-flow}` — and **none of the three is one of the eight** golden entries the residual table derives for this construct, because those eight are by construction the ones that HOLD a `WARNING` symbol. The reason is not the enum either: measured with `javap -p -c` on the pinned digest, `State$Type.fail(Task)` is a CONSTRUCTION out of two booleans with **no source state** (false → FAILED, true+true → SUCCESS, true alone → WARNING, and `isAllowWarning()` unread when `isAllowFailure()` is false), so there is no `WARNING` to map *from*. All three flows write BOTH flags and reach `SUCCESS` in one step. A SECOND spelling — two sequential `if`s in `WorkerTaskProcessor` / `Execution.guessFinalState` / `ExecutableUtils.guessState` — is a real conversion and is the only one that can move a stored `WARNING`.

- [x] **Step 1: Measure the terminal mapping on the artifact**, both directions, with the class and method. → `State$Type.fail(Task)` and the three spelling-B sites; four call sites, one of them `ExecutorService.java:937`'s `Pause` `timeout:`.
- [x] **Step 2: The ~~two~~ three that need no new state**, banked and attributed, before the four that do. Ratchet **118 → 115** with `RunState` untouched (`a4e2dc30`).
- [x] **Step 3: The remaining four**, on `RunState::Warning`. Ratchet **115 → 111** (`1e1c946d`). Migration **0143** widens 0141's `resume_with` CHECK with `TERMINAL:WARNING`.
- [x] **Step 4: Mutation per mapping arm.** 17 mutations; **15 died first run, 2 survived** — `plan.tasks[0]` in `descend` (every corpus declarer puts the flag on task 0) and `Warning` added to `children_executed`'s exclusion (the one flow that reaches it is baselined on a missing handler). Both fixed with discriminating rows (`f92f93d5`).
- [x] **Step 5: Ratchet 118 → 111**, md5 `375c5c88…` → `d2d3ef39…`; full gate **259 / 3479 / 0 / 5, exit 0**; four commits.

**The five that do not bank, each with what holds it:** four declare the `AllowFailure` FLOWABLE, a different construct that is still planned opaque (`valids/allow-failure`, `…-with-retry`, `valids/finally-allowfailure`, `valids/retry-with-flowable-errors`), and `valids/waitfor-child-task-warning` is held by an `io.kestra.plugin.core.output.OutputValues` child with no handler — **measured** by unbaselining it, not inferred: its `WARNING` child is byte-identical to the golden now and the loop stalls one run lower down.

---

## Task 6: `sla:` — two bankable, five unbankable, and the reason

**Files:** create `src/sla.rs`; modify `oracle-exec/README.md`.

- [ ] **Step 1: Only `EXECUTION_ASSERTION` is bankable.** **Five of the seven are unbankable by any implementation whatsoever**, and the mechanism is the fifth way this oracle is blind: **`Sleep::capped(ZERO)` means no run is ever in flight, and `driver::pass_time` only moves the clock to a registered `not_before_ms`** — so a `MAX_DURATION` window **does not exist** in this harness. Not "is not entered" — *does not exist*. **Verify that before building on it.**
- [ ] **Step 2: The cause count exceeds the declaration count.** The table counts **5**; the cause is **7**, because two entries are held by an `sla:` in a **different flow**, which a per-flow needle structurally cannot see. Record that a declaration count and a cause count can differ **in both directions**.
- [ ] **Step 3: Implement `EXECUTION_ASSERTION`; refuse `MAX_DURATION` by name with the measurement.** W1 ports `sla:` as a data shape and says nothing evaluates one — check whether that sentence is still true after this task and correct it if not.
- [ ] **Step 4: Ratchet before/after with attribution; full gate; commit.**

---

## Task 7: The defect the ratchet cannot see, and the coverage statement

**Files:** modify `src/worker/claim.rs`; modify `oracle-exec/README.md`.

- [ ] **Step 1: `claim::due_retries` is state-gated to `RETRYING`/`RETRIED`** and would therefore **never wake a `PAUSED` run that the oracle happily resumes** — the driver reaches the same runs by a different door, so **the ratchet is green either way**. This is W4's item-15 shape: a property proved about a path no deployment takes. Fix it, and **write a test that fails if the gate is narrowed back**.
- [ ] **Step 2: Sweep for other state-gated wakers by concept** — *"what selects a run to resume"* — not by phrase. W6's Critical was exactly this class and its figure sweep could not see it.
- [ ] **Step 3: The coverage statement.** Separate the **18** entries the ratchet speaks for, each with its construct, from what it cannot speak for — and give the **fifth blindness mechanism** (§1.2 of the spec) its place beside the other four. **W8 Task 4 fix round 1 hands you a further row that is NOT a sixth mechanism and must not be filed as one**: `valids/loop-until-restart` is a **latency-dependent golden** — the ratchet speaks and is *wrong*, because the golden's value is a function of the artifact's scheduler rather than of the flow. Its cause is the fifth's (the harness compresses time); its effect is the opposite (a golden entry that exists and that **no implementation can bank**). List it apart, under *entries no implementation can bank*, or the statement files an unbankable entry as a blindness and loses the only actionable fact about it — that there is nothing to fix. `oracle-exec/README.md` carries the wording. State the `concurrency:` deletion as a **correction**, not as lost scope.
- [ ] **Step 4: Sweep the statement under the rule W6 arrived at** — *a figure is safe only if a running program compares it* — **and extend it to predicates**, which is the gap W6's own Critical exposed.
- [ ] **Step 5: Full gate; commit.**

---

## Final review

Dispatch the whole-branch review on the most capable model. W6's found a Critical
that was **the branch's own named failure mode** — a second walk over the
execution chain that repeated a fix made one task earlier and was **not swept
for**. Assume this branch has one too, and **point the reviewer at predicates
rather than figures**: the seams here are the new non-terminal state against every
terminal predicate in the crate, `claim`'s state gates, and the `Pause`/`LoopUntil`
shared variant.
