# W13a — Flow triggers, window-free arm, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A flow declaring an `io.kestra.plugin.core.trigger.Flow` trigger with no `dependsOn` starts when another execution in its workspace reaches one of the trigger's `states`.

**Architecture:** `scheduler::sync` keeps an index `workflow.flow_listeners`. `settle` writes one outbox row per transition that some listener could hear. `flow_trigger::process_events` runs on the W12 scheduler ticker after `fire_due`. For each event it takes one transaction: claim with `SKIP LOCKED`, delete the event, evaluate each listener with W6's pure `trigger::flow::evaluate`, and fire through `start_execution_in`. The depth is persisted (`executions.execution_depth`), so chains run at least one hop per pass with no stack growth (more when a fired execution's first settle already emits an event; spec §4.4 point 4).

**Tech Stack:** Rust, sqlx runtime queries (no compile-time macros in this workspace), tokio, Postgres.

**Spec:** `docs/{en,vi}/specs/2026-09-25-workflow-w13-flow-triggers-design.md` §1–§4, §6, §7.1. If this plan and the spec disagree, stop and report.

**A warning about this document.** Code blocks are **shapes to verify, not text to paste**. On earlier branches, plan prose was wrong 6 times in 24 and a plan's illustrative code shipped 4 errors. **Read the file before editing it. Any disagreement between this plan and the code means the plan is wrong.**

---

## Global Constraints

- **The ratchet cannot judge this package. Expected: 109 before and after.** A flow-fired execution carries no `parent_id` for `harvest.py` to walk (W6 measured 210/210). Report the figure beside the golden-entry count, and report that it would also read 109 with `process_events` deleted.
- **Migration numbers: `0157` for this PR.** `origin/main` ends at `0156`, and the unmerged SP4a branch holds `0154`/`0155`. Re-check both before naming the file. **Never edit a migration that has already been applied anywhere shared.**
- **`src/trigger/` stays pure.** No sqlx, no tokio, no wall clock. `tests::no_wall_clock_in_the_trigger_path` must stay green. `trigger::flow::evaluate`'s semantics do not change; needing them to is a **finding**.
- **`settle`'s hot path gains exactly one statement**, the conditional outbox `INSERT` (§4.3). Nothing else in this PR runs inside the source's transaction.
- **Six off switches**, identical to W12's claim: the flow is deleted; the flow's `state = 'disabled'`; flow YAML `disabled`; trigger `disabled`; the workspace's `workflows` feature is off; the workspace is not `active`. W12 fixed the SQL spelling of the last two in `scheduler/fire.rs`. Reuse it; do not reinvent it.
- **Draft listeners fire** (user decision). `ListeningFlow.draft` is always `false` in egret, and the code says so where it is built.
- **Max depth is `trigger::flow::DEFAULT_MAX_EXECUTION_DEPTH` (100)**, read from the source's persisted `execution_depth`.
- **The gate: measure it at the branch base before Task 2**, with the env below. Reconcile later gates by test-name-set diff.
- **Never run two test suites at once.**
- **Mutation discipline.** Every mutation must show evidence that it applied (`git diff --stat` plus a token count), a run count, an unmutated control, and the assertion that killed it. A mutation that fails to compile prints no `test result:` line. A surviving mutant is reported as unproved.
- **A zero is reported beside a number that can disagree. A sweep is reported as N examined / M corrected.** Use `/usr/bin/grep` with file arguments, not a piped `rtk grep`.
- **Commits:** Conventional Commits, `git -c user.email=hoangquocvietbk2.13@gmail.com`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Commit before deliberately breaking anything. Never commit `.superpowers/`. Never `git checkout` a commit; check `git branch --show-current` before every commit. **No PRs**; the controller opens them.

**Environment (the controller sets these up):** a dedicated Postgres for this branch on its own port, so it cannot share `egret_test_template` with another branch. Use `DATABASE_URL` pointing at it, plus `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`, `EGRET_TEST_STARROCKS_URL=mysql://root@localhost:9030`, and `CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target`. The target dir is shared and I/O can stall, so run long cargo commands in the background and poll them.

## File Structure

