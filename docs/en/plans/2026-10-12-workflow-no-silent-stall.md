# No silent stall: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** No workflow execution can stall in `RUNNING` for ever without a message.
- Every task egret cannot run is refused at validate, publish and start, either by its construct (§3) or because this deployment does not serve its type (§4).
- Whatever still reaches the worker without a handler ends `FAILED` with a log line saying why (§5).

**Architecture:**
- **Static refusals (§3)** are new errors in `spec::validate_for_run`, which `build_plan` already calls, so every start path refuses them. A corpus guard ties "planned opaque" to "refused".
- **`Served` (§4)** is a pure function of the `workflow` config, installed once at start-up. A pure `spec::unrunnable(flow, &Served)` is called at the five sites of §4.3, and the flow schema filters by it.
- **The worker (§5)** turns `NoHandler` and `NotALeaf` into a `FAILED` outcome, recorded through the existing commit path.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9, utoipa), Postgres. No frontend code.

**Spec:** `docs/{en,vi}/specs/2026-10-12-workflow-no-silent-stall-design.md`. **§n refers to the spec.** The spec's header holds the user's dated rulings.

**A warning about this document.** Code blocks show shapes and test cases to verify against the code, not text to paste. Names were read at `ee8194af`. Where this plan states a fact about the code, the implementer re-reads it first; earlier plans in this programme were wrong about such facts more than once.

## Global Constraints

- **User decisions (spec header, 2026-10-12)**, in effect verbatim:
  - A task planned as opaque that no handler can run is refused at validate and at start.
  - Refusal is per deployment. Whether a deployment serves a type is decided from its configuration by the same logic that builds the worker's handlers.
  - The worker fails what slips through. `NoHandler` and `NotALeaf` end the run `FAILED`, with a system log line; retry and `errors:` apply as for any `FAILED`.
  - The served set is installed once at start-up. `build_plan` and `validate_for_run` stay pure and deployment-free.
- **Exact codes:**
  - `unsupported_allow_failure`
  - `unsupported_working_directory`
  - `unsupported_loop_until_property`
  - `unsupported_flow_task`
  - `unknown_task_type` (now an error)
  - `task_type_not_served`
- **Exact log lines (§5):**
  - `NoHandler`: ``egret cannot run task type `X` on this deployment: no handler is registered for it. This is not an error in the flow.``
  - `NotALeaf`: ``egret cannot run task `id`: it is a control-flow task (`X`) that egret does not execute. This is not an error in the flow.``
- **Never "serve everything" by default.** A process with no installed `Served` must not accept every type.
- **Drafts still save** with these errors; only publish and start refuse.
- **Environment:**
  - Never touch the shared dev DB (:5432), shared buckets, `docker-compose.dev.yml` services, or a kind cluster named `egret`.
  - Database tests run on this branch's own Postgres container (`egret-nss-postgres`, port 5443), created by the controller.
  - Never run two cargo suites at once. Wait while `pgrep -x cargo` is non-empty.
  - Commit before a long suite.
  - Never `git add -A` or `commit -a`; add files by name. Never commit `.superpowers/`.
  - Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Docs:** this package changes behaviour that long doc comments describe. Every doc comment that states the old behaviour is corrected in the same commit as the behaviour, and the report lists each one with `file:line`.

## Gates

- `cargo fmt --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 cargo test -p egret-workflow --no-fail-fast`
- `cargo test -p egret-server`
- `cargo test -p egret-bin` if Task 2 touches it

Count the tests from the full output, not from a `tail`.

---

### Task 1: Static refusals (§3)

