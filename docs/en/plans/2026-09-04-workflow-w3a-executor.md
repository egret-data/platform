# W3a — Executor State Machine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `advance()` — a pure function deciding what an execution does next — reproducing Kestra's flowable semantics, judged by a differential oracle whose corpus is Kestra's own test suite.

**Architecture:** `crates/egret-workflow/src/executor/` holds `Plan`, `ExecutionState`, `Command` and `advance()`, with no I/O, no clock and no database. `crates/egret-workflow/oracle-exec/` holds a pinned Kestra, a harvester that turns real executions into **normalised semantic traces**, the golden, and a ratchet test that fails in both directions.

**Tech Stack:** Rust (the existing `egret-workflow` crate), `kestra/kestra:v2.0.0-rc13` + `postgres:18` via docker compose, Python 3 for the harvester/normaliser (matching `oracle/`'s Java-harness-plus-committed-artifacts shape: the harness runs by hand, its output is committed).

**Spec:** `docs/en/specs/2026-09-04-workflow-w3a-executor-design.md`

## Global Constraints

Every task's requirements implicitly include this section.

- **Kestra is pinned to `kestra/kestra:v2.0.0-rc13`.** Never `latest`. This matches the artifact `oracle/pom.xml` pins (`io.kestra:core:2.0.0-rc13`). A moving tag is a moving golden.
- **The artifact is authoritative for behaviour; the sibling `kestra/` checkout is authoritative only for reading an implementation.** They have drifted. Before writing any sentence beginning "Kestra does", measure it against the running container.
- **Reproduce observable behaviour, not implementation structure.** Upstream defects are copied. Java class names and map iteration order are not. Sub-executions ARE behaviour: the API returns their ids.
- **Comparison is on a normalised SEMANTIC trace, never on Kestra's JSON layout.** Running side by side with a live Kestra is not a goal, so egret's own records are free to differ in shape. Compare: task identity, creation order, state sequences, attempt counts, spawned sub-executions.
- **Over-acceptance is the divergence class that must not ship** — egret answering where Kestra refuses — subject only to exceptions recorded beside the rule in the oracle README, each with its reason.
- **A corpus entry must FAIL before the fix that motivates it.** Show it red, then green.
- **Every fix must be shown to kill a mutation.** Build the mutation first and assert it compiles; a mutation that fails to compile looks exactly like one no test caught. Run the mutation against the WHOLE crate, never a name-filtered subset — a run filtered to the tests you expect to kill can only confirm your expectation.
- **Every recursive descent over author input needs a derived bound**, written as an expression over a measured cost, with the arithmetic in the constant.
- **`executor/` performs no I/O.** No `tokio`, no `sqlx`, no `reqwest`, no `std::time::SystemTime::now()`. `now` arrives as a parameter. A test that needs a database belongs to W3b.
- **Tests:** export `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` and `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`. Never point tests at the dev database.
- **Run the FULL workspace gate before reporting DONE:** `cargo test --workspace --no-fail-fast`. Not `-p egret-workflow`. Cargo unifies features across the set of crates being built, so a crate-only run resolves differently from the shipping one — in W2 this hid a Critical through eight tasks and forty review rounds. Read the exit code from the command itself, never from a pipeline that ends in `tail`.
- **Commits:** `git -c user.email=hoangquocvietbk2.13@gmail.com commit`. Never commit anything under `.superpowers/` or the scratchpad.
- **No auto-merge.** Open the PR and stop.

## File Structure

```
crates/egret-workflow/
  src/executor/
    mod.rs            advance() — the state machine, the only public entry
    plan.rs           Plan, PlanTask, TaskKind, RetryPolicy      (no I/O)
    state.rs          ExecutionState, TaskRun, Attempt, RunState (no I/O)
    command.rs        Command, FailReason                        (no I/O)
    flowable/
      sequential.rs   Sequential + the implicit root task list
      parallel.rs     Parallel
      dag.rs          Dag
      branch.rs       If + Switch (one file: both pick a branch by a rendered value)
      loop_.rs        Loop + SpawnExecution
    retry.rs          retry, timeout, errors: — the attempt lifecycle
  oracle-exec/
    README.md         the rules, in the voice oracle/README.md uses
    docker-compose.yml     pinned image + basic-auth, the only two edits
    harvest.py        stand up Kestra, publish the corpus, run, follow parentId
    normalise.py      trace -> normalised semantic trace
    corpus/           the Kestra flows, copied with provenance
    corpus.json       id -> flow path + provenance + expected disposition
    golden.json       normalised traces from real Kestra
    baseline.txt      the ratchet
    runnable.txt      Task 1's measured result: which flows run standalone, and why the rest do not
  tests/
    executor.rs       the ratchet test + the four failure arms
```

`executor/` is split by flowable because each is an independent semantic unit with its own corpus slice, and because a single `executor.rs` is how `dataapi/src/lifecycle.rs` reached 12,091 lines.

---

## Task 1: The harness, and how much of Kestra's corpus actually runs

**Files:**
- Create: `crates/egret-workflow/oracle-exec/docker-compose.yml`
- Create: `crates/egret-workflow/oracle-exec/harvest.py`
- Create: `crates/egret-workflow/oracle-exec/corpus/` (copied flows)
- Create: `crates/egret-workflow/oracle-exec/corpus.json`
- Create: `crates/egret-workflow/oracle-exec/runnable.txt`
- Create: `crates/egret-workflow/oracle-exec/README.md` (the corpus + provenance sections; the rules sections come in Task 2)

**Interfaces:**
- Produces: `corpus.json` entries of the shape `{"id": "valids/parallel", "path": "corpus/valids/parallel.yaml", "disposition": "runs" | "rejected" | "not-standalone", "reason": "...", "inputs": {...}, "inputs_from": "..."}`. Every later task consumes this.

The spec says the corpus is Kestra's, and that how much of it runs standalone is **a result, not an assumption**. This task produces that number.

- [ ] **Step 1: Pin the compose file**

Copy `kestra/docker-compose.yml` from the sibling checkout and make exactly **two** edits — pin `kestra/kestra:v2.0.0-rc13`, and enable basic-auth, because 2.x refuses unauthenticated API calls.

A third edit, `kestra.encryption.secret-key`, is **allowed but deliberately not taken**, and the README must price it rather than leave it implicit. Without it `valids/input-log-secret` returns `422 Unable to use a SECRET input/output as encryption is not configured`, so the SECRET input type is untestable here. Taking it costs a full re-harvest and recovers exactly one flow, and that flow exercises no W3a flowable — an input's TYPE is W1's concern. W8 or W1 may want it; W3a does not. Every edit is recorded in the README with its reason, and a further edit needs its reason written down before it is made.

```yaml
        kestra:
          server:
            basic-auth:
              enabled: true
              username: admin@kestra.io
              password: Admin1234!
```

Record in the README that these are the only two edits, and why each is needed.

- [ ] **Step 2: Copy the corpus with provenance**

From `../../../kestra/core/src/test/resources/flows/`, copy `valids/*.yaml` AND `valids/*.yml` (261 + 56 = **317** — the two extensions are one corpus; measuring with a `*.yaml` glob silently drops the entire retry matrix) and `invalids/*.yaml` (28) into `oracle-exec/corpus/`. Record in the README the source path and the checkout commit (`git -C ../../../kestra rev-parse HEAD`), so a future reader can tell which Kestra the corpus came from.

- [ ] **Step 3: Write the harvester**

`harvest.py` must:
1. `docker compose -p w3oracle up -d` and wait for `GET /api/v1/main/flows/search` to return 200 **under basic auth** — note the readiness probe must not treat a non-200 as "not ready": an unauthenticated call returns 401 and a wrong path returns 405, and both look like "still starting". Probe the exact authenticated URL.
2. `POST /api/v1/main/flows` with `Content-Type: application/x-yaml` for each corpus flow, recording the HTTP status. A 422 here is the `invalids` disposition, not an error.
3. For each flow that published, `POST /api/v1/main/executions/{namespace}/{id}` and poll `GET /api/v1/main/executions/{eid}` until the state is terminal, with a per-flow timeout. **Terminal is Kestra's own `State.Type.isTerminated()`, not a shorter list you write:** `FAILED, WARNING, SUCCESS, KILLED, CANCELLED, RETRIED, SKIPPED`. An earlier version of this step named only the first four, which mis-files `exit-cancelled` as a timeout. Read the enum from `core/src/main/java/io/kestra/core/models/flows/State.java` and use it verbatim.
4. **Follow `parentId` to harvest sub-executions.** They are hidden from `/executions/search` — a sub-execution fetchable by id and carrying `parentId` was absent from a 200-row listing that returned all 24 top-level executions. A harvester that lists executions loses every loop iteration silently. Discover them from the parent's log lines (`in sub-execution [[link execution="..."]]`) and by querying Kestra's Postgres directly for `value->>'parentId'`.

- [ ] **Step 3b: Recover the inputs — the YAML alone is not the test case**

**53 of the 317 `valids` declare `inputs:`.** The YAML declares that an input
exists; the *value* lives in the Java test beside it, e.g.
`ParallelTest.errors()` drives `flows/valids/finally-parallel.yaml` with
`Map.of("failed", true)`. A bare `POST /executions/{ns}/{id}` either fails or —
worse — runs the flow's other branch and produces a golden that looks fine and
tests nothing.

For each of them, read the Java test that references the flow
(`grep -rl "<flow-name>" core/src/test/java`) and record the inputs it passes
into `corpus.json`:

```json
{"id": "valids/finally-parallel", "inputs": {"failed": true},
 "inputs_from": "core/src/test/java/io/kestra/plugin/core/flow/ParallelTest.java:errors",
 "inputs_alternatives": [{"failed": false}]}
```

`inputs_alternatives` carries the other input maps when several Java tests drive
the same flow with different values — 20 flows do. Keeping only the first would
discard measured coverage.

Where no Java test drives the flow, or the values are computed rather than
literal, mark it `not-standalone` with that reason. Do not invent an input
value: a guessed input produces a golden for a flow nobody tests.

Post the inputs at execution time (Kestra accepts them as multipart form fields
on the execution POST — **verify the exact wire format against the artifact**
before writing the harvester around it).

- [ ] **Step 4: Run it and write down what happened**

Run the harvester over all 345 flows. Classify each into `corpus.json`:

| disposition | meaning |
|---|---|
| `runs` | published and reached a terminal state standalone |
| `rejected` | publish returned 4xx — the `invalids`, and any `valids` this Kestra refuses |
| `not-standalone` | published but could not reach a terminal state without the Java test's setup (needs a plugin this image lacks, a trigger, an input, a mocked worker) |

Write `runnable.txt` with the count in each bucket and, for every `not-standalone` flow, the **specific** reason. Do not round the number up and do not drop a flow because it is inconvenient: this file is the honest denominator for every coverage claim the rest of the package makes.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-workflow/oracle-exec/
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(workflow): W3a Task 1 — Kestra's own corpus, and how much of it runs standalone"
```

---

## Task 2: The normalised trace, the golden, and a ratchet that fails both ways

**Files:**
- Create: `crates/egret-workflow/oracle-exec/normalise.py`
- Create: `crates/egret-workflow/oracle-exec/golden.json`
- Create: `crates/egret-workflow/oracle-exec/baseline.txt`
- Create: `crates/egret-workflow/tests/executor.rs`
- Modify: `crates/egret-workflow/oracle-exec/README.md` (add the rules sections)
- Modify: `crates/egret-workflow/src/lib.rs` (declare `mod executor;`)
- Create: `crates/egret-workflow/src/executor/mod.rs` (a stub `advance()` returning `vec![]`)

**Interfaces:**
- Consumes: Task 1's `corpus.json`.
- Produces: the normalised-trace JSON shape below, which Tasks 3-8 are judged against, and `fn advance(plan, state, now) -> Vec<Command>` as a stub whose signature later tasks fill in.

**Before you build a single golden, read this.** `valids/pause-behavior` is
driven with `behavior: "{{ inputs.behavior }}"`, and on `v2.0.0-rc13` all four
values — `FAIL`, `RESUME`, `WARN`, `CANCEL` — produce the **identical
execution**: `FAILED`, one task run, histories
`CREATED/RUNNING/PAUSED/RESTARTED/RUNNING/FAILED`. Kestra's own
`PauseTest.java:376-380` asserts `RESUME` gives `SUCCESS` with two task runs.

**Do not conclude that rc13 ignores `behavior`.** An earlier version of this
warning said exactly that and it is false. Published with LITERAL values on the
same image, the property works perfectly and matches the checkout:

| flow | state | task runs |
|---|---|---|
| literal `RESUME` | `SUCCESS` | 2 |
| literal `WARN` | `WARNING` | 2 |
| literal `FAIL` | `FAILED` | 1 |
| literal `CANCEL` | `CANCELLED` | 1 |
| omitted | `SUCCESS` | 2 |

What fails is narrower and stranger: **rendering a Pebble expression into a
`Property<Behavior>` fails silently at resume time and takes the task down with
it** — for every value, including the one that would have failed anyway. So that
row's apparent agreement is a coincidence twice over: the flow never reaches the
`FAIL` behaviour, it dies rendering.

Build the golden from what the pinned image did. But record the CAUSE
correctly — "rendering the property from an input fails on this artifact", not
"the property has no effect" — because the second sentence would send whoever
implements `pause` in W8 hunting for a feature that is already there. And more
generally: when a documented input appears to have no observable effect,
**find out why before writing it down**. A wrong reason is how a question gets
closed.

- [ ] **Step 1: Define the normalised trace**

One golden entry per corpus id:

```json
{
  "id": "valids/parallel",
  "execution": ["CREATED", "RUNNING", "SUCCESS"],
  "runs": [
    {"task": "par", "value": null, "parent": null,
     "state": ["CREATED", "RUNNING", "SUCCESS"],
     "attempts": [["CREATED", "RUNNING", "SUCCESS"]]},
    {"task": "p1", "value": null, "parent": 0,
     "state": ["CREATED", "RUNNING", "SUCCESS"],
     "attempts": [["CREATED", "RUNNING", "SUCCESS"]]}
  ],
  "order": [[0, 1, "before"]],
  "spawned": []
}
```

`order` holds only the pairs whose relation was stable across five harvests.

Rules, each traceable to a measurement in the spec:

1. Replace every generated id with its **ordinal in first-appearance order**; `parent` is that ordinal, not a ksuid.
2. **Drop every clock field, but derive the happens-before relation first.**
   `startDate`, `endDate`, `duration`, every `histories[].date`,
   `metadata.originalCreatedDate` are dropped. Durations are dropped, not
   bucketed — a duration bucket is a timing assertion wearing a correctness
   costume.

   **But dropping them outright loses something Kestra's own tests assert.**
   There are **4 ordering assertions, in 2 test files** — `ParallelTest.errors()`
   and `DagTest.errors()` — of the form
   `assertThat(a2.startDate.isAfter(a1.endDate)).isTrue()`. (An earlier version
   said 39 across 12: that was a raw grep for `isAfter|isBefore`, which counts
   every temporal comparison in the suite.) That is not a
   timing claim, it is the claim that `a2` **serialised after** `a1`, which is
   exactly the semantics a `sequential` nested in a `parallel` exists to
   provide. A normaliser that drops all clocks cannot see a flowable that stops
   serialising.

   So before dropping, compute for every ordered pair of runs one of
   `before` / `after` / `overlapping`, and emit the relation as
   `"order": [[i, j, "before"], ...]`.

   The relation is only trustworthy where it is **stable**. Harvest each flow
   **five times** and keep only the pairs whose relation is identical in all
   five; drop the rest and publish the dropped count in `runnable.txt`. This is
   the same discipline the creation-order experiment used: a relation observed
   once is a coincidence, and genuinely parallel branches will flip.
3. **Keep `runs` in Kestra's `taskRunList` order.** It is creation order; §3.3 of the spec earned that with a discriminating experiment.
4. Keep `histories[].state` sequences verbatim.
5. `spawned` holds sub-execution traces of the same shape, ordered by the parent run they belong to.

- [ ] **Step 2: Write the golden**

Run `normalise.py` over Task 1's harvest and commit `golden.json`, restricted to the `runs` and `rejected` dispositions. `not-standalone` flows get a corpus entry with no golden and are never compared — and the README must say the count, because an entry that is silently skipped is worse than one that is honestly excluded.

- [ ] **Step 3: Write the ratchet test — and make each arm fail on purpose first**

`tests/executor.rs`, with `assemble_failures` carrying four arms, exactly as `tests/oracle.rs` does:

```rust
// A panicking state machine is a mismatch in every mode, never a test crash.
// stale:       a baseline line naming an id the corpus no longer holds
// regressions: an id NOT in the baseline whose trace differs
// banked:      an id IN the baseline whose trace now matches — progress that
//              must be banked by deleting that line, in the commit that earned it
```

Before writing the implementation, prove each arm can fail: add a stale baseline line, flip one golden entry, and delete one baseline line for an entry that matches. Each must fail **naming the id**. A ratchet whose arms have never been seen red is a ratchet nobody has tested.

- [ ] **Step 4: Baseline everything**

`advance()` is a stub, so every comparable entry mismatches. Write them all into `baseline.txt` with a header block explaining that this is the starting debt and that each later task banks its slice. Record the count.

- [ ] **Step 5: Run the full gate and commit**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
cargo test --workspace --no-fail-fast
```

Expected: pass. The ratchet is green because everything is baselined.

---

## Task 3: `Plan`, `ExecutionState`, `Command`, and `sequential`

**Files:**
- Create: `crates/egret-workflow/src/executor/plan.rs`, `state.rs`, `command.rs`, `flowable/sequential.rs`
- Modify: `crates/egret-workflow/src/executor/mod.rs`
- Modify: `crates/egret-workflow/oracle-exec/baseline.txt` (bank the sequential slice)

**Interfaces:**
- Produces: every type below. Tasks 4-8 add variants to `TaskKind` and `Command` but must not change these signatures.

- [ ] **Step 1: Write the types**

```rust
// plan.rs — the published, reference-resolved flow. Built once, never mutated.
pub type TaskIx = usize;

pub struct Plan {
    pub tasks: Vec<PlanTask>,   // flattened; children addressed by index
    pub roots: Vec<TaskIx>,     // the flow's top-level `tasks:`
    pub errors: Vec<TaskIx>,    // the flow's top-level `errors:`
}

pub struct PlanTask {
    pub id: String,                  // the author's task id, as written
    pub kind: TaskKind,
    pub retry: Option<RetryPolicy>,
    pub timeout: Option<Duration>,
    pub errors: Vec<TaskIx>,         // this task's own `errors:`
}

pub enum TaskKind {
    Runnable { type_id: String },
    Sequential { tasks: Vec<TaskIx> },
}

pub struct RetryPolicy {
    pub max_attempts: u32,           // Kestra spells it `maxAttempts`, measured
    pub interval: Duration,          // std::time::Duration — a span, not a clock read
    pub max_duration: Option<Duration>,
    pub behavior: RetryBehavior,
    pub warning_on_retry: bool,
}

/// The artifact's `Constant` retry carries six properties. The variants are
/// MEASURED, not guessed: the eight `valids/retry-*.yml` flows Task 1 recovered
/// spell them `RETRY_FAILED_TASK` and `CREATE_NEW_EXECUTION` on the wire, and
/// the four `CREATE_NEW_EXECUTION` flows terminate in `RETRIED` — a state my
/// first draft of the terminal list did not even contain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryBehavior { RetryFailedTask, CreateNewExecution }  // wire: RETRY_FAILED_TASK / CREATE_NEW_EXECUTION
```

Derive `Clone, Debug, PartialEq, Eq` on `Plan`, `PlanTask`, `TaskKind` and
`RetryPolicy`. `Command` needs them too — Task 3's test uses `assert_eq!` on a
`Vec<Command>`, which does not compile without them.

```rust
// state.rs — the snapshot advance() reads. Owned by the caller.
pub type RunIx = usize;

/// The programme spec writes `now: Instant`. It is spelled `Tick` here on
/// purpose: `std::time::Instant` cannot be constructed at an arbitrary value,
/// so every retry and timeout test would need a real clock — in a module whose
/// whole point is that it has none. Milliseconds since the execution was
/// created. This divergence from the spec's illustrative signature is
/// deliberate; record it in the README rather than leaving a reader to wonder.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Tick(pub i64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionState {
    pub runs: Vec<TaskRun>,   // CREATION order — the oracle compares this order
    pub history: Vec<RunState>,
}

impl ExecutionState {
    /// A freshly created execution: no runs, history `[Created]`.
    pub fn new(_plan: &Plan) -> Self {
        Self { runs: Vec::new(), history: vec![RunState::Created] }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRun {
    pub task: TaskIx,
    pub parent: Option<RunIx>,
    pub value: Option<String>,       // a loop iteration's value
    /// The caller's render of this task's expression — an `If` condition or a
    /// `Switch` value. `advance()` is pure and cannot render, so it reads the
    /// result here. `None` means "not rendered yet", which is not an error.
    /// Populated from Task 5 onward; the field exists from Task 3 so no later
    /// task changes this struct's shape.
    pub rendered: Option<String>,
    pub history: Vec<RunState>,
    pub attempts: Vec<Attempt>,
    pub not_before: Option<Tick>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt { pub history: Vec<RunState> }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState { Created, Running, Success, Failed, Retrying, Killed }
```

```rust
// command.rs — what advance() asks the caller to do. The caller does the I/O.
pub enum Command {
    CreateRun { task: TaskIx, parent: Option<RunIx>, value: Option<String> },
    StartRun { run: RunIx },
    ScheduleRetry { run: RunIx, not_before: Instant },
    SetRunState { run: RunIx, state: RunState },
    SetExecutionState { state: RunState },
}
```

- [ ] **Step 2: Write the failing test, and the two helpers every later task reuses**

```rust
// mod.rs
pub fn advance(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command>;

// tests: define these ONCE, here, in `executor/mod.rs`'s `#[cfg(test)] mod tests`.
// Tasks 4-8 use them and must not fork a second copy.
const T0: Tick = Tick(0);
/// Builds a `Plan` from a corpus flow by id, via W1's YAML parser.
fn plan_of(corpus_id: &str) -> Plan;
```

```rust
#[test]
fn a_sequential_runs_its_children_one_at_a_time() {
    let plan = plan_of("valids/sequential");
    let state = ExecutionState::new(&plan);
    let cmds = advance(&plan, &state, T0);
    assert_eq!(cmds, vec![Command::CreateRun { task: 0, parent: None, value: None }]);
}
```

- [ ] **Step 3: Run it and watch it fail**

`cargo test -p egret-workflow executor::` — expected: FAIL, `advance` returns `vec![]`.

- [ ] **Step 4: Implement `sequential` and the root list**

The root task list is a `Sequential` with no `PlanTask` of its own. Implement `advance()` as: find the frontier, emit `CreateRun` for what is not yet created, `StartRun` for what is created and unblocked, and `SetExecutionState` when the frontier is empty.

- [ ] **Step 5: Bank the slice**

Run the ratchet. Every corpus flow whose only flowable is `sequential` (21 flows carry it; the subset using nothing else is smaller — **measure it, do not estimate it**) should now match. Delete exactly those lines from `baseline.txt`. The ratchet fails if you delete a line that still mismatches, and fails if you leave one that now matches.

- [ ] **Step 6: Mutation**

Delete the `StartRun` emission. Assert it compiles, then assert the ratchet goes red naming ids. Restore, re-run, confirm green. Record the mutation and its victims in the README.

- [ ] **Step 7: Full gate, then commit**

---

## Task 4: `parallel` and `dag`

**Files:**
- Create: `crates/egret-workflow/src/executor/flowable/parallel.rs`, `dag.rs`
- Modify: `plan.rs` (add `TaskKind::Parallel`, `TaskKind::Dag`), `mod.rs`, `baseline.txt`

**Interfaces:**
- Consumes: Task 3's types.
- Produces:
```rust
TaskKind::Parallel { tasks: Vec<TaskIx>, concurrency: Option<u32> },
TaskKind::Dag { nodes: Vec<DagNode> },
pub struct DagNode { pub task: TaskIx, pub depends_on: Vec<TaskIx> }
```

- [ ] **Step 1: Write the failing tests**

Two behaviours, both measured, and the second is not obvious:

```rust
#[test]
fn parallel_creates_every_child_before_starting_any() {
    // Kestra's taskRunList is CREATION order, proven by a flow whose branches
    // sleep 4s/2s/1s: the list stayed a_slow, b_mid, c_fast. If advance()
    // created children lazily as earlier ones finished, the order would follow
    // completion and this test is what catches it.
}

#[test]
fn one_invalid_sibling_fails_every_sibling() {
    // Measured on the artifact: a validation error in exactly one parallel
    // branch (c_fast) left a_slow and b_mid FAILED as well.
}
```

- [ ] **Step 2: Run them and watch both fail**
- [ ] **Step 3: Implement `parallel`, then `dag`**

`Dag` is `parallel` with a dependency predicate: a node is startable when every `depends_on` run is `Success`. Reject a cycle at plan-build time, not in `advance()`.

- [ ] **Step 4: Bank, mutate, gate, commit**

The mutation for `dag`: make `depends_on` vacuously satisfied. It must go red naming ids. If it does not, the corpus has no dag flow with a real dependency and the gap must be recorded in `runnable.txt`, not papered over with a hand-written entry that happens to pass.

---

## Task 5: `if` and `switch`

**Files:**
- Create: `crates/egret-workflow/src/executor/flowable/branch.rs`
- Modify: `plan.rs`, `mod.rs`, `baseline.txt`

**Interfaces:**
- Produces:
```rust
TaskKind::If { condition: String, then_: Vec<TaskIx>, else_: Vec<TaskIx> },
TaskKind::Switch { value: String, cases: Vec<(String, Vec<TaskIx>)>, default: Vec<TaskIx> },
```

Both hold an **unrendered** expression. `advance()` is pure and cannot render, so the caller renders and supplies the result in `TaskRun::rendered`, which Task 3 already defined. **This task adds no field to any Task 3 type** — it only adds two `TaskKind` variants and the code that reads a field that was there all along.

- [ ] **Step 1: Write the failing test** — an `If` whose condition rendered `"true"` takes `then_`; one that rendered `"false"` takes `else_`; one with **no** rendered value emits nothing and is not an error, because the render has not happened yet.
- [ ] **Step 2: Run and watch it fail**
- [ ] **Step 3: Implement.** `Switch` picks the case whose key equals the rendered value, else `default`. Kestra's truthiness for `If` is not Rust's: **measure it** against the artifact before choosing a predicate, and record what you measured.
- [ ] **Step 4: Bank, mutate, gate, commit**

---

## Task 6: `loop`, and the command that spawns

**Files:**
- Create: `crates/egret-workflow/src/executor/flowable/loop_.rs`
- Modify: `plan.rs`, `command.rs`, `mod.rs`, `baseline.txt`, `oracle-exec/README.md`

**Interfaces:**
- Produces:
```rust
TaskKind::Loop { values: String, tasks: Vec<TaskIx>, concurrency_limit: u32 },  // default 1
Command::SpawnExecution { task: TaskIx, value: String, parent_run: RunIx },

impl Plan {
    /// True when `task` is inside some `Loop`'s `tasks:`. A loop body never
    /// gets a run in the parent execution, so `advance()` needs to recognise
    /// one to know NOT to create it.
    pub fn is_loop_body(&self, task: TaskIx) -> bool;
}
```
Adding a method is allowed; changing a Task 3 signature is not.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_loop_spawns_one_execution_per_value_and_no_child_run() {
    // Measured: twelve top-level `happy` executions produced TWENTY-TWO
    // sub-executions, while `fail` and `order` produced zero. The loop body's
    // task NEVER appears in the parent's taskRunList — so advance() must emit
    // SpawnExecution and must NOT emit CreateRun for the body.
    let cmds = advance(&plan, &state, T0);
    assert!(cmds.iter().all(|c| !matches!(c, Command::CreateRun { task, .. } if plan.is_loop_body(*task))));
    assert_eq!(cmds.iter().filter(|c| matches!(c, Command::SpawnExecution { .. })).count(), 2);
}
```

- [ ] **Step 2: Run and watch it fail**
- [ ] **Step 3: Implement.** `concurrencyLimit` defaults to 1 (measured from the task's schema): 0 means unlimited, 1 means full serialisation, n means up to n at once.
- [ ] **Step 4: Record the weaker guarantee**

Nothing executes `SpawnExecution` until W3b/W6, so this task asserts the command is **emitted**, and the sub-execution goldens harvested in Task 1 cannot be compared end to end. Write that into `oracle-exec/README.md` as an explicit hole with a count, and mark those golden entries so nobody later reads them as covered. **An uncompared golden that looks compared is worse than no golden.**

- [ ] **Step 5: Bank, mutate, gate, commit**

---

## Task 7: `retry`, `timeout`, and `errors:`

**Files:**
- Create: `crates/egret-workflow/src/executor/retry.rs`
- Modify: `mod.rs`, `state.rs`, `baseline.txt`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_retried_task_has_two_attempts_and_the_execution_passes_through_retrying() {
    // Measured with maxAttempts: 2 — the execution's own history read
    //   CREATED RUNNING FAILED RETRYING RUNNING FAILED RETRYING RUNNING FAILED
    // Kestra's record ALSO showed `boom` as two taskRunList entries. Running
    // side by side with Kestra is not a goal, so egret reproduces the SEMANTICS
    // (two attempts, the execution passing through RETRYING) and not the
    // repeated array element.
}
```

- [ ] **Step 2: Decide the double-entry knowingly**

The spec records this as measured and **not explained**. Before implementing, determine from the artifact whether the duplicate is a semantic fact or a serialisation artefact of Kestra's record — `javap` the record type, or compare against the same execution read from Kestra's Postgres. Write the finding into the README either way. This is a decision to be made on purpose, not by accident.

- [ ] **Step 3: Implement.** `timeout` is a deadline compared against the `now` parameter — `advance()` still reads no clock. `errors:` runs on the failure path, task-level first, then flow-level.
- [ ] **Step 4: Bank, mutate, gate, commit**

---

## Task 8: The `invalids` — refusing what Kestra refuses

**Files:**
- Modify: `crates/egret-workflow/oracle-exec/README.md`, `baseline.txt`, `crates/egret-workflow/tests/executor.rs`
- Modify: whichever of `plan.rs` / `flowable/*` the refusals turn out to need

The 28 `invalids` test the one divergence class the programme forbids shipping. They are last because a refusal is only meaningful once there is something to refuse.

- [ ] **Step 1: Classify the rejections by CAUSE, not by directory**

Task 1 measured 45 rejections, and **only 27 of them are Kestra refusing a
flow**. The other 18 are the pinned image lacking a test-only plugin class, or
the sibling checkout being ahead of `rc13` — the artifact/checkout drift the
Global Constraints warn about, showing up as data. Read `rejected_class` from
`corpus.json`; **never infer "invalid" from the `invalids/` directory name**.
Making egret refuse those 18 would be a defect manufactured out of a naming
convention.

Task 1 also found that `invalids/loop-switch-failed` is **not an invalid flow at
all** — Kestra's own `RunContextTest` executes it and asserts `FAILED`. It is 27
of 28, and that flow belongs to the `runs` side.

Group the genuine refusals by what makes each invalid; the groups are the
implementation units.
- [ ] **Step 2: Write the over-acceptance assertion**

```rust
#[test]
fn egret_refuses_every_flow_kestra_refuses() {
    // Not "does not panic" and not "returns an error" — assert the plan build
    // FAILS, and assert on egret's own message per group, so a refusal that
    // becomes a different refusal is visible rather than silently passing.
}
```

- [ ] **Step 3: Run and watch it fail**, naming how many of the 28 egret currently accepts. That number is the task's starting debt; write it in the README.
- [ ] **Step 4: Implement the refusals** group by group, banking each.
- [ ] **Step 5: Mutation per group** — remove each refusal, assert it compiles, assert red.
- [ ] **Step 6: Write the coverage statement**

The README must close with what this oracle does **not** prove: the corpus is Kestra's own test suite, which is a sample of an infinite space; the ratchet prevents regression but does not bound behaviour on a flow nobody wrote; and `not-standalone` flows were never compared at all. Publish the counts. In the voice `oracle/README.md` uses — a limit stated plainly is worth more than a number rounded up.

- [ ] **Step 7: Full gate, then commit and open the PR**

---

## States and constructs no W3a task claims

Task 3 measured a gap this plan left open, and leaving it implicit is how an
entry ends up permanently baselined with nobody able to say why:

| state | golden runs | produced by | owner |
|---|---|---|---|
| `PAUSED` | 18 | `Pause` | W8 |
| `SKIPPED` | 4 | `disabled:` | **unclaimed — assign before W3a closes** |
| `CANCELLED` | 3 | flow concurrency | W8 |
| — | 18 over-submitted, **22 baselined entries** | `Subflow` | W6 |
| — | 19 over-submitted | `core.execution.*` (`Labels` and 3 siblings, `ExecutionUpdatableTask`) | **unclaimed** |
| — | — | `LoopUntil` / its alias `WaitFor` | W8 (spec §2.1) — absent from this plan, so name it here |

**`Subflow` is the largest block W3a will leave behind, and no W3a task touches
it.** The programme spec assigns it to W6, which is correct — but
`Command::SpawnExecution` is Task 6's, for a **loop iteration**, and that is a
different construct. Anyone reading W3a's residual baseline will otherwise
assume Task 6 covers `Subflow`; it does not.

`LoopUntil` is owned — the spec's §2.1 moved it to W8 and gave the reason — but
its name appears nowhere in this plan, so a reader of W3a's residual baseline has
no way to learn that. Task 6 found it the same way Task 3 found `Subflow`: by
checking what was left over rather than assuming the table was complete.

`core.execution.*` is a family, not a name: `Labels` is one of four
`ExecutionUpdatableTask` implementors — neither a `RunnableTask` nor a flowable.
Six of the entries it accounts for include `valids/labels-update-task-deduplicate`,
which is the entry the oracle README's own circularity demonstration stands on.

`disabled:` belongs to no package in the programme spec. It is a task-level flag,
not a flowable, so it is arguably W1's model or W8's semantics — but it is
currently nobody's, and its four entries would sit baselined forever with no
owner named. The oracle README must carry this table, and W3a's final review must
either assign `disabled:` or record the decision to defer it with a reason.

Two more blockers Task 3 measured that are **not** W3a's to fix, and which the
README must name so a later task does not rediscover them:

- **An over-refusal already on main.** `build_plan` refuses
  `valids/switch-impossible`, which Kestra *ran*: W1's bare `Vec<Task>` fields
  reject an explicit `null` that Jackson accepts. Over-refusal is the mirror of
  the divergence class this programme forbids, and 2 of the 6 banked refusals
  are right only by accident of the same rule. W1's model owns it; Task 8's
  per-group message assertions must not be built on it.
- **Task 7 is blocked on the W1 model, not on the executor type.**
  `models::Sequential` drops Java's `errors` and `finally`, so `PlanTask::errors`
  cannot be populated at all until W1's model carries them.

## Final review

After Task 8, dispatch the whole-branch review on the most capable model. W2's lesson: the whole-branch review found a Critical that eight task reviews could not, because it lived in the seam between two tasks. The seams here are `plan.rs` (every task adds a variant) and `baseline.txt` (every task deletes lines from a file no single task owns).