```
migrations/0157_workflow_flow_triggers.sql   NEW — flow_listeners, flow_trigger_events,
                                             executions.execution_depth, one-per-event index
crates/egret-workflow/src/
  scheduler/sync.rs          + flow-trigger rows written beside Schedule rows
  flow_trigger/mod.rs        NEW — module doc: the per-statement safety section
  flow_trigger/process.rs    NEW — process_events(pool) -> EventReport
  lib.rs                     + pub mod flow_trigger
  worker/claim.rs            NewExecution gains trigger_execution_id + execution_depth;
                             settle's was!=is block gains the outbox INSERT
  worker/render.rs           + pub fn execution_render_context (the source's context)
  worker/mod.rs              re-exports
  runtime.rs                 spawn_scheduler calls process_events after fire_due
crates/egret-workflow/tests/
  flow_trigger_sync.rs       NEW — Task 2
  flow_trigger_fire.rs       NEW — Task 4
  runtime.rs                 + the end-to-end chain test (Task 4)
crates/egret-workflow/oracle-exec/
  w13-flow-trigger.json      NEW — Task 1's measurement
  README.md                  + W13a coverage section (Task 5)
deploy/helm/egret/templates/configmap-env.yaml   comment beside schedulerEnabled (Task 4)
```

---

## Task 1: Measure what Kestra stores for a Flow-fired execution

**Files:** create `crates/egret-workflow/oracle-exec/w13-flow-trigger.json`. Modify spec §4.1/§4.5 in both languages **only if** the measurement contradicts them.

**Interfaces:** Produces the exact `execution.trigger` object and the `executionDepth` field for a flow-fired execution. Task 4's `flow_trigger_record` reproduces it from this file.

- [ ] **Step 1: Start the pinned artifact.** Run `cd crates/egret-workflow/oracle-exec && docker compose up -d`. Read `harvest.py`'s `BASE`/`_auth()` for the API base (tenant-qualified `/api/v1/main/...` on this build) and the credentials. Check that port 8080 is free first.
- [ ] **Step 2: Create two flows and a chain.** Use namespace `w13` and `POST /api/v1/main/flows` with `Content-Type: application/x-yaml`:

```yaml
id: producer
namespace: w13
tasks:
  - id: a
    type: io.kestra.plugin.core.log.Log
    message: hi
```

```yaml
id: listener
namespace: w13
triggers:
  - id: on_done
    type: io.kestra.plugin.core.trigger.Flow
    states: [SUCCESS]
    inputs:
      src: "{{ trigger.executionId }}"
inputs:
  - id: src
    type: STRING
tasks:
  - id: a
    type: io.kestra.plugin.core.log.Log
    message: "{{ inputs.src }}"
```

Add a third flow, `listener2`, that listens to `listener`, so there is a two-hop chain.
- [ ] **Step 3:** Start `producer` by hand. Wait until `listener` and `listener2` executions exist. Fetch all three with `GET /api/v1/main/executions/{id}`.
- [ ] **Step 4: Save verbatim.** For each execution save: `trigger`, `labels`, `parentId`, any depth field (search the JSON for `depth`), `inputs`, and the state history. Record the image digest and the capture time.
- [ ] **Step 5: Findings.** Write each answer with the value it came from quoted:
  - the full key set of `trigger.variables`;
  - whether `state` is the bare enum name (e.g. `"SUCCESS"`). **The unique index in spec §4.1 depends on this answer**;
  - where the execution depth lives and its value at hop 1 and hop 2;
  - whether `inputs` rendered `trigger.executionId`.
- [ ] **Step 6:** Run `docker compose down -v` and confirm no `oracle-exec` container remains.
- [ ] **Step 7: Commit:** `docs(workflow): W13a Task 1 — what Kestra stores for a Flow-fired execution`.

---

## Task 2: Migration 0157, and `sync` writes listener rows

**Files:** create `migrations/0157_workflow_flow_triggers.sql`; modify `crates/egret-workflow/src/scheduler/sync.rs`; create `crates/egret-workflow/tests/flow_trigger_sync.rs`.

**Interfaces:**
- Consumes: `scheduler::sync(conn, &FlowRow, gone) -> Result<()>` (W12). It is already called by `FlowRepo::create/update/soft_delete` and `scheduler::reconcile`. Also `trigger::flow::WindowFreeFlowTrigger::resolve(&Trigger) -> Result<_, FlowTriggerError>`, `models::{TriggerKind::Flow, FlowTrigger { states, depends_on, .. }}`.
- Produces: the tables below; listener rows kept in step by `sync`.

