# W3a — the executor state machine, and the instrument that judges it

**Status:** design, awaiting review
**Depends on:** W1 (model), W2 (expressions)
**Followed by:** W3b (claim loops, reaper, `core.log` / `egret.sql` handlers)

## 1. Goal

`advance()` — the pure function that decides what an execution does next — plus
the flowable semantics it needs, measured against real Kestra rather than
inferred from its source.

```rust
/// No I/O, no DB, no clock. Snapshot in, commands out.
fn advance(plan: &Plan, state: &ExecutionState, now: Instant) -> Vec<Command>
```

W3 is split because the semantic difficulty and the durability difficulty are
different kinds of work and want different reviewers. This half has no I/O at
all. §6.5 of the programme spec already draws this line; W3a is `executor/`,
W3b is `worker/`.

## 2. Scope

**In:** `advance()`; the `Plan`/`ExecutionState`/`Command` types; the flowables
`sequential`, `parallel`, `if`, `switch`, `dag`, `loop`; `retry`, `timeout`,
and `errors:`; and the differential oracle that judges all of it.

**Out:** claim loops, the reaper, task handlers, and anything that touches
Postgres or a network (W3b). `subflow` (W6). `pause`/`resume`, `allowFailure`,
concurrency limits, `WorkingDirectory`, labels (W8).

### 2.1 `loopUntil` is W8, not W3

The programme spec contradicts itself: the package table (§9) lists `loopUntil`
under W3, and §10 lists it under W8 with a reason — it is where Kestra's test
mass lives, and it is additive in the YAML, so deferring it strands no flow
anyone has written. §10 governs. The package table is wrong and should be
corrected when this spec is accepted.

