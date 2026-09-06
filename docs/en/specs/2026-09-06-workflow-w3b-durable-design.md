# W3b — the durable executor, and the first time the oracle judges a running system

**Status:** design, awaiting review
**Depends on:** W3a (the pure state machine), W1 (model), W2 (expressions)
**Followed by:** W4 (runner & container), W6 (triggers & subflow), W7 (frontend)

## 1. Goal

Make `advance()` run. W3a built a pure function and a differential oracle that
drives it through a test harness; W3b builds the **real** driver — a claim loop
over Postgres, a reaper, and the handlers that actually execute a task — and
then **wires that driver into the same oracle**, so the thing under comparison
stops being a harness and becomes the system that will ship.

## 2. Why the oracle changes, and what that buys

W3a's harness is four tiers of test code: it feeds terminal states from the
golden, it renders expressions itself, and it never spawns anything. Three
consequences were measured and published in `oracle-exec/README.md`:

- **39 entries differ on nothing but `spawned`.** `Command::SpawnExecution` is
  emitted and nothing executes it.
- **227 of the 1104 kept ordered pairs are never asserted**, because egret's
  `spawned` is always empty, so only 877 are.
- The oracle is **circular for exactly one field** — a `Runnable` run's last
  state and each attempt's last — because the harness supplies it.

A real driver closes all three at once. It spawns, so `spawned` becomes
comparable and the 227 pairs come into scope; and it produces terminal states
from a handler that actually ran, so the fed field stops being fed.

**This is the package's real deliverable.** The claim loop is the means.

## 3. The measurement that makes it feasible

Wiring a real executor to the oracle is only worth doing if the executor can run
the corpus. Measured over the **266** comparable `runs` entries:

| non-flowable task type | entries needing it |
|---|---|
| `core.debug.Return` | 115 |
| `core.log.Log` | 82 |
| `core.execution.Fail` | 55 |
| `core.output.OutputValues` | 23 |
| `core.flow.Sleep` | 20 |

**194 of the 266 use only `Log`, `Return`, `Fail` and `Sleep`.** Four trivial
handlers reach three quarters of the comparable corpus end to end.

### 3.1 The programme spec's handler list predates this measurement

§9 of the programme spec gives W3b *"tasks `core.log`, `egret.sql`"*.
`egret.sql` reaches **zero** corpus entries; `core.debug.Return` reaches 115 and
`core.execution.Fail` 55, and neither is named. That list was written before the
corpus existed.

W3b implements the four the oracle needs — `core.log.Log`, `core.debug.Return`,
`core.execution.Fail`, `core.flow.Sleep` — and **`egret.sql` as well**, because
it is the one that makes the feature a product rather than a demo. But
`egret.sql` ships with **no oracle coverage**, and this spec says so rather than
letting a green ratchet imply otherwise. Its correctness rests on its own tests
against a real engine, the way `egret-dataapi`'s does.

`Sleep` is a `RunnableTask` in Kestra despite living under `core.flow`
(`Sleep.java:47`), so it is a handler here too, not a flowable.

## 4. Architecture

### 4.1 The loop is the one `egret-automation` already proved

`crates/egret-automation/src/scheduler.rs` claims with
`UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED LIMIT n) RETURNING …`,
and its own comment explains why. W3b reuses that statement rather than
inventing one.

- **worker loop** — claim `task_runs` where `state = 'pending' AND not_before <= now()`,
  run the handler, and **in the same transaction** record the result, call
  `advance()`, and insert the successors. No window exists in which a task is
  finished and the execution has not moved.
- **reaper** — `working_timeout_s`, as automation does.

### 4.2 `advance()` does not change

W3a's function is already pure over `(plan, state, now)` and its contract is
already written. W3b is a **caller**, and the plan must treat any change to
`advance()` as a finding rather than a step: if the real driver needs the state
machine to differ from what the oracle judged, that is a divergence to measure,
not a refactor to perform.

### 4.3 The driver hands `advance()` states the harness never did

W3a's Task 4 measured this and left it as a warning. The harness applies one
worker result per round, so a `CreateRun` is never emitted while a sibling sits
in `CREATED`; a message-driven runner **does** produce that state, on
`valids/dag`, `valids/if` and `valids/parallel-disabled-tasks`. Kestra has a
guard for it that W3a deleted as unreachable, with the trigger named as *"a
caller that applies a batch partially — W4's worker."* That caller is now W3b's.
Expect it back.

### 4.4 Rendering is the caller's job, and the caller changes

`advance()` cannot render. The harness renders from `corpus.json` plus the flow
source, and reaches `inputs` and `vars`. Kestra's `RunContext` reaches
`outputs`, `taskrun` and `tasksWithState()`. That gap is why W3c's `runIf:` fix
banks nothing and why `loop-outputs-auto` cannot resolve its `values:`. The real
driver has outputs, so **the render surface widens and previously-unreachable
expressions become live** — some of them into entries that are currently
baselined for a different reason. Every such move must be attributed, not
absorbed.

## 5. `SpawnExecution` is W3b's, not W6's

The plan assigned it to W6 with `Subflow`. That is wrong twice over: W3a emits
the command for a **loop iteration**, which is not a subflow, and spawning is a
durable write — the kind of work this package is already building. W6 keeps
`Subflow`'s own semantics; W3b gives the command an executor.

Closing it is what makes the 39 entries and the 227 pairs comparable, so the
plan must report both counts before and after rather than declaring the hole
closed.

## 6. What judges W3b

Three instruments, and they answer different questions.

1. **The existing ratchet, with the real driver behind it.** This is the
   headline. Every entry it banks is one the shipping system reproduces, not one
   a harness reproduced.
2. **Durability properties, which the ratchet cannot see.** Two workers never
   claim the same run; a process killed between the handler and the commit
   leaves no half-applied execution; the reaper reclaims exactly what
   `working_timeout_s` says and nothing else. These need a real Postgres and
   deliberate crashes, not a corpus.
3. **`egret.sql` against a real engine**, with no oracle coverage, stated as
   such.

## 7. Fidelity rules

Unchanged from W3a and worth restating because a running system makes two of
them easier to break:

- Reproduce observable **behaviour**, including upstream defects; not
  implementation structure.
- **Over-acceptance must not ship.** W3a closed two (`disabled:`, the null task
  list) and W3c a third (`runIf:`), and W3c's own fix introduced a fourth,
  narrower one which it then closed. A durable driver has more places to run
  something Kestra did not.
- A guard nobody has seen fail is a guard nobody has seen. Eight shipped or
  nearly shipped across W3a and W3c.
- A count written down beside a derivation is the failure this programme has
  produced eight times. Derive; assert the total beside the parts.

## 8. Risks

1. **The corpus was harvested from a Kestra that ran tasks. egret's handlers are
   not Kestra's.** `Return` renders a format string; `Log` writes a message;
   `Fail` throws. Where the golden's terminal depends on what the task *did*
   rather than that it ran, egret must reproduce the doing. The plan's first
   task should measure how many of the 194 depend on an output value.
2. **Widening the render surface will move entries for reasons unrelated to
   durability**, and those moves will look like W3b's work. Attribute them.
3. **`egret.sql` ships uncovered.** That is a deliberate trade, and the risk is
   that a green ratchet reads as coverage it does not have.

## 9. Deliverables

- `crates/egret-workflow/src/worker/` — the claim loop, the reaper, the handler
  registry, and the five handlers.
- The `SpawnExecution` executor.
- `oracle-exec/` — the same corpus and golden, judged through the real driver.
- Durability tests against a real Postgres.
- A coverage statement that separates what the ratchet proves from what
  `egret.sql`'s own tests prove.