- [ ] **Step 1: The migration.** Use spec §4.1 verbatim (including `done text[] NOT NULL DEFAULT '{}'` and `attempts integer NOT NULL DEFAULT 0` on `flow_trigger_events`), with a header comment naming the spec and explaining the number (SP4a holds 0154/0155). If Task 1 found that `state` is not the bare enum name, replace the unique index's `(execution_trigger->'variables'->>'state')` term with a typed column `trigger_source_state text` plus a CHECK tying it to the Flow type, as W12 did with `scheduled_date`. Say which you chose.
- [ ] **Step 2: Failing tests** in `tests/flow_trigger_sync.rs`. Go through `FlowRepo`, not raw SQL:
  - `a_flow_trigger_gets_one_listener_row_with_its_states`: `states: [SUCCESS, FAILED]` → row with `{SUCCESS,FAILED}`, `depends_on = false`, `disabled = false`.
  - `default_states_are_the_terminated_states_and_paused`: no `states` key → exactly `models::FlowTrigger`'s default list, `PAUSED` included.
  - `a_depends_on_trigger_gets_a_row_marked_depends_on`.
  - `removing_the_trigger_or_deleting_the_flow_deletes_the_row`.
  - `flow_or_trigger_disabled_sets_the_flag`.
  - `schedule_and_flow_triggers_coexist_in_one_flow`: one flow with both kinds → one `triggers` row and one `flow_listeners` row.
  - `reconcile_fills_listener_rows_for_a_flow_inserted_by_raw_sql`.
  - Extend W12's writer scanner test so it still finds exactly four writers. It needs no change, but assert that it still passes.
- [ ] **Step 3: Implement.**
  - In `sync`, collect `TriggerKind::Flow` triggers beside the schedules.
  - `DELETE FROM workflow.flow_listeners WHERE flow_id = $1 AND NOT (trigger_id = ANY($2))`.
  - Upsert each row with `states` as `text[]` of Kestra's enum names, `depends_on = !depends_on.is_empty()`, and `disabled = flow.disabled || trigger.disabled`.
  - A trigger whose `resolve` fails for a reason other than `UnsupportedDependsOn` gets **no row**, and a `tracing::warn!` names the flow and the reason.
  - `dependsOn` triggers get a row even though `resolve` refuses them. Put the `depends_on` check before `resolve`.
- [ ] **Step 4: Run** `cargo test -p egret-workflow --test flow_trigger_sync --test scheduler_sync --test repo`. Expect PASS.
- [ ] **Step 5: Mutations:**

| mutation | must kill |
|---|---|
| drop the flow-trigger branch in `sync` | `a_flow_trigger_gets_one_listener_row_with_its_states` |
| drop the listener `DELETE` | `removing_the_trigger_or_deleting_the_flow_deletes_the_row` |
| `disabled = trigger.disabled` only | the flow half of `flow_or_trigger_disabled_sets_the_flag` |
| write `states` from the YAML without defaults | `default_states_are_the_terminated_states_and_paused` |

- [ ] **Step 6: Full crate gate; commit:** `feat(workflow): W13a — flow_listeners and the outbox schema`.

---

## Task 3: Link and depth on `NewExecution`, the source's render context, and the `settle` hook

**Files:** modify `crates/egret-workflow/src/worker/claim.rs`, `worker/render.rs`, `worker/mod.rs`; tests in `crates/egret-workflow/tests/worker.rs`.

**Interfaces:**
- Produces:

```rust
pub struct NewExecution<'a> {
    // existing fields unchanged ...
    pub trigger_execution_id: Option<Uuid>,   // NEW
    pub execution_depth: i32,                 // NEW; 0 for every existing caller
}

/// The render context `when`/`inputs` of a Flow trigger are evaluated in:
/// the SOURCE execution's, as `runContextFactory.of(null, execution)`.
/// Reads without locking the execution.
pub async fn execution_render_context(conn: &mut PgConnection, execution_id: Uuid)
    -> Result<RenderContext>;
```

