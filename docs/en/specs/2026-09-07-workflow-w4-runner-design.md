# W4 — the runner, and the three holes W3b handed forward

**Status:** design, awaiting review
**Depends on:** W3b (the durable executor)
**Followed by:** W5 (Kestra plugin host), W9 (KV store, namespace files, internal storage)

## 1. The measurement that reframes this package

The programme spec §9 **gave** W4 *"`TaskCommands` contract; `egret.container`, `egret.python`; log streaming; output files; cancel/kill"* — quoted as it stood before this package measured anything; the row has since been corrected, because **`egret.python` was never W4's to build** (§6.4 gives that name to the notebook `KernelLauncher` and `TASK_TYPES` has held it since W1, with a required `notebook` reference). W4's handler is **`egret.script.python`**. In that order — which reads as a package about running containers.

Measured over the corpus before writing any of it:

- **There is not one script, container, shell or command task type anywhere in the 345 flows.** The only non-`core.*` type is `io.kestra.plugin.fs.http.Trigger`, on two entries, and that is W6's.
- So `egret.container` and `egret.script.python` reach **zero** corpus entries and **zero** golden entries — exactly like `egret.sql`, whose spec §3.1 recorded the same thing after the fact.
- Meanwhile the three holes W3b explicitly handed to W4 touch **20 distinct baselined entries**, with an overlap of one:

| hole W3b handed forward | baselined entries |
|---|---|
| a task-level `timeout:`, which Kestra's **worker** enforces (`WorkerTaskCallable`, and the validator says so) | 4 |
| a `retry:` with a non-zero `interval:`, which today has **no waker outside the oracle** | 12 |
| kill / cancel — `TaskHandler::cancel` exists and **nothing calls it** | 5 |

Plus the four entries that differ on the happens-before relation alone because W3b's driver finishes **one run per tick**, a ruling made in W3b and explicitly handed here.

**So the oracle-visible deliverable of W4 is the holes, not the handlers.** The container runner is product work with its own tests, the way `egret.sql` is. This spec says so at the top rather than letting a green ratchet imply the runner was measured.

## 1.1 The "20 baselined entries" was a union of three declaration greps, and it is ~0

**Correcting §1 rather than editing it.** The table above counts flows that
*declare* a thing. Tasks 1 and 2 measured what is *reachable*, and the answer is
almost nothing:

| hole | declared | reachable | measured by |
|---|---|---|---|
| `timeout:` | 4 | **0** | Task 2 — three are on a `Pause`, and Kestra's worker **ignores a `timeout:` on a flowable** (measured on the artifact: a `Sequential` around a 4 s `Sleep` succeeds in 4.2 s). The fourth is an *hour* on a `core.log.Log`. Instrumented: the budget was applied 4 times across the whole oracle and expired **0** times. |
| `retry:` non-zero `interval:` | 14 (not 12) | **10 could stall, 0 moved** | Task 1 — 4 are `CREATE_NEW_EXECUTION` and never write a `not_before`. And the oracle cannot see the fix at all: `driver::pass_time` **rewinds `created_at` and then calls `worker::resume` itself**, so a due retry is consumed inside the call that made it due and `tick`'s own waker never sees one — the mechanism that makes the oracle fast is what hides both a timing bug and its cure. (W4 Task 6 added the second half of that sentence: as first written it said the rewind happened *before* any deadline could elapse, which is backwards — the rewind is what makes one elapse.) Instrumented: the waker fired **0** times, with a positive control firing on the same build. |
| kill | 5 | **0** | Re-derived off the five flows' own YAML (W4 Task 6 corrected this row: *"two match on the flow id and declare no runnable task at all"* was not what they contain — a `Subflow` is planned, opaquely, as a runnable): one `Exit` carrying `state: KILLED`, an author-declared terminal; one bare `Pause`; two flows declaring `concurrency: {behavior: QUEUE, limit: 1}`; and one parent declaring two `Subflow`s, named for the concurrency limit on its **child**. Four W8 constructs and one output, and **not one of them declares a `timeout:`**, which is the only thing that reaches the kill path. None is a worker killing a running handler. |

**So W4 moves the ratchet by nothing, and that is the honest headline.** The
`RETRYING` waker still closed a run that hung for ever; `timeout:` still reaches
the handler where it used to be a 300 s constant. Those are real defects in the
shipping path. They are simply defects **this oracle has no way to see** — the
same position `egret.sql` and the container runner are in, and the same position
W3c was in when it closed an over-acceptance and banked nothing.

The general lesson, and it is mine: **a `grep` for a keyword counts declarations,
and a package's headline number must count what a code path can reach.** Every
one of the three thirds was wrong in the same direction, so their union was wrong
three times over.

## 2. What the holes actually are

### 2.1 `RETRYING` has no waker, and the sentence saying otherwise is false