`io.kestra.plugin.core.flow.WaitFor` is an **alias** of `LoopUntil` (measured
from the running artifact's plugin listing). W1's YAML parser must resolve
aliases; whichever package implements `loopUntil` owns both spellings.

### 2.2 `loop` stays in W3a, and `advance()` spawns

Measured, not assumed: Kestra's `Loop` does **not** iterate inside the
execution. Each value produces a separate **sub-execution**. Over the spike's
runs, twelve top-level `happy` executions produced **twenty-two**
sub-executions — two failing runs that stopped at iteration 0, plus ten
successful runs over two values — while the `fail` and `order` flows produced
zero. The loop body's task never appears in the parent's `taskRunList`.

Execution-spawning is W6's machinery. Rather than move `loop` to W6, `advance()`
gains a `Command::SpawnExecution` variant: the pure function models the
semantics completely and stays pure, and *executing* that command is W3b/W6's
problem. The oracle compares the parent trace in W3a; the cross-sub-execution
comparison lands when a real spawner exists.

## 3. The instrument

W2's finding was not that a differential oracle is nice to have. It was that
**every defect that mattered was found by measurement, and none by reasoning** —
six times a test that "obviously" covered a case survived the mutation that
should have killed it. W3a gets the same treatment: a corpus of flows, a golden
harvested from real Kestra, and a ratchet.

### 3.1 What it runs against

`kestra/kestra:v2.0.0-rc13`, pinned. This is the same artifact the W2 oracle
pins (`io.kestra:core:2.0.0-rc13`), and for the same reason: `latest` is a
moving tag and a moving golden is not a golden. The compose file that Kestra
ships must be modified in exactly two ways — pin the image, and enable
basic-auth, because 2.x refuses unauthenticated API calls.

**The artifact is authoritative, the checkout is not.** This rule carries over
from W2 unchanged.

### 3.2 The trace is normalisable — measured, not hoped

The oracle only works if two runs of the same flow produce comparable traces.
They do. Running one flow covering `sequential`, `parallel`, `if`, `switch`,
`dag` and `loop` twice, the two execution documents differ at exactly **18
field paths**, and every one of them falls into two classes:

| class | fields |
|---|---|
| clock | `state.startDate`, `state.endDate`, `state.duration`, every `state.histories[].date`, `metadata.originalCreatedDate`, and the same fields inside `attempts[]` |
| generated id | `id` (execution and task run), `parentTaskRunId`, `originalId`, `labels[].value` |

There were **no** length differences and **no** presence differences. The shape
is stable; only values move.

### 3.3 The order is creation order, and that was measured discriminatingly

Ten runs of the same flow produced **one** distinct `taskRunList` order. That
alone proves nothing — every task in it finished in milliseconds, so a list
ordered by *completion* would have looked identical.

So a second flow was built whose three parallel branches sleep 4s, 2s and 1s:
completion order is the exact reverse of creation order. Five runs, one distinct
order, and it is `par a_slow b_mid c_fast` — **creation order**. A list ordered
by completion would have inverted.

This is the load-bearing assumption of the whole oracle, and this is the
experiment that can falsify it. The first ten runs could not.

### 3.4 Normalisation rules

1. Replace every generated id with its ordinal in first-appearance order.
2. Drop every clock field. Durations are dropped, not bucketed: a duration
   bucket is a timing assertion wearing a correctness costume.
3. Keep `taskRunList` order verbatim — §3.3 earned the right to.
4. Keep `state.histories[].state` sequences verbatim; drop their dates.
5. Follow `parentId` to harvest sub-executions. They are **hidden from
   `/executions/search`** (measured: a sub-execution fetchable by id, carrying
   `parentId`, was absent from a 200-row listing that returned all 24 top-level
   executions). A harvester that lists executions silently loses every loop
   iteration.

### 3.5 The ratchet

Identical to W2's, and for the same reason: `baseline.txt` names the currently
mismatching entries; the test fails both when an entry outside the baseline
mismatches (a regression) and when a baselined entry starts matching without
its line being deleted (unbanked progress).

## 4. What the spike already found that reasoning would not have

Each of these was a wrong guess of mine, corrected by the artifact within
minutes. They are listed because they are the argument for the instrument.

| I wrote | the artifact said |
|---|---|
| `retry.maxAttempt` | `maxAttempts` — and `Constant` carries six properties, including `behavior` and `warningOnRetry` |
| `{{ taskrun.value }}` in a loop body | `{{ item.value }}` |
| `Sleep: PT0S` | *"duration: must be a positive duration"* |
| `Loop` iterates in-execution | one sub-execution per value |

Two more, recorded because they are behaviour W3a must reproduce and neither is
obvious:

- **One bad sibling failed all three.** A validation error in exactly one
  parallel branch (`c_fast`) left `a_slow` and `b_mid` `FAILED` as well.
- **A retried task appears twice in `taskRunList`.** With
  `maxAttempts: 2`, `boom` appears as **two** entries, each holding two
  attempts, while the execution's own history reads `CREATED RUNNING FAILED
  RETRYING RUNNING FAILED RETRYING RUNNING FAILED`. This is measured and **not
  yet explained**; explaining it is a task in the plan, not a footnote here.

## 5. Fidelity rules

Inherited from the programme spec and W2, unchanged:

- Reproduce observable **behaviour**, including upstream defects; do not
  reproduce implementation **structure** (Java class names, map iteration order).
  Sub-executions are behaviour — they have ids the API returns.
- Over-acceptance — egret answering where Kestra refuses — is the divergence
  class that must not ship, subject to the exceptions W2's README enumerates
  beside the rule.
- A corpus entry must **fail before the fix that motivates it**, and the fix
  must be shown to kill a mutation. "I could not construct a mutant that
  escapes" is a statement about the mutants someone thought of.
- Every recursive descent over author input needs a derived bound, stated as an
  expression over a measured cost.

## 6. Risks

1. **The corpus is chosen, not enumerated.** W2 could enumerate its surface —
   63 filters, a finite matrix. Execution interleavings are not finite. The plan
   must say what the corpus covers and, more importantly, what it does not.
2. **`SpawnExecution` cannot be end-to-end verified in W3a.** Nothing executes
   it until W3b/W6. W3a asserts the command is emitted; that is weaker than the
   rest of the oracle and must be labelled as such rather than counted as
   covered.
3. **The retry double-entry (§4) is unexplained.** If it turns out to be a
   Kestra defect rather than a model, copying it is still the rule — but the
   plan must decide knowingly, not by accident.

## 7. Deliverables

- `crates/egret-workflow/src/executor/` — `advance()`, `Plan`, `ExecutionState`,
  `Command`, the six flowables, retry/timeout/`errors:`.
- `crates/egret-workflow/oracle-exec/` — the pinned compose, the corpus, the
  harvester, the normaliser, `golden.json`, `baseline.txt`, and a README that
  carries the rules the way `oracle/README.md` does.
- No change to `worker/`, `repo.rs` or `routes.rs`. W3a ships no I/O.