**Files:**
- Modify:
  - `crates/egret-workflow/src/spec/validate.rs`: the `TaskKind::AllowFailure`, `TaskKind::WorkingDirectory`, `TaskKind::LoopUntil` and `TaskKind::Leaf` arms of `walk_task`, and the walk itself (nested `errors:`);
  - `crates/egret-workflow/src/executor/mod.rs`: `PUBLISHED_BUT_REFUSED`, `every_other_flowable_stays_opaque`'s neighbours, and the new corpus guard;
  - `crates/egret-workflow/oracle-exec/baseline.txt`, re-measured;
  - doc comments that call these constructs warnings or "publish clean": `plan.rs` (`build_plan`'s doc, `loop_until_gap`, `pause_gap`'s neighbours), `validate.rs`'s module doc and the `KESTRA_FLOW_CONTROL` paragraph, and `oracle-exec/README.md` where it says so.
- Test: `validate.rs` unit tests and `executor/mod.rs` tests.

**Interfaces — Produces:**
- The five codes of §3, as `Finding.code` values in `ValidationReport.errors`.
- A helper `fn opaque_tasks(flow: &Flow, plan: &Plan) -> Vec<(String /*path*/, String /*type*/)>`, or the equivalent in test code, used by the guard.

- [ ] **Step 1: Measure first.** In the report, record:
  - the corpus `valids` and `invalids` flows that contain `AllowFailure`, `WorkingDirectory`, a `LoopUntil`/`WaitFor` with `loop_until_gap(..).is_some()`, an unported control-flow type, or an unknown type, counted from the parsed model rather than from grep;
  - which of them `build_plan` plans today;
  - their current baseline line.

  Also read how `walk_task` and `validate_script_tasks` descend, and state which tasks under `error_children()` `walk_task` misses today.

- [ ] **Step 2: Write the tests (red).**
  1. **One test per code.**
     - Every spelling: the FQCN, the `core.` alias, and mixed case where `canonical_flow_type` folds case.
     - The finding's `path` is the task's path, or the field's path for `unsupported_loop_until_property`, as `unsupported_pause_property` does.
     - The message contains the hint of §3's table.
  2. **Nested positions.** Each construct inside:
     - `tasks:` of a `Sequential`;
     - `errors:` of a `Sequential`;
     - `then:` of an `If`;
     - a `Dag` node;
     - the flow's `finally:`.

     Every one gives the error.
  3. **`build_plan` refuses each.** The `PlanError` message carries the code.
  4. **Drafts still save.** A `routes` test: `PUT /flows/{id}` with an `AllowFailure` answers 200, and its `validation.errors` holds `unsupported_allow_failure`.
  5. **`unknown_task_type` is an error,** and a typo is reported exactly once.
  6. **`not_yet_enforced` stays for flow-level features** (`concurrency`, `sla`, `checks`, `quotas`, triggers): those tests are unchanged.
  7. **The corpus guard.** For every corpus flow that parses: for every task that `build_plan`'s plan would make `TaskKind::Runnable` while `models::TaskKind` is not `Leaf`, `validate_for_run` has an error at that task's path or below it.
     - Because `build_plan` now refuses such flows, compute "planned opaque" with the flattening logic, not through `build_plan`'s result. The implementer chooses how: for example, `flatten_list` without the third gate, exposed `pub(crate)` for tests.
     - Run a mutation that deletes the `unsupported_working_directory` push, and record that the guard fails naming a flow.

- [ ] **Step 3: Implement.**
  - **The errors.** Emit them where the warnings were. `pause_gap` and `loop_until_gap` stay the single predicates.
  - **The descent.** Make `walk_task`'s checks reach `error_children()`, and state in the report what that changes for the other checks in the walk: duplicate ids (`seen_ids`) in particular.
  - **The oracle.** Update `PUBLISHED_BUT_REFUSED`, one row per newly refused `valids` flow, with a `cause` in the style of the `Pause` rows. Re-measure `baseline.txt`. The report lists every line that changed and why; a line that changes for another reason is a defect.

- [ ] **Step 4: Gates, then commit.** `fix(workflow): refuse AllowFailure, WorkingDirectory, expression-bounded LoopUntil and unknown types instead of stalling`

### Task 2: `Served`, `unrunnable`, and the start-up install (§4.1, §4.2)