`claim.rs` justifies omitting a `not_before` term with *"the delay is `advance()`'s to enforce and it already does"*. Enumerated during W3b's whole-branch review: the claim excludes `RETRYING`; `reap` excludes it; `reopen_due` only fires inside a `settle` whose `now` is fixed for the transaction; and `resume`'s only caller is the oracle driver, reached through `pass_time`, which **rewinds `executions.created_at`** to fake the delay.

So a retry on a run with no concurrently-live sibling sits `RETRYING` **for ever**, silently. **Fourteen** baselined entries declare a non-zero interval — see §1.1; this sentence said *twelve* until Task 1 derived it, and §1.1 correcting the table above did not correct the prose below it.

This is the package's first deliverable, and it is a scheduling problem, not a container problem.

### 2.2 `timeout:` is the worker's, and the worker is W4's

W3a Task 3 established this against the artifact and corrected an earlier brief of mine that had called it `advance()`'s: Kestra enforces `timeout` in `WorkerTaskCallable`, and its own validator warns that the property *"is only relevant for runnable tasks"*. W3b then measured that a task-level `timeout:` is **stripped before a handler sees it**, so `egret.sql`'s statement runs under a 300 s constant unrelated to what the author asked for.

### 2.3 `cancel` refuses, deliberately, so W4 fails loudly

W3b Task 6 did not implement `TaskHandler::cancel` for `egret.sql`. It made it **refuse**, with the reasoning written down: implementing a method nobody invokes produces a guard nobody can see fail, and this package has now produced ten of those. The refusal is a tripwire placed at the one site that must change when the kill path is wired.

**W4 is expected to trip it.** If it does not, that is a finding.

## 3. `TaskCommands`, and the name that must not be reused

The programme spec §6.4 names the trait `TaskHandler` and says explicitly it is *not* `TaskRunner`, because in Kestra that name means a shell-command executor (`TaskRunner.run(RunContext, TaskCommands, filesToDownload)`, whose javadoc says *"will be called by the script plugin"*). W4 introduces the `TaskCommands` contract — the thing Kestra's own `TaskRunner` takes — so this is the package where the two names finally sit side by side and the distinction has to hold.

## 4. What judges W4

Three instruments, and only one of them is the ratchet.

1. **The ratchet, for the holes.** Twenty baselined entries are reachable by fixing scheduling, timeout and cancellation. That is the number to report before and after, with every moved entry attributed — an entry that banks for an unattributed reason is a coincidence, not a bank.
2. **The container runner's own tests.** Like `egret.sql`, and with the lesson W3b learned the hard way: `egret.sql`'s suite reported `7 passed` with **no engine present**, identical to a run with one, until it was made to fail. A suite that self-skips silently is not coverage. If W4's runner needs Docker, a missing Docker must **fail** and a skip must be typed.
3. **The one-run-per-tick decision.** W3b ruled that the tick stays as it is because widening it changes the transaction boundary the durability properties rest on, and handed the decision here on the grounds that W4 introduces a real worker pool. W4 owns it now, and either answer is acceptable **with its cost measured** — four ordered-pair entries on one side, the transaction shape on the other.

## 5. Fidelity rules

Unchanged, and two of them are load-bearing here:

- **Over-acceptance must not ship**, and W3b's whole-branch review found the first *runtime* instances — eleven of them, now in a register. A runner that starts a process Kestra would have killed is the next one.
- **A guard nobody has seen fail is a guard nobody has seen.** Ten across W3a, W3c and W3b. Note W3b also found the inverse: two fences that are *alternatives*, where every single-hunk mutation survives because either one suffices. A redundant guard and a weak one look identical in a mutation table; the difference has to be measured and written down.

## 6. Risks

1. **Nothing in the corpus exercises the runner**, so its correctness rests entirely on tests W4 writes. That is the same trade `egret.sql` took, and the risk is the same: a green ratchet read as coverage it does not have.
2. **Docker in the test path.** `egret.sql` needs StarRocks and W3b made that failure loud rather than silent. A container runner needs a daemon, and the same rule applies — but the cost is higher, because more machines lack Docker than lack a database.
3. **The kill path crosses the lease.** W3b's Critical was a reaped worker committing over its replacement, fixed with a fence on both writers. Cancellation is the same seam approached from the other side: a killed handler must not commit either, and the fence should already refuse it — **verify that rather than assume it**.

## 7. Deliverables

- The `RETRYING` waker, and the corrected sentence in `claim.rs`.
- Task-level `timeout:` reaching the handler, enforced by the worker.
- The kill path, tripping `egret.sql`'s deliberate refusal.
- `TaskCommands`, `egret.container`, `egret.script.python`, with their own tests and a loud failure when the daemon is absent.
- A ruling on one-run-per-tick with its cost measured.
- A coverage statement that separates the twenty entries the ratchet can speak for from the runner it cannot.