- `execution_render_context` is `load_with(conn, id, Lock::No, Stack::root())`, then `Surface::load(conn, &ctx)`, then `surface.context(&ctx.plan, &state, None)`. Verify each name against the code.
- The hook: in `settle`'s `was != is` block, for every new state except `CREATED`, run spec §4.3's `INSERT … WHERE EXISTS`. It is placed before `notifies_parent`'s terminal-only branch, not inside it.

- [ ] **Step 1: Failing tests:**
  - `a_start_writes_trigger_execution_id_and_depth`: via `start_execution_in`.
  - `every_existing_caller_writes_depth_zero`: a webhook start and an API start.
  - `a_transition_writes_one_event_when_a_listener_can_hear_it`: listener `states: [SUCCESS]`, one-task flow driven to SUCCESS → exactly one event with `state = 'SUCCESS'`. The `RUNNING` transition writes none.
  - `paused_writes_an_event_for_default_states`: a flow with a bounded `Pause` task and a default-states listener.
  - `no_listener_in_the_workspace_writes_no_event`: plus a listener in **another** workspace, as the control.
  - `a_rolled_back_transition_leaves_no_event`.
  - `execution_render_context_renders_the_sources_inputs_and_outputs`: render `{{ inputs.x }}` and `{{ outputs.a.value }}` against a finished source.
- [ ] **Step 2: Implement.** Add the fields and bind `trigger_execution_id` and `execution_depth` in `start_execution_in`'s INSERT. Every existing `NewExecution { .. }` literal gains `trigger_execution_id: None, execution_depth: 0`; list them with a grep and report the count. Add the hook and the render helper.
- [ ] **Step 3: Run** `--test worker --test webhook_route --test executions_routes --test scheduler_fire --test runtime`. Expect PASS.
- [ ] **Step 4: Mutations:**

| mutation | must kill |
|---|---|
| hook only on `notifies_parent` states | `paused_writes_an_event_for_default_states` |
| drop the `EXISTS` | `no_listener_in_the_workspace_writes_no_event` |
| drop the `workspace_id = $2` term inside `EXISTS` | the other-workspace control |
| don't bind `execution_depth` | `a_start_writes_trigger_execution_id_and_depth` |

- [ ] **Step 5: Full crate gate; commit:** `feat(workflow): W13a — execution depth, the source's render context, and the outbox hook in settle`.

---

## Task 4: `process_events`: fire the window-free arm

**Files:** create `crates/egret-workflow/src/flow_trigger/{mod.rs,process.rs}`; modify `lib.rs`, `runtime.rs`, `deploy/helm/egret/templates/configmap-env.yaml` (comment only); create `crates/egret-workflow/tests/flow_trigger_fire.rs`; extend `tests/runtime.rs`; helpers in `tests/common/mod.rs`.

**Interfaces:**
- Consumes: Tasks 2–3; `trigger::flow::{WindowFreeFlowTrigger, ListeningFlow, SourceExecution, WhenVerdict, evaluate, Outcome, FiredExecution, DEFAULT_MAX_EXECUTION_DEPTH}`; `runners::render_str(&str, &RenderContext)`; W12 `scheduler/fire.rs`'s off-switch SQL and its error classification (`classify`: `InvalidInput` → refused; the unique violation → already fired; anything else → retried).
- Produces:

```rust
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EventReport { pub events: usize, pub fired: usize, pub refused: usize, pub errored: usize }
pub async fn process_events(pool: &PgPool) -> egret_core::Result<EventReport>;
```

- [ ] **Step 1: Test helpers** in `tests/common/mod.rs`:
  - `drive_to(db, execution_id, state)`: finish a one-task execution through the real worker tick, with a registry that answers `core.log.Log`. Read `tests/runtime.rs`'s `Recorder` for the pattern.
  - `events(db) -> Vec<(Uuid, String)>`.
  - `fired_by(db, source) -> Vec<(Uuid /*flow*/, i32 /*depth*/, Value /*trigger*/)>`.
  - The tests in this file share one DB per process and a pass is global. Serialise them with a static mutex and clear `flow_trigger_events` at the start of each, as `tests/scheduler_fire.rs` does.