**Files:**
- Modify:
  - `crates/egret-workflow/src/runtime.rs`: `served_for` beside `handlers_for`, and `install_served`/`served`;
  - `crates/egret-workflow/src/spec/` (a new `unrunnable.rs`, re-exported from `spec`);
  - `crates/egret-bin/src/main.rs`: the install, and the "A flow whose task type is unregistered HANGS" comment, which Task 4 finishes;
  - test support (`tests/common`, the `test-util` feature) with a way for a test to install a `Served`.
- Test: unit tests in `unrunnable.rs` and `runtime.rs`.

**Interfaces — Produces:**

```rust
// runtime.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served(std::collections::BTreeSet<String>);
impl Served {
    pub fn serves(&self, type_id: &str) -> bool;    // exact match, as Handlers::get
    pub fn from_handlers(h: &crate::worker::Handlers) -> Self; // for the drift test
}
pub fn served_for(cfg: &egret_config::WorkflowConfig) -> Served; // pure: no I/O, no runner built
pub fn install_served(s: Served) -> Result<(), Served>;           // OnceLock, like storage::install
pub fn served() -> &'static Served;                               // panics with a clear message if not installed

// spec/unrunnable.rs
pub fn unrunnable(flow: &crate::models::Flow, served: &crate::runtime::Served) -> Vec<crate::spec::Finding>;
```

These are the shapes. If the code argues for a different home (for example `Served` in `worker::registry`), keep the names and say why in the report.

- [ ] **Step 1: Read `handlers_for`, `Handlers::with_core_tasks`, and the `storage`, `namespace`, `kv`, `script` and `kestra` `register` functions.** List in the report the exact keys each `container_runner` setting registers.

- [ ] **Step 2: Write the tests (red).**
  1. **The drift test.** For `container_runner = none` and `docker`, `served_for(cfg) == Served::from_handlers(&handlers_for(cfg, ..))`.
     - `handlers_for` with `docker` needs no daemon to construct (verify). For `kubernetes`, client construction needs a cluster: compare against the keys the `kubernetes` branch would register, by sharing the registration list rather than by building the client. The report states what the `kubernetes` case actually proves.
     - Run a mutation that drops one `kv` key from `served_for`, and record that the test fails.
  2. **`unrunnable`:**
     - with `none`, an `egret.sql` task gives `task_type_not_served` at its path, and so does a `core.http` under a `Sequential`'s `errors:`, under `finally:`, and as a `Dag` node;
     - `io.kestra.plugin.core.log.Log` passes;
     - an unknown type gives nothing (§3 owns it);
     - a script type with `none` gives the runner message, and `egret.sql` gives the no-handler message (§4.2).
  3. **Missing install.** `served()` without an install panics with a message naming `install_served`. There is no permissive default.

- [ ] **Step 3: Implement.**
  - **Install before the router or the worker starts.** `main.rs` builds `Served` from `config.workflow` and installs it before either.
  - **Test processes.** Give them a helper, `install_served_for_tests(Served)` or the equivalent. Because a `OnceLock` is process-wide, document in the report how test binaries that need different sets are arranged: for example one binary per set, or a test-only override behind `cfg(any(test, feature = "test-util"))`.

- [ ] **Step 4: Gates, then commit.** `feat(workflow): Served, the task types this deployment runs, and unrunnable`

### Task 3: The call sites and the schema (§4.3, §4.4)

**Files:**
- Modify:
  - `crates/egret-workflow/src/routes.rs`: `draft_dto` and `validate_flow_source`;
  - `crates/egret-workflow/src/publish.rs`: step 3;
  - `crates/egret-workflow/src/worker/claim.rs`: `start_execution_in_on`;
  - `crates/egret-workflow/src/worker/subflow.rs`: the target's plan;
  - `crates/egret-workflow/src/spec/schema/mod.rs`: `Accepted`, and the schema cache key if it caches per feature set.
