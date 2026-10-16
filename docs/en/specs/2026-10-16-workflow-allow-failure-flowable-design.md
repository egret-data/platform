# AllowFailure flowable: a block that turns a child's failure into a warning

**Status:** design, 2026-10-16. Nothing is built. Read in source from `origin/main` at `a39b36f7`, and from the Kestra checkout `data-portal/kestra` at `96893b589`. The pinned reference image `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd` is the artifact the oracle was recorded against; a Java-level claim below that the checkout and the image might disagree on is marked **verify on the image** and is measured in the plan's first task.

**Depends on:**
- The no-silent-stall package (`docs/en/plans/2026-10-12-workflow-no-silent-stall.md`), which refuses `AllowFailure` at validate as `unsupported_allow_failure` so that a flow holding one stops instead of stalling in `RUNNING`.
- W8 (`docs/en/plans/2026-09-12-workflow-w8-advanced-semantics.md`), which owns the `allowFailure:` **flag** on a task (`executor/allow_failure.rs`), a different construct from this one (§2).
- The oracle (`crates/egret-workflow/oracle-exec/README.md`) and its ratchet (`oracle-exec/baseline.txt`).

## 1. Why

`io.kestra.plugin.core.flow.AllowFailure` is a block of tasks: it runs them in order, and if one of them fails the rest of the block is skipped, the block ends as a **warning**, and the flow goes on with the task after it. Users write it for best-effort sections. Today egret refuses the construct at validate, so a user who writes one gets a validation error on save, and a run that does reach it cannot happen.

Four oracle entries are baselined on this construct: `valids/allow-failure`, `valids/allow-failure-with-retry`, `valids/finally-allowfailure` and `valids/retry-with-flowable-errors`. The last one also needs a retry **on the block itself** (§2, non-goal). The first three are the target of this package.

## 2. Two constructs with one name

| | `allowFailure:` flag | `AllowFailure` flowable |
|---|---|---|
| What it is | a `Task.java` property every task may write | a class `extends Sequential` |
| Where egret has it | `executor/allow_failure.rs`, shipped in W8 Task 5 | not implemented; refused at validate |
| What it does | maps a task's own terminal `FAILED` to `WARNING` (and `WARNING` to `SUCCESS` under `allowWarning:`) | runs a list of children and maps the **block's** outcome |

This package builds the second. It reuses the first's mapping function only where the Java code does (§3.2); it does not change the flag's behaviour.

**Non-goal:** a retry on the block (`retry:` on an `AllowFailure`, as in `valids/retry-with-flowable-errors`). Egret retries a child task (`RetryPending`, `executor/flowable/mod.rs`) but has no measured retry for a flowable. That is a separate row and is left baselined; §7 says what the plan does with it.

## 3. What Kestra does

### 3.1 The block's state

`AllowFailure.resolveState` (`kestra/core/src/main/java/io/kestra/plugin/core/flow/AllowFailure.java`) makes two calls to `FlowableUtils.resolveState`, both with the block's `allowFailure` and `allowWarning` flags (the flowable's own `allowFailure:`/`allowWarning:` fields, default `false`):

1. **With** the `errors:` list: the state of the block as the errors see it. If there is none yet, the block is not terminal and the method returns empty.
2. **Without** the `errors:` list (`normalState`), still with `finally:`: the state of the tasks alone.

If the first call returned a state, and the second is `FAILED`, the block is **`WARNING`**, whatever the first call said. Otherwise the block takes the first call's state.

Consequence, read from the code: when a child fails, the block is `WARNING` whether or not an `errors:` task then succeeds. Its errors list handled the failure but does not make the block `SUCCESS`. `valids/allow-failure` and `valids/finally-allowfailure` are the cases that exercise this.

### 3.2 `guessFinalState` and the flags