- [ ] **Step 2: Failing tests** in `tests/flow_trigger_fire.rs`, one per spec §7.1 row this task owns:
  - `a_success_fires_the_listener_once`: one execution. `trigger_execution_id` = the source, `execution_depth` = 1, `execution_trigger` equals Task 1's shape (loaded with `include_str!`), and `inputs` equal the listener's literal inputs. The event is gone afterwards.
  - `inputs_that_fail_to_render_create_a_failed_execution` (**amended in Task 4 fix round 1**; this line first said the fire is refused, which misread the measurement): in Task 1 (`oracle-exec/w13-flow-trigger.json`, `listener_run1_FAILED_bad_inputs`) Kestra fails `inputs: {src: "{{ trigger.executionId }}"}` with `Unable to find \`trigger\`` and STILL creates the listener's execution, `CREATED -> FAILED`, no task run, no `inputs`. egret does the same (spec §4.5). Assert that execution, a sibling with literal inputs firing, and the event consumed.
  - `refusals`: same flow; other workspace; `kind` not normal (write the source's kind column by SQL); `when: "{{ false }}"`; `when` that cannot render. Each gives zero executions, and the event is consumed.
  - `every_off_switch`: all six, each with an unaffected sibling listener that does fire.
  - `a_chain_runs_one_hop_per_pass`: A→B→C. Pass 1 fires B at depth 1; pass 2 fires C at depth 2.
  - `a_cycle_stops_at_depth_100`: A↔B. Set the source's `execution_depth` to 99 by SQL: one more fire happens at 100, and the next is refused with `DepthExceeded`.
  - `a_broken_listener_does_not_stop_its_siblings`: one listener's YAML corrupted by SQL, one sibling → the sibling fires, `errored`/`refused` counted, the event consumed.
  - `a_transient_failure_keeps_the_event_for_that_listener_only`: a BEFORE INSERT fault on `workflow.executions` scoped to one of two listener flows, the same pattern as `scheduler_fire.rs`. The healthy sibling fires in pass 1 and is not fired again in pass 2. The failing listener does not fire, the event remains with `attempts = 1` and the sibling in `done`. The fault removed, pass 2 fires only the failing listener and deletes the event.
  - `a_listener_that_keeps_failing_is_given_up_after_five_attempts`: the event is deleted on the 5th attempt and a `warn` is logged.
  - `fire_and_delete_are_atomic`: a fault after the fire, before COMMIT → no execution, the event remains.
  - `a_draft_listener_fires`.
  - `a_depends_on_listener_is_skipped_with_a_named_reason`: the log/`refused` count names W13b.
- [ ] **Step 3: Implement `process_events`** per spec §4.4:
  - The claim is `DELETE … WHERE id = (SELECT … FOR UPDATE SKIP LOCKED) RETURNING`.
  - Listeners are selected with the six off switches in SQL.
  - Each listener runs in a savepoint. `when` and `inputs` are rendered with `render_str` against `execution_render_context(source)`. A render error becomes `WhenVerdict::Unrenderable`, and an absent `when` becomes `WhenVerdict::NotDeclared` exactly as `evaluate` expects.
  - `evaluate` is called with `ListeningFlow { draft: false, .. }` (comment: the user decision).
  - A fire goes through `start_execution_in` with `trigger_execution_id` and `execution_depth`.
  - `flow_trigger_record(&FiredExecution, &source) -> Value` reproduces Task 1's shape. Its unit test compares against that file, not against a literal.
  - Error classification reuses W12's `classify`. Move it to a shared place if needed, and do not copy it.
  - **Per-listener retry, not per-event.** The event row carries `done text[]` (keys `"<flow_uuid>/<trigger_id>"` of listeners already handled) and `attempts int`, both in Task 2's migration.
    - A listener that fires, is refused, or hits a deterministic error (`InvalidInput`, or the unique index = already fired) is added to `done`.
    - A listener that hits a retried error rolls back **only its savepoint**, so siblings' fires commit.
    - At the end of the transaction: if every heard listener is in `done`, the event is deleted. Otherwise it is updated (`done`, `attempts + 1`) and left for the next pass.
    - At `attempts = 5` the event is deleted anyway and the still-failing listeners are logged at `warn`.
    - This keeps spec §4.4's promise that one listener's failure never undoes another's fire, and a failing listener cannot block its siblings.
  - The claim picks events regardless of `attempts`. A retried event is simply re-claimed next pass, and `ORDER BY id` keeps it from starving newer ones only if its retries are few. That is the reason for the cap.
- [ ] **Step 4: Wire the ticker.** In `runtime::spawn_scheduler`, after `fire_due`, call `crate::flow_trigger::process_events(&pool)`. Log at `info` when `fired > 0`, at `warn` on `Err`, and never break the loop. Add a chart comment beside `schedulerEnabled`: turning the scheduler off lets the flow-trigger outbox grow, and turning it on processes the backlog (spec §4.4 point 6).
- [ ] **Step 5: End-to-end** in `tests/runtime.rs`: `a_flow_trigger_runs_to_success_with_nothing_driving_it`. Worker and scheduler on, `Handlers::with_core_tasks()`, producer started through `start_execution`, poll ≤ 20 s → the listener's execution is `SUCCESS`. No call to `tick`, `fire_due` or `process_events` in the test body. Control: scheduler off → zero listener executions. Use `#[sqlx::test]` for isolation, as the W12 runtime tests do.
- [ ] **Step 6: The safety section** in `flow_trigger/mod.rs`, in `scheduler/mod.rs`'s voice, one entry per SQL statement: the outbox INSERT (inside the source's transaction), the claim-delete (`SKIP LOCKED`), the listener SELECT (no lock; why none is needed), the fire (the unique index backstop), and the `done`/`attempts` update. Each entry names its test and its mutation result.
- [ ] **Step 7: Mutations:**