- Test: `tests/routes.rs`, `tests/publish.rs`, `tests/scheduler_fire.rs`, `tests/flow_trigger_fire.rs`, `tests/executor.rs` or `tests/end_to_end.rs` for `Subflow`, `tests/flow_schema.rs`. Pick the file each test belongs in by its neighbours.

**Interfaces — Consumes:** `runtime::served()` and `spec::unrunnable` from Task 2.

- [ ] **Step 1: Measure.** Read what a schedule fire and a Flow-trigger fire do today when `start_execution_in_with` returns `Err(InvalidInput)`: whether a born-failed execution is written, whether it is logged, and whether it retries. Record it in the report; this task adds nothing to it (§4.3).

- [ ] **Step 2: Write the tests (red).** All run with `container_runner = none` installed, using a flow with an `egret.sql` task:
  1. `GET /flows/{id}` and `PUT /flows/{id}`: `validation.errors` holds `task_type_not_served`, and the save succeeds.
  2. Both validate routes report it.
  3. Publish answers 400 with the message.
  4. The API start answers 400 and writes no execution row.
  5. A webhook start and a replay of an execution of it are refused the same way.
  6. A schedule fire does what Step 1 recorded, and no execution reaches `RUNNING`.
  7. A `Subflow` whose target uses `egret.sql`: the parent run fails with "cannot execute an invalid flow".
  8. The schema has no `egret.sql`, `core.http`, `core.slack`, `egret.dataapi.call`, `egret.dataset.refresh` or `egret.report.send`, and still has `io.kestra.plugin.core.log.Log`. Its ETag differs from the ETag with a `Served` that includes `egret.sql`, if the test arrangement of Task 2 can produce both.
  9. A served flow still runs to `SUCCESS` (`core.log`).
  10. One mutation per call site (five, plus the schema): remove the call and record the test that fails.

- [ ] **Step 3: Implement.**
  - Append `unrunnable(..)`'s findings to the report's `errors` at the three report sites.
  - At the start chokepoint, refuse with `Error::InvalidInput(format!("the flow cannot run on this deployment: {}", ..))`, joining the messages as `publish` does.
  - At the `Subflow` target, fail the run through the existing `fail_run` with the existing message shape.
  - In the schema, add `served().serves(type)` to `Accepted`.
  - Regenerate `openapi.json` only if a description changed, and then also run `npm run gen:api` from `frontend/`.

- [ ] **Step 4: Gates, then commit.** `feat(workflow): refuse task types this deployment does not serve, at validate, publish, start and Subflow`

### Task 4: The worker fails what slips through (§5)

**Files:**
- Modify:
  - `crates/egret-workflow/src/worker/claim.rs`: `run_one` and `input_for`;
  - `crates/egret-workflow/src/worker/mod.rs`: the `WorkerError::NoHandler` and `NotALeaf` docs;
  - `crates/egret-workflow/src/spec/registry.rs`: the "Not guarding it is still the right call" paragraph;
  - `crates/egret-bin/src/main.rs`: the "HANGS" paragraph;
  - `crates/egret-workflow/oracle-exec/README.md` item 19, the driver's "carries on" comment, and `baseline.txt` if traces change.
- Test: `tests/worker.rs` (or the file that holds `a_worker_whose_lease_was_reaped_cannot_commit`).

**Facts to re-check before starting:**
- `NoHandler` comes from `Registry::run`, inside the handler future. Today it leaves `run_one` through `let outcome = outcome?;`.
- `NotALeaf` comes from `Claimed::input`, called by `input_for` before `run_one` has a `TaskInput`, so before there is a run log handle.
- The errors are `egret_core::Error::Internal(anyhow)` wrapping `WorkerError`, so match them by downcast.