`FlowableUtils.resolveState` ends in `Execution.guessFinalState(tasks, parent, allowFailure, allowWarning, terminal)` (`kestra/core/src/main/java/io/kestra/core/models/executions/Execution.java`, from line 760). It picks the highest-priority terminal state of the listed task runs (`KILLED` > `FAILED` > `CANCELLED` > `WARNING` > `PAUSED`), then, when the flags are set, maps `FAILED` to `WARNING` (or `SUCCESS` when `allowWarning` is also set) and `WARNING` to `SUCCESS` under `allowWarning`.

The `WARNING` that `AllowFailure.resolveState` returns for a failed block is a **literal**, not passed through that second mapping. So the block's own `allowWarning:` does not turn it into `SUCCESS`. It does change the result when a child is already `WARNING` (for example a nested `allowFailure:` task), because `guessFinalState` runs on that path. The plan measures this combination on the image rather than trusting the reading.

### 3.3 Siblings, errors and finally

- A failed child stops its siblings in the block, as in a `Sequential`: the block's remaining children are not run.
- `errors:` run once the block has failed, as in a `Sequential`. They do not change the block's `WARNING` (§3.1).
- `finally:` runs after the block is terminal, whatever its state.
- The flow after the block runs. Its state does not inherit the block's `WARNING` as a failure.

## 4. What egret does now

- `spec::validate` reports an `AllowFailure` node as `unsupported_allow_failure` (no-silent-stall Task 1). A draft still saves with that error in `validation.errors` (no-silent-stall plan, Task 4 check). That `build_plan` returns `Err` for such a flow is documented at `executor/plan.rs:527-537`; the plan confirms the publish and run paths against the code, since this note was not traced through them.
- `executor::plan` flattens the node as `TaskKind::Runnable` (opaque), which is what the ratchet counts (the doc at `executor/mod.rs:3331-3345`).
- No `TaskKind` variant exists for it. The corpus holds six `AllowFailure` flows (`executor/mod.rs:4140`); four are baselined (§1).

## 5. Design

### 5.1 A new `TaskKind` variant, driven like `Sequential`

Add `TaskKind::AllowFailure { tasks: Vec<TaskIx> }`, planned by `flatten_flow` from the `AllowFailure` node the way `Sequential` is, with the `errors:` and `finally:` lists carried as they are for every flowable. Its handler is `executor/flowable/allow_failure.rs`, which calls `sequential::drive` for the children, `errors` and `finally`, and changes only the **terminal** state it returns:

| Children (`tasks`) | Errors ran | Block state | Measured on the image (`28f59bf175e1`) |
|---|---|---|---|
| all `SUCCESS` | no | `SUCCESS` | `probe-allsuccess`: block `SUCCESS` |
| a child `FAILED`, no retry pending | either | `WARNING` (§3.1) | errors ran: `allow-failure`, `finally-allowfailure` (both `WARNING`); no errors: `probe-noerrors` `WARNING`; errors that succeed: `probe-errors-ok` `WARNING`; block `allowWarning: true`, child `FAILED`: `probe-failwarn` `WARNING` |
| a child `WARNING` | either | `WARNING`, or `SUCCESS` if the block's `allowWarning:` is set (§3.2) | no `allowWarning`: `probe-combo-nowarn` `WARNING`; `allowWarning: true`: `probe-combo-warn` `SUCCESS` |
| `KILLED` or `CANCELLED` | either | the same as a `Sequential` | **not measured**: no kill was observed (see the measurement note) |

Measured on 2026-10-16 in a throwaway Kestra started from the oracle's `docker-compose.yml` (project `allowfail-probe`, containers `allowfail-probe-*`, removed after). Terminal states of every task run and the execution agree with `oracle-exec/golden.json` for `valids/allow-failure`, `valids/allow-failure-with-retry` and `valids/finally-allowfailure` (the golden's intermediate states were not compared one by one). The sibling after a failed child is not run in any of the measured blocks.

Measurement notes:

- **Retry.** `valids/allow-failure-with-retry` puts `retry:` on the failing *child*, not on the block: the child runs three attempts (`FAILED`, `RETRYING` twice, then `FAILED`), the block is `WARNING`, and `error_handler` and `finally_task` run. Egret's child retry (`RetryPending`) already covers this shape.
- **Block-level retry (non-goal, §2).** `valids/retry-with-flowable-errors` run once on the image: the block carries no attempts of its own; the retry lands on the child `run_script` (two attempts, `FAILED` then `RETRYING` then `SUCCESS`), the block is `SUCCESS`, and `errors:` does not run. The golden records the same shape. So the image applies a block's `retry:` to its failing child; §2's statement that egret has "no measured retry for a flowable" is about Egret's side, not the image's. The row stays baselined and the refusal stays (plan Task 3); this measurement is the evidence for a later package.
- **KILLED / CANCELLED.** The kill route tried (`POST` and `DELETE` on `/api/v1/main/executions/{id}/kill`) answered 405 and 404, and a 60 s `Sleep` finished `SUCCESS` before a kill landed. The row is the design only and is not banked by this package.

### 5.2 Where the change lands

- `models/task.rs`: no change. The node already exists as `TaskKind::AllowFailure(AllowFailure)`.
- `executor/plan.rs`: the new variant; `flatten_flow` maps the node to it.
- `executor/flowable/allow_failure.rs`: new. Its terminal mapping is a pure function, unit-tested as a table.
- `executor/flowable/mod.rs`: dispatches the variant (one arm), next to `Sequential`.
- `spec/validate.rs`: the refusal for `AllowFailure` is removed. `unsupported_allow_failure` stays for nothing.
- `executor/mod.rs`: `build_plan` plans it; the tests that counted it as opaque (`executor/mod.rs:3390-3410`, `3960-3970`, `4140`, `4235`) move with it.

### 5.3 What is not changed

- The `allowFailure:` flag and `executor/allow_failure.rs`: untouched.
- Retry on a block (§2). A flow with `retry:` on an `AllowFailure` keeps a refusal for that property, so it stalls nowhere and is not silently run without its retry. The refusal's code is named in the plan, not invented here.

## 6. Acceptance

1. The three targeted entries are banked: `valids/allow-failure`, `valids/allow-failure-with-retry`, `valids/finally-allowfailure` match the golden trace, and their lines leave `oracle-exec/baseline.txt`.
2. `valids/retry-with-flowable-errors` stays baselined, with its reason updated to name §2's retry non-goal.
3. Every table row in §5.1 has a unit test, and each one is killed by a deliberate mutation (the failed child yields `SUCCESS`; `finally` skipped; errors resetting the block to `SUCCESS`).
4. The validation tests that assert `unsupported_allow_failure` (`executor/mod.rs:3405`, `3968`) are updated to assert the plan, not the refusal; a draft that holds a `retry:` on the block still gets its refusal.
5. The ratchet moves down by exactly the banked entries. No entry is added.
6. Gates: `cargo fmt --check`, `cargo clippy -p egret-workflow --all-targets -- -D warnings`, `cargo test -p egret-workflow`, and the oracle run named in `oracle-exec/README.md`. The DB-backed tests run on a private database, never the shared dev stack.

## 7. Open points the plan settles first

- **Verify each §5.1 row on the image.** The reading in §3 is from source. A row is not implemented until the image agrees.
- **Retry on a flowable.** Egret has no measured retry for a block. The plan measures whether `retry:` on an `AllowFailure` reaches the child, and if it does not, leaves the row baselined with the reason (§6.2) rather than inventing a retry.
- **`allowWarning:` on the block.** §3.2's combination is measured, not assumed.

## 8. Rules this package must meet

- The oracle is the acceptance test. A trace that differs from the image is not "close enough"; the mismatch is written into `baseline.txt` with its cause or the code changes.
- No change to `oracle-exec/golden.json` to match egret's output.
- Docs bilingual: this spec and the plan have a Vietnamese reference in `docs/vi/` (convention, `docs-bilingual-convention`), written after the English is final.

## 9. Not built here

- Retry on a block (§2).
- Any change to the `allowFailure:` flag (W8).
- Child subflow inheritance of labels or of this flowable (unrelated to the flowable's state).