| mutation | must kill |
|---|---|
| drop the process_events call in the ticker | the end-to-end test |
| drop `SKIP LOCKED` and the unique index (two processors, attempt counter as in W12 Task 5) | a new `two_processors_never_fire_one_event_twice` |
| delete the event in its own transaction before firing | `fire_and_delete_are_atomic` |
| drop the savepoint | `a_broken_listener_does_not_stop_its_siblings` |
| roll back the whole event on a retried error | `a_transient_failure_keeps_the_event_for_that_listener_only` (the sibling fires twice or not at all) |
| never give up | `a_listener_that_keeps_failing_is_given_up_after_five_attempts` |
| render `when` on the listener's context instead of the source's | a `when` reading `{{ inputs.x }}` of the source (add it to `refusals` as a positive case) |
| drop the depth check | `a_cycle_stops_at_depth_100` |
| each off-switch clause | `every_off_switch` |

- [ ] **Step 8: Full crate gate; commit:** `feat(workflow): W13a — process the flow-trigger outbox on the scheduler ticker`.

---

## Task 5: Coverage statement, sweep, and the full gate

**Files:** modify `crates/egret-workflow/oracle-exec/README.md`; modify `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (W13a done); modify `trigger/flow.rs` doc lines that say nothing calls `evaluate`.

- [ ] **Step 1: The W13a section** of the coverage statement: ratchet 109 → 109, and it would read 109 with `process_events` deleted. Say both halves.
- [ ] **Step 2: Sweep** the proposition "nothing calls `trigger::flow::evaluate`" / "a flow trigger cannot fire". Search three populations: the phrase, ordinals/counts, and generated data. Report N examined / M corrected per population, with a positive control each. Keep EN and VI in step.
- [ ] **Step 3: Full local CI:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` (only its W13a-file warnings matter: two pre-existing sites are known), and `cargo test --workspace --no-fail-fast`. Diff names against the base. Only W13a tests are added, none are lost, and the only failures are the known StarRocks-live ones.
- [ ] **Step 4: Commit:** `docs(workflow): W13a coverage statement and the sweep`.

---

## Final review

Whole-branch review on the most capable model. Aim it at:
- (1) any transition path that bypasses the hook: every `settle` caller, and state changes written outside `settle`;
- (2) whether an event can be lost or fired twice across a crash between claim and commit;
- (3) the per-listener `done`/`attempts` retry and its give-up path;
- (4) render context correctness: source vs listener, and `PAUSED`-then-resumed;
- (5) cross-workspace, where the listener SELECT's workspace predicate and `evaluate`'s must both hold;
- (6) the outbox's growth with the scheduler off.