- [ ] **Step 1: Write the tests (red),** on the branch's Postgres:
  1. **`NoHandler`.**
     - A flow with a type registered in `TASK_TYPES` but absent from the test's `Handlers`.
     - Tick the worker: the run ends `FAILED`; its log holds exactly the `NoHandler` line of the Global Constraints, at level `ERROR`; the execution ends `FAILED`; `ended_at` is set on the run, its attempt and the execution.
  2. **With `retry: {type: constant, maxAttempts: 3, interval: PT1S}`:** three attempts, each `FAILED` with the line, then the execution ends `FAILED`.
  3. **With the flow's `errors:`** holding a `core.log` task: it runs.
  4. **An execution that was stalled before.**
     - Claim the run with a worker whose registry lacks the type, under the old behaviour. Simulate this by writing the claimed state the old loop leaves: a lease set, no outcome.
     - Expire the lease, `reap`, tick: the run ends `FAILED`.
  5. **`NotALeaf`.** Insert an execution directly (validation now refuses the flow) whose source holds a `WorkingDirectory`, planned with the old flattening; or construct the claimed row as `claim` would. Tick: the run ends `FAILED`, with the `NotALeaf` line naming the task id and type.
  6. **Unchanged.** `Unplannable` (a stored source that does not parse) still aborts the tick and records nothing.
  7. **Mutations:**
     - revert the `NoHandler` mapping: test 1 fails;
     - revert the `NotALeaf` mapping: test 5 fails.

- [ ] **Step 2: Implement.**
  - **`NoHandler`.** Map `Err(NoHandler)` from the handler future to `Ok(Outcome::failed_with(line))` before the existing `input.log.failure(message)`, so the line is written and the outcome is committed by the existing transaction.
  - **`NotALeaf`.** `input_for` must produce something `run_one` can commit without a leaf spec. Choose the smallest change: for example, `input_for` returns `Prepared::Refused { log, message }`, carrying a `RunLog` built from the claim's identity. Then `run_one` commits `Outcome::failed_with(message)` through the same path. State the choice in the report.
  - **Keep everything else.** `Registry::cancel` and `release` keep refusing on a miss. The server `warn` stays.
  - **Docs.** Correct every doc listed under **Files** in this commit. The report lists each with `file:line`.

- [ ] **Step 3: Run the oracle driver tests and the ratchet.** Record which baseline lines change from a stall to `FAILED`. Each must be an entry that reached `NoHandler` or `NotALeaf`; any other change is a defect.

- [ ] **Step 4: Gates, then commit.** `fix(workflow): a run with no handler or no leaf fails with a message instead of looping for ever`

### Task 5: Live check and docs

**Files:**
- Modify: the spec's status line (EN and VI).
- No new code, unless the live check finds a defect. A fix goes in a separate commit with its own test.

- [ ] **Step 1: Private stack.**
  - Start egret against the branch's Postgres, with its own ports and `container_runner = none`, following `frontend/tests/e2e/README.md`'s private-stack recipe.
  - Never use the shared dev stack or a kind cluster named `egret`.
- [ ] **Step 2: Browser,** driven by Playwright MCP. Say so in the report; it is not an automated test.
  1. A new flow with an `egret.sql` task: the editor's Problems tab shows `task_type_not_served` as an error, and Save succeeds.
  2. Run: the error toast carries "the flow cannot run on this deployment".
  3. A flow with `AllowFailure`: Problems shows `unsupported_allow_failure`.
  4. The editor's `type:` completion does not offer `egret.sql`.
- [ ] **Step 3: The stalled-before-upgrade execution.**
  1. Build `main` (`ee8194af`) in a separate target directory, and run its binary on the same private database.
  2. Start a draft whose `type: egret.sql`, and confirm it stalls: `RUNNING`, with a `warn` per cycle. Shorten `working_timeout` if the config allows it.
  3. Stop it, start this branch's binary, and wait one `working_timeout`.
  4. The execution ends `FAILED`, and W7b's detail page shows the log line.
- [ ] **Step 4:** Update the spec's status line in EN and VI with what was measured, then commit. `docs(workflow): no-silent-stall measured live`
