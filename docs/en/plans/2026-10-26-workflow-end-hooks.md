# Workflow package #2 "End hooks" — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** `docs/en/specs/2026-10-26-workflow-end-hooks-design.md`. Read §0 (Kestra's semantics), Decisions D1–D16 and the "False premises" list first. This plan adds no decision. The spec's §12 has five questions for the controller. This plan builds the default answer to each; where an answer would change a task, the task says so.

**Goal:** egret runs every list Kestra runs at the end of a block or an execution:
- the flow's `finally:`;
- `finally:` on every flowable except `Pause` (D14 moves `Pause` to #9);
- `errors:` on `Loop`, `LoopUntil` and `WorkingDirectory` (Part A, PR #2a);
- the flow's `afterExecution:`, in a post-terminal phase, with every SQL rule that read "terminal" as "no live work" moved to `workflow.execution_is_idle` (Part B, PR #2b).

**Architecture:**
- **Model.** `models::Task`'s flowables gain `finally` (and three gain `errors`). `children()` and `error_children()` carry them, so every existing walk sees them.
- **Plan.** `executor::plan::flatten_flow` keeps today's layout as a prefix and appends the new lists (D3). Stored `task_runs.task_ix` therefore stays valid for executions that are already running.
- **The list rule.** One function, `flowable::current_list`, ports Kestra's `findTaskDependingFlowState`, including `finally` (D2). Every flowable and the flow's root call use it.
- **The post-terminal phase.** `advance` gains `after_execution::drive` for a terminal execution (D8). It never changes the execution's state. `executor::terminated` decides when a parent hears about the child (D10).
- **Rule 2.** Migration 0210 defines `workflow.execution_is_idle(uuid)` (D13). The orphan sweep's superseded rule, the `_wd` sweep and the pruner read it.

**Tech stack:** Rust (serde, serde_norway, sqlx, tokio), Postgres for the worker, oracle and sweep tests, and React and TypeScript (vitest) for Task B6.

**Branch and worktree:**
- `feat/workflow-end-hooks` in `data-portal/egret-endhooks`, stacked on `feat/workflow-no-silent-drops` (draft PR #756) at `68c9f4de`.
- Part A is PR #2a. Part B is PR #2b, stacked on #2a.
- One migration, `0210`, in Part B.

## Facts measured for this plan (2026-10-26, at `68c9f4de`)

Paths are relative to `crates/egret-workflow/src/` unless another root is given. Kestra's are in the spec's header form (`ES`, `DE`, `FU`, `ExS`, `k/`).

- **Model.**
  - Flow lists: `finally_` (`models/flow.rs:81-82`, `rename = "finally"`) and `after_execution` (`:91-92`).
  - Flowables: `Sequential` (`models/task.rs:309-316`), `Parallel` (`:330-337`), `If` (`:358-374`), `Switch` (`:382-391`), `Dag` (`:404-409`), `Loop` (`:463-472`, no `errors`), `LoopUntil` (`:488-508`, no `errors`), `Pause` (`:598-609`), `AllowFailure` (`:621-632`, the only one with `finally`), `WorkingDirectory` (`:646-674`, no `errors`).
  - Walks: `children()` (`:122-160`, AllowFailure's `finally` at `:144`), `error_children()` (`:182-204`), and the `_mut` twins (`:212`, `:235`).
  - Unknown keys are dropped by `kind_for`'s `serde_json::from_value` (`:959-989`).
- **Plan.**
  - Structs: `Plan` (`executor/plan.rs:20-37`), `PlanTask` (`:153-322`, `errors` at `:307`), `TaskKind::AllowFailure { tasks, finally }` (`:384-387`), `children_of`/`direct_task_children` (`:108-149`).
  - Entry points: `build_plan` (`:1095-1124`), `plan_stored` (`:1149-1151`), `flatten_flow` (`:1173-1183`), `flatten_list` (`:1231-1241`), `flatten_one` (`:1243`, pre-order reserve at `:1255-1258`), AllowFailure's three lists (`:1292-1300`), `pause_gap` (`:862-919`, `errors` at `:866`).
  - `LoopUntil` flattens no `errors` (`:1470-1475`).
- **Executor.**
  - `advance` (`executor/mod.rs:178`), its terminal early return (`:179-189`), the root call (`:225`), `drives_forward` (`:326-331`).
  - `current_list` (`executor/flowable/mod.rs:199-233`), `aggregate` (`:244-282`), `run_of` (`:284`), `descend` (`:488-587`).
  - Flowable drives: `sequential::drive` (`sequential.rs:44-62`), `walk` (`:85`), `walk_errors` (`:163`); `parallel::drive` (`parallel.rs:33-66`); `dag::drive` (`dag.rs:76`); `branch::drive_if` (`branch.rs:130`) and `drive_switch` (`:191`); `allow_failure::drive` (`allow_failure.rs:23-48`); `working_directory::drive` (`working_directory.rs:27-49`); `loop_until::drive` (`loop_until.rs:224-279`).
  - The AllowFailure tests: `allow_failure_drive_tests` (`executor/mod.rs:10033-10170`, a `run(src, fail)` helper at `:10042`).
  - Corpus helpers in `executor/mod.rs` `tests`: `plan_of` (`:407`), `corpus_source` (`:413`), `every_corpus_flow` (`:428`), and `every_corpus_flow_graphs_and_every_planned_task_is_one_node` (`:458`).
- **Storage of runs.** `task_runs.task_ix` is written at `worker/claim.rs:538` and read as `TaskRun::task` at `:4393`. Each load re-plans with `plan_stored(&source)` (`:4344-4349`). Starts take the draft row's `flows.source` (`routes.rs:2621-2625`). Nothing reads the snapshot's `"plan"` at run time (`publish.rs:347-351`).
- **Loop iterations.** `sub_flow_source` (`worker/spawn.rs:823-883`; clears `errors`/`finally_`/`after_execution` at `:836-839`; re-serialises at `:878`); `find_task` (`:891-902`).
- **Worker.**
  - Claim and settle: claim SQL (`claim.rs:519-560`, no execution-state filter), due retries (`:799-807`), `notifies_parent` (`:2930`), the settle tail's `terminal:` (`:3487-3497`), outbox (`:3608-3672`), `sla_terminal` (`:4652-4691`).
  - `render_updates` gate (`worker/render.rs:1504`).
  - Kill route terminal answer (`worker/kill.rs:186`); kill cascade (`:358`).
- **Sweeps.** `run_liveness` superseded query (`worker/commands.rs:2591-2597`), `sweep_decision` (`:2672`, premise doc `:2669-2671`), `pod_sweep_deletes` (`worker/k8s/runner.rs:1592`), `_wd` sweep selection (`storage/wd_sweep.rs:163-169`), pruner (`storage/prune.rs:10-30`, `terminal()` `:375-381`).
- **Walker.** `spec/keys.rs`: `ROOT_LISTS` (`:432`), `child_lists` (`:507-522`), `silent` (`:561-569`), the `lift_of(…Sequential, "finally") == None` assertion (`:1280-1286`), `TWO` (`:1351`), `finally_and_the_errors_two_takes_draw_nothing` (`:1636-1650`).
- **Graph.** The flow lists are drawn at `graph/mod.rs:489-553`. Flowable `finally` is drawn for `AllowFailure` only (`:831-841`).
- **Frontend.** `executionFinished` (`frontend/src/components/workflows/executions/polling.ts:43-45`), `ExecutionTopologyTab.tsx:67-79`, `ExecutionGanttTab.tsx:91`, `timing.ts:50`, `actions.ts:53,74`.
- **Oracle.**
  - Ratchet 301 golden / 72 mismatching / 72 baselined / 0 broken. `(301, 229, 72)` is asserted at `tests/executor.rs:813`.
  - Census: `LIVE` 295, `VALIDATE_REFUSED` 25, `UNKNOWN_TYPE_REFUSED` 21.
  - Constants that will move: `EX:6999` (finally 14), `EX:7044` (afterExecution 10), `MISMATCH_ON_RUNS` 55 (`EX:4402`), `MISMATCH_ON_ORDER` 27 (`EX:4426`), `MISMATCH_ON_EXECUTION` 32 (`EX:4377`), `SPAWNED_ENTRIES_DIFFERING_ONLY_INSIDE_SPAWNED` 4 (`EX:4826`), `RESTRICTION_REMOVES*` (`EX:3098-3150`), kill names (`EX:7527`).
  - The baseline lines of the 21 entries are in spec §7.1.
- **Corpus** (Ruby scan of all 351 files, 2026-10-26):
  - flowable `finally`: 7 flows (Sequential 2, Parallel 1, Dag 1, Loop 2, Pause 1);
  - `errors` on Loop/LoopUntil/WorkingDirectory: 6 (`loop-switch-failed`, `all-flowable`, `finally-loop`, `finally-loop-parallel`, `working-directory`);
  - inside `afterExecution:` only `OutputValues`, `Log` and `If`; no `retry:` or `timeout:` inside any `finally:`/`afterExecution:`; no `Pause`/`Exit`/`Subflow`/`Loop` in `afterExecution:`.
- **Migrations:** the highest is `migrations/0209_username_ledger.sql`; 0210 is free on this branch.

## Global Constraints

- **The private stack only.** Every cargo command runs with exactly:

  ```bash
  export DATABASE_URL=postgres://egret:egret@localhost:5458/egret_test; export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6458; export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-endhooks/target; unset EGRET_ALLOW_DB_SKIP EGRET_ALLOW_STORE_SKIP
  ```

  The containers `egret-endhooks-postgres` and `egret-endhooks-valkey` are already running.
  - Never use port 5432, and never an `egret-dev-*` or `egret-demo-*` container.
  - Never set any `EGRET_ALLOW_*_SKIP`: a DB-gated test that returns early has asserted nothing.
- **One cargo command at a time.** Never `pkill` cargo. `rg` is not installed: use `grep -rn`. `cp` is aliased to `-i` here: in scripts use `command cp -f`.
- **The oracle ratchet stays green after every task:** `cargo test -p egret-workflow --test executor -- --nocapture`.
  - Record in the task report, before and after: golden, mismatching, baselined, broken, `LIVE`, `VALIDATE_REFUSED`, `UNKNOWN_TYPE_REFUSED`. The baseline on this branch is 301/72/72/0, LIVE 295, VALIDATE_REFUSED 25, UNKNOWN_TYPE_REFUSED 21.
  - A constant changes only in the task that moves it, with a dated comment naming the entries.
  - `oracle-exec/golden.json` is never edited.
  - A baselined entry that starts matching makes the ratchet fail as "banked". Its line is then removed from `oracle-exec/baseline.txt` in the same task, never earlier.
- **MANDATORY full-suite gate at the end of every task**, after the task's own tests and the ratchet:

  ```bash
  cargo test -p egret-workflow --no-fail-fast > /private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/3525fc70-d007-4b4e-b99e-ef27f3a3a1f8/scratchpad/endhooks-<task>.txt 2>&1
  grep -n "FAILED\|panicked\|failed," /private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/3525fc70-d007-4b4e-b99e-ef27f3a3a1f8/scratchpad/endhooks-<task>.txt
  ```

  - Only the 7 `sql_starrocks_live` tests may fail: they need a StarRocks engine.
  - `runtime::a_cancelled_token_stops_both_tickers` is a load-timing flake. If it fails, re-run it alone and record that it passes.
  - Any other failure blocks the commit. Earlier packages' per-task test subsets missed false refusals that only the full suite caught.
- **Mutation discipline.** For each new rule, break it once by hand, see a named test go red, and restore it. The task report records the mutation, the command and the red line. A mutation that leaves everything green means a test is missing: add it.
- **Messages are verbatim** as the spec gives them. Each new message says in its doc comment whether it is Kestra's (with file:line) or egret's own.
- **Kestra semantics come from the spec's §0.** If the code says otherwise, stop and report. Never "fix" the spec in code.
- **No network commands in tasks** (no `git push/fetch/pull`, no `gh`, no `npm install` from the network). The controller pushes.
- **Commits** follow Conventional Commits and end with the line `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Never commit `.superpowers/` or anything under the scratchpad.
- **Line numbers are from `68c9f4de`.** Re-grep every anchor before editing.

## File map

| File | Responsibility | Task |
|---|---|---|
| `src/executor/plan_layout_pre_end_hooks.json` (new, test data) | today's plan layout per corpus flow | A1 |
| `src/models/task.rs` | `finally` on 9 flowables, `errors` on 3; `children()`/`error_children()` | A2 |
| `src/executor/plan.rs` | `Plan::finally`/`after_execution`, `PlanTask::finally`, deferred flattening, `pause_gap` refuses `finally` | A2, A3, B1 |
| `src/graph/mod.rs`, `src/graph/tests.rs` | flowable `finally` drawn for every kind | A3 |
| `src/executor/flowable/{mod,sequential,parallel,dag,branch,allow_failure}.rs` | `current_list` with `Finally`; callers | A4 |
| `src/executor/mod.rs` | root call passes `plan.finally`; tests; `after_execution` branch; `terminated`, `drives_after` | A4, B1 |
| `src/executor/flowable/{loop_until,working_directory}.rs` | per-iteration and after-finish lists | A5 |
| `src/worker/spawn.rs` | `sub_flow_source` keeps the `Loop`'s lists; `find_task` walks `after_execution` | A6, B1 |
| `src/spec/keys.rs`, `src/spec/schema/property-treatment.json` | walker descends; regenerated rows | A7 |
| `oracle-exec/baseline.txt`, `tests/executor.rs`, `oracle-exec/README.md`, `oracle-exec/handler_surface.py`, `oracle-exec/handler-surface.txt` | banks and constants (each bank in the task that causes it) | A4, A6, A8, B3, B7 |
| `src/executor/after_execution.rs` (new) | the post-terminal phase | B1 |
| `src/spec/validate.rs`, `src/spec/keys.rs` | D12 refusal, D11 warning | B2 |
| `src/worker/claim.rs`, `src/worker/render.rs` | parent report at `terminated`; render gate | B3 |
| `tests/end_hooks.rs` (new, DB) | worker-level behaviour of the phase | B3, B4, B5 |
| `migrations/0210_workflow_execution_is_idle.sql` (new) | the predicate | B5 |
| `src/worker/commands.rs`, `src/storage/wd_sweep.rs`, `src/storage/prune.rs` | read `execution_is_idle` | B5 |
| `frontend/src/components/workflows/executionState.ts`, `executions/polling.ts`, `executions/topology/ExecutionTopologyTab.tsx`, `executions/gantt/ExecutionGanttTab.tsx`, `executions/timing.ts` (+ tests) | "finished" = terminal and no live run | B6 |
| docs (EN) | spec as-built notes, roadmap notes, spec #1 §10 closed, debt spec note, programme §9, schema README, oracle README | A8, B7 |
| docs (VI), `egret-data` Platform docs | mirrors | B8 |

---

## Part A — PR #2a: `finally:` and `errors:` inside a live execution

### Task A1: freeze today's plan layout (D3's instrument)

**Files:**
- Create: `src/executor/plan_layout_pre_end_hooks.json`.
- Modify: `src/executor/mod.rs` (`tests`): the generator and the prefix test.

**Interfaces:** none in production. The fixture is a JSON object `{ "<corpus id>": [[ix, "<task id>", "<kind tag>", [direct children…]], …] }` for every corpus flow that `plan_stored` plans. `<kind tag>` is the `TaskKind` variant name. The children come from `Plan::children_of(ix)` as it is today.

- [ ] **Step 1: write the generator and the test, before any production change.**

```rust
/// One flow's layout as D3 freezes it: index, id, kind, and the children
/// `Plan::children_of` returned before package #2.
fn layout(plan: &Plan) -> Vec<(usize, String, String, Vec<usize>)> {
    (0..plan.tasks.len())
        .map(|ix| {
            let kind = format!("{:?}", plan.tasks[ix].kind);
            let tag = kind.split([' ', '{', '(']).next().unwrap_or("").to_string();
            (ix, plan.tasks[ix].id.clone(), tag, plan.children_of(ix))
        })
        .collect()
}

/// **D3: a plan built after package #2 starts with the layout every stored
/// `task_runs.task_ix` was written against** (`worker/claim.rs:538`, read
/// back at `:4393`). A list #2 adds may only be APPENDED: an execution that
/// was running at deploy time is re-planned from its own source at its next
/// claim, and a shifted index would hand its runs another task's spec.
/// `EGRET_WRITE_PLAN_LAYOUT=1` rewrites the fixture; that is allowed only in
/// Task A1, at `68c9f4de`, before any planner change.
#[test]
fn the_plan_before_end_hooks_is_a_prefix_of_every_plan_after() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/executor/plan_layout_pre_end_hooks.json");
    let mut now = serde_json::Map::new();
    for (id, source) in every_corpus_flow() {
        if let Ok(plan) = plan::plan_stored(&source) {
            now.insert(id, serde_json::to_value(layout(&plan)).unwrap());
        }
    }
    if std::env::var("EGRET_WRITE_PLAN_LAYOUT").as_deref() == Ok("1") {
        std::fs::write(path, serde_json::to_string_pretty(&now).unwrap()).unwrap();
    }
    let frozen: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(frozen.len() >= 300, "the fixture covers the corpus ({})", frozen.len());
    let mut moved = Vec::new();
    for (id, old) in &frozen {
        let old: Vec<(usize, String, String, Vec<usize>)> = serde_json::from_value(old.clone()).unwrap();
        let Some(new) = now.get(id) else {
            moved.push(format!("{id}: planned before #2, not now"));
            continue;
        };
        let new: Vec<(usize, String, String, Vec<usize>)> = serde_json::from_value(new.clone()).unwrap();
        for o in &old {
            // A node keeps its index, id and kind, and every old child is still
            // its child (a new `finally` or a loop's new `errors` only adds some).
            match new.get(o.0) {
                Some(n) if n.1 == o.1 && n.2 == o.2 && o.3.iter().all(|c| n.3.contains(c)) => {}
                other => moved.push(format!("{id}: ix {} was {o:?}, is {other:?}", o.0)),
            }
        }
    }
    assert!(moved.is_empty(), "{moved:#?}");
}
```

  The `AllowFailure` tag changes in A3 (`AllowFailure { tasks }`), but `format!("{:?}")` up to the first space or brace is `AllowFailure` both times.
- [ ] **Step 2: generate.** `EGRET_WRITE_PLAN_LAYOUT=1 cargo test -p egret-workflow --lib -- executor::tests::the_plan_before_end_hooks`. Then run it again without the variable: PASS. Record the number of flows in the fixture.
- [ ] **Step 3: mutation.** Swap the ids of two entries of one flow in the fixture by hand, see the test red with that flow's id, and restore the file with `git checkout`.
- [ ] **Step 4: ratchet and full-suite gate.** No count moves.
- [ ] **Step 5: commit** `test(workflow): freeze the plan layout every stored task_ix was written against`.

---

### Task A2: the model reads `finally:` and the loops' `errors:`; `Pause` `finally:` is refused (D14)

**Files:**
- Modify: `src/models/task.rs`: the fields, `children()`, `error_children()` and their `_mut` twins.
- Modify: `src/executor/plan.rs` `pause_gap`: refuse `finally`.
- Modify: `src/spec/validate.rs` tests (the REFUSED fixture table, if it lists `pause_gap` cases).

**Interfaces:**

```rust
// On Sequential, Parallel, If, Switch, Dag, Loop, LoopUntil, WorkingDirectory, Pause:
/// Kestra `AbstractBranch._finally` (`k/plugin/core/flow/AbstractBranch.java:30-35`),
/// `If.java:114`, `Switch.java:115`, `Dag.java:109`, `Pause.java:199`.
#[serde(rename = "finally", default, skip_serializing_if = "Vec::is_empty")]
pub finally: Vec<Task>,
// On Loop, LoopUntil, WorkingDirectory (AbstractBranch.errors):
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub errors: Vec<Task>,
```

`children()` returns, for each kind, its control-flow children and then `finally`, in the order `AllowFailure` uses today. `error_children()` returns `errors` for `Loop`, `LoopUntil` and `WorkingDirectory` too.

The `pause_gap` message, egret's own, in the same shape as the `errors` row: ``task '<id>' is a pause with 'finally', which egret does not run yet: … Roadmap package #9 lifts it``. Copy the `errors` row's text exactly and change only the property name.

- [ ] **Step 1: failing tests** (in `models/task.rs` tests):

```rust
#[test]
fn every_flowable_reads_finally_and_lists_it_last_among_its_children() {
    for (ty, body) in [
        ("io.kestra.plugin.core.flow.Sequential", "tasks: [{id: a, type: core.log, message: m}]"),
        ("io.kestra.plugin.core.flow.Parallel", "tasks: [{id: a, type: core.log, message: m}]"),
        ("io.kestra.plugin.core.flow.If", "condition: 'true'\nthen: [{id: a, type: core.log, message: m}]"),
        ("io.kestra.plugin.core.flow.Switch", "value: x\ncases: {x: [{id: a, type: core.log, message: m}]}"),
        ("io.kestra.plugin.core.flow.Dag", "tasks: [{task: {id: a, type: core.log, message: m}}]"),
        ("io.kestra.plugin.core.flow.Loop", "values: [1]\ntasks: [{id: a, type: core.log, message: m}]"),
        ("io.kestra.plugin.core.flow.LoopUntil", "condition: 'true'\ntasks: [{id: a, type: core.log, message: m}]"),
        ("io.kestra.plugin.core.flow.WorkingDirectory", "tasks: [{id: a, type: io.kestra.plugin.scripts.shell.Commands, commands: [x]}]"),
    ] {
        let src = format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: b\n    type: {ty}\n    {}\n    finally: [{{id: z, type: core.log, message: m}}]\n",
            body.replace('\n', "\n    ")
        );
        let flow = crate::spec::parse(&src).unwrap_or_else(|e| panic!("{ty}: {e:?}"));
        let ids: Vec<&str> = flow.tasks[0].children().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.last(), Some(&"z"), "{ty}: {ids:?}");
    }
}

#[test]
fn loop_loop_until_and_working_directory_read_errors() {
    for (ty, body) in [
        ("io.kestra.plugin.core.flow.Loop", "values: [1]"),
        ("io.kestra.plugin.core.flow.LoopUntil", "condition: 'true'"),
        ("io.kestra.plugin.core.flow.WorkingDirectory", ""),
    ] {
        let src = format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: b\n    type: {ty}\n    {body}\n    tasks: [{{id: a, type: core.log, message: m}}]\n    errors: [{{id: e, type: core.log, message: m}}]\n"
        );
        let flow = crate::spec::parse(&src).unwrap();
        let ids: Vec<&str> = flow.tasks[0].error_children().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["e"], "{ty}");
    }
}
```

  In `executor/plan.rs` tests:

```rust
#[test]
fn a_pause_finally_is_refused_and_names_package_nine() {
    let src = "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n    finally: [{id: z, type: core.log, message: m}]\n";
    let e = build_plan(src).unwrap_err().message;
    assert!(e.starts_with("unsupported_pause_property"), "{e}");
    assert!(e.contains("'finally'") && e.contains("Roadmap package #9 lifts it"), "{e}");
}
```

- [ ] **Step 2: run, expect FAIL:** `cargo test -p egret-workflow --lib -- models::task executor::plan::tests::a_pause_finally`.
- [ ] **Step 3: implement.** The flattener is not changed yet: `flatten_one` reads fields by name, so the new fields are ignored until A3. Run the A1 prefix test: it stays green, because nothing is planned yet.
- [ ] **Step 4: the duplicate-id census.** `children()` now reaches the new lists, so `duplicate_task_id` can newly fire. Run `cargo test -p egret-workflow --lib -- spec` and the ratchet, and record `LIVE`/`VALIDATE_REFUSED`. Expected: unchanged. A move is a finding: name the flow and stop.
- [ ] **Step 5: mutation.** Drop `finally` from `Sequential`'s arm of `children()`: the first test goes red on `Sequential`. Restore. Drop the `finally` arm of `pause_gap`: the Pause test goes red. Restore.
- [ ] **Step 6: ratchet and full-suite gate.** Expected: no count moves (the keys walker still skips the new lists until A7).
- [ ] **Step 7: commit** `feat(workflow): flowables read finally and the loops read errors; Pause finally is refused for #9`.

---

### Task A3: the plan appends the new lists (D3); the graph draws them

**Files:**
- Modify: `src/executor/plan.rs`: `Plan::finally`, `PlanTask::finally`, `TaskKind::AllowFailure { tasks }`, `children_of`, `flatten_flow`, `flatten_one`.
- Modify: every reader of `TaskKind::AllowFailure { tasks, finally }`: `grep -rn "AllowFailure {" src`.
- Modify: `src/graph/mod.rs:831-841` (draw `finally` for every kind with the field), `src/graph/tests.rs`.

**Interfaces:**

```rust
pub struct Plan {
    pub tasks: Vec<PlanTask>,
    pub roots: Vec<TaskIx>,
    pub errors: Vec<TaskIx>,
    /// The flow's `finally:` (package #2a). Appended after every list the
    /// plan held before #2 (D3).
    pub finally: Vec<TaskIx>,
    pub retry: Option<RetryPolicy>,
}
pub struct PlanTask {
    // … unchanged …
    /// The block's `finally:` (Kestra `AbstractBranch._finally`). Inline for
    /// `AllowFailure` (where it always was), appended for every other kind (D3).
    pub finally: Vec<TaskIx>,
}
```

`flatten_flow` keeps `flatten_list(roots)` and `flatten_list(errors)` first. `flatten_one` pushes `(ix, &model_task)` onto a `deferred: VecDeque` for each kind whose new lists exist. The kinds are every flowable except `AllowFailure` (inline) and `Loop` (D5: nothing in the parent). Then `flatten_flow` drains the queue. Each drained entry sets `out[ix].finally` and, for `LoopUntil`/`WorkingDirectory`, `out[ix].errors`, flattening them with `flatten_list`, which defers nested new lists again. Then it flattens `flow.finally_` into `Plan::finally` and drains again. Thread the queue as a `&mut` parameter through `flatten_list`/`flatten_one`.

- [ ] **Step 1: failing tests** (`executor/plan.rs` tests):

```rust
#[test]
fn a_sequential_finally_is_planned_after_everything_that_was_planned_before() {
    let src = "id: f\nnamespace: a.b\ntasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks: [{id: a, type: core.log, message: m}]\n    finally: [{id: z, type: core.log, message: m}]\n  - id: after\n    type: core.log\n    message: m\nfinally:\n  - id: ff\n    type: core.log\n    message: m\n";
    let plan = plan_stored(src).unwrap();
    let ids: Vec<&str> = plan.tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["seq", "a", "after", "z", "ff"], "today's layout first, then the new lists");
    let seq = 0;
    assert_eq!(plan.tasks[seq].finally, vec![3]);
    assert_eq!(plan.finally, vec![4]);
    assert_eq!(plan.children_of(seq), vec![1, 3]);
}

#[test]
fn an_allow_failure_finally_stays_inline() {
    let src = "id: f\nnamespace: a.b\ntasks:\n  - id: af\n    type: io.kestra.plugin.core.flow.AllowFailure\n    tasks: [{id: a, type: core.log, message: m}]\n    finally: [{id: z, type: core.log, message: m}]\n  - id: after\n    type: core.log\n    message: m\n";
    let plan = plan_stored(src).unwrap();
    let ids: Vec<&str> = plan.tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["af", "a", "z", "after"]);
    assert_eq!(plan.tasks[0].finally, vec![2]);
}

#[test]
fn a_loops_errors_and_finally_are_not_planned_in_the_parent() {
    let src = "id: f\nnamespace: a.b\ntasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks: [{id: a, type: core.log, message: m}]\n    errors: [{id: e, type: core.log, message: m}]\n    finally: [{id: z, type: core.log, message: m}]\n";
    let plan = plan_stored(src).unwrap();
    assert!(plan.tasks.iter().all(|t| t.id != "e" && t.id != "z"));
}
```

  In `graph/tests.rs`: `a_sequential_finally_is_drawn_like_an_allow_failures`. Copy the existing AllowFailure test (`grep -n "AllowFailure" src/graph/tests.rs`) and assert one node `z` on `Branch::Finally`. The corpus test `every_corpus_flow_graphs_and_every_planned_task_is_one_node` (`executor/mod.rs:458`) must stay green; it fails if the graph does not draw a planned `finally`.
- [ ] **Step 2: run, expect FAIL:** `cargo test -p egret-workflow --lib -- executor::plan graph`.
- [ ] **Step 3: implement.** Run the A1 prefix test: it must stay green. Then run `cargo test -p egret-workflow --lib -- executor graph`. Expect some executor tests over corpus flows with `finally:` to change: the executor does not drive the new lists until A4, so the flows that end without running them are unchanged. Record any test that moves.
- [ ] **Step 4: mutation.** (a) Flatten a `Sequential`'s `finally` inline (in `flatten_one`) instead of deferring it: the A1 prefix test goes red on `valids/finally-sequential`. Restore. (b) Leave `finally` out of `children_of`: `the_plan_before…` stays green, but `a_sequential_finally…`'s last assertion goes red. Restore.
- [ ] **Step 5: ratchet and full-suite gate.** Expected: the ratchet unchanged (nothing drives the lists yet).
- [ ] **Step 6: commit** `feat(workflow): the plan appends finally lists after today's layout`.

---

### Task A4: one list rule, Kestra's (D2)

**Files:**
- Modify: `src/executor/flowable/mod.rs` (`current_list`, `Current::Finally`), `sequential.rs`, `parallel.rs`, `dag.rs`, `branch.rs`, `allow_failure.rs`, `src/executor/mod.rs:225`.
- Test: a new `mod end_hooks_drive_tests` in `src/executor/mod.rs`, modelled on `allow_failure_drive_tests` (copy its `run` and `last_of` helpers, and add an `order` helper returning run ids in creation order).

**Interfaces:**

```rust
pub(crate) enum Current<'a> {
    Tasks(&'a [TaskIx]),
    Errors(&'a [TaskIx]),
    /// Kestra `findTaskDependingFlowState`'s `finally` answer (`Execution.java:589-593,615-620`).
    Finally(&'a [TaskIx]),
    RetryPending,
    Ended,
}
pub(crate) fn current_list<'a>(
    plan: &Plan, state: &ExecutionState,
    tasks: &'a [TaskIx], errors: &'a [TaskIx], finally: &'a [TaskIx],
    parent: Option<RunIx>, now: Tick,
) -> Current<'a>;
pub(crate) fn drive(  // sequential
    plan: &Plan, state: &ExecutionState,
    children: &[TaskIx], errors: &[TaskIx], finally: &[TaskIx],
    parent: Option<RunIx>, now: Tick,
) -> Step;
```

`current_list`'s body follows spec §4.1's case list line by line, with a comment citing `Execution.java` lines on each arm. "Exhausted" = every task in the list has a run whose latest state is terminal.

- [ ] **Step 1: failing tests.** One per row. Each source is a corpus shape cut to the minimum, using `core.log` leaves and `io.kestra.plugin.core.execution.Fail` for failure (as `BLOCK` does).

```rust
/// spec §0.1, golden `finally-sequential`: tasks, then finally; the block ends
/// after its finally and keeps its tasks' state.
#[test]
fn a_sequential_runs_finally_after_a_failure_and_stays_failed() {
    let (plan, state, flow) = run(SEQ_FINALLY, &["ko"]);
    assert_eq!(order(&plan, &state), ["seq", "ko", "a1", "a2"]);
    assert_eq!(last_of(&plan, &state, "seq"), Some(RunState::Failed));
    assert_eq!(last_of(&plan, &state, "a1"), Some(RunState::Success));
    assert_eq!(flow, RunState::Failed);
}

/// `finally` runs on success too.
#[test]
fn a_sequential_runs_finally_after_success() { /* run(SEQ_FINALLY, &[]) -> seq SUCCESS, a1 a2 SUCCESS */ }

/// spec §0.2: a failing finally child changes nothing and does not stop the next one.
#[test]
fn a_failing_finally_child_neither_fails_the_block_nor_stops_the_next() {
    let (plan, state, flow) = run(SEQ_FINALLY, &["a1"]);
    assert_eq!(last_of(&plan, &state, "a1"), Some(RunState::Failed));
    assert_eq!(last_of(&plan, &state, "a2"), Some(RunState::Success));
    assert_eq!(last_of(&plan, &state, "seq"), Some(RunState::Success));
    assert_eq!(flow, RunState::Success);
}

/// golden `finally-sequential-error`: errors strictly before finally.
#[test]
fn errors_run_before_finally() { /* order: seq, ko, e1, e2, a1, a2 */ }

/// golden `finally-flow-error`: the flow's own lists in the same order.
#[test]
fn the_flows_errors_then_finally() { /* root: ko, e1, e2, a1, a2; flow FAILED */ }

/// golden `finally-flow-error-first`: the sibling after the failure is never created.
#[test]
fn the_flows_finally_skips_the_rest_of_tasks() { /* ko, a1; "ok" absent */ }

/// golden `finally-parallel`: finally is sequential under a Parallel.
#[test]
fn a_parallels_finally_runs_one_at_a_time() { /* a1 terminal before a2 is created */ }

/// `If.java:188-198`: an If whose branch is empty ends without its finally.
#[test]
fn an_if_with_nothing_to_run_skips_its_finally() { /* condition false, no else: z absent */ }

/// `FU:163-170`: a block whose children are all disabled is SUCCESS without finally.
#[test]
fn a_block_of_disabled_children_skips_its_finally() { /* all disabled: z absent, seq SUCCESS */ }

/// `Execution.java:602-606`: a failure that will be retried starts neither list.
#[test]
fn a_retry_pending_failure_starts_no_finally() { /* ko with retry maxAttempts 2: z created only after the last attempt */ }

/// allow_failure_drive_tests keep passing unchanged: AllowFailure is on the same rule.
```

  Write each `/* … */` as full code in the shape of the first test. `SEQ_FINALLY` is a const like `BLOCK`: a `Sequential` `seq` holding `ko` (`Fail`), with `finally: [a1, a2]` (`core.log`).
- [ ] **Step 2: run, expect FAIL:** `cargo test -p egret-workflow --lib -- executor::end_hooks_drive_tests executor::allow_failure_drive_tests`.
- [ ] **Step 3: implement**, then run `cargo test -p egret-workflow --lib -- executor`.
- [ ] **Step 4: mutation**, each restored after its red:
  - (a) In `current_list`, return `Errors` before checking "finally started": `errors_run_before_finally` stays green, but a flow whose `finally` started while `errors` exist must go red. If nothing goes red, add `once_finally_has_started_errors_are_not_re_entered`.
  - (b) Make `Done` aggregate over `finally` too: `a_failing_finally_child…` goes red.
  - (c) Drop the `If` empty-branch skip: `an_if_with_nothing_to_run…` goes red.
  - (d) Drive `Finally` with `parallel::drive_concurrent`: `a_parallels_finally_runs_one_at_a_time` goes red.
- [ ] **Step 5: ratchet, and bank in this task.** Expected: ten entries report "banked": `finally-flow`, `finally-flow-error`, `finally-flow-error-first`, `finally-sequential`, `finally-sequential-error`, `finally-sequential-error-first`, `finally-dag`, `finally-parallel`, `restart-with-finally` and `task-runif-taskswithstate`. The real list is whatever the printout says.
  - Remove exactly those lines from `oracle-exec/baseline.txt`, never more.
  - Move every constant they move, each with a dated comment naming them: `(301, 229, 72)` (`EX:813`), the attribution rows (`EX:6999`, `EX:7044`), `MISMATCH_ON_*`, `RESTRICTION_REMOVES*`, the handler-surface guard and its regenerated file, and the README counts.
  - For an expected entry that does not bank, record the first differing field from the printout and leave its line in place. A8 rewrites its cause.
  - The ratchet is green again at the end of this step. Any change other than "banked" is a stop.
- [ ] **Step 6: full-suite gate.**
- [ ] **Step 7: commit** `feat(workflow): finally runs on every flowable and on the flow, by Kestra's list rule`.

---

### Task A5: `LoopUntil` and `WorkingDirectory` run their `errors:` and `finally:` (D6, D7)

**Files:** `src/executor/flowable/loop_until.rs`, `src/executor/flowable/working_directory.rs`, and the tests in `end_hooks_drive_tests`.

- [ ] **Step 1: measure first (spec §4.1).** Read `worker/working_dir.rs` and `worker/claim.rs`'s `NotALeaf`/block finish path. Answer in the report: after `Command::FinishWorkingDirectory` and `CompleteWorkingDirectoryClaim`, can the block's run stay `RUNNING` while its `errors`/`finally` children run, without the worker treating it as a live block (re-preparing it, or the `_wd` sweep deleting what it needs)? If not, **stop**. Report to the controller and propose the fallback of spec §4.1: refuse `WorkingDirectory` `errors:`/`finally:` with `unsupported_working_directory_property`, naming #17.
- [ ] **Step 2: failing tests:**
  - `a_loop_until_runs_errors_then_finally_in_each_iteration_before_the_condition`: two iterations, the body fails in iteration 1. Assert the order `body, e, z` within each iteration, and that the condition is rendered after `z`. Use `loop_until`'s existing test harness: `grep -n "fn .*loop_until" src/executor/mod.rs`.
  - `a_working_directory_runs_errors_and_finally_after_it_finishes` and `a_working_directory_runs_finally_after_success` (D7: egret runs it; Kestra's code does not, Q4).
  - `a_restarted_loop_until_discards_its_finally_runs` (`RestartLoop` reaches `finally` through `children_of`).
- [ ] **Step 3: run, expect FAIL**, then implement. Run `cargo test -p egret-workflow --lib -- executor` and `--test working_dir_claims`.
- [ ] **Step 4: mutation.** Ask the condition before `finally`: the first test goes red. Restore.
- [ ] **Step 5: ratchet** (no new bank expected; record) **and full-suite gate.**
- [ ] **Step 6: commit** `feat(workflow): LoopUntil and WorkingDirectory run their errors and finally`.

---

### Task A6: a `Loop` iteration runs the `Loop`'s `errors:` and `finally:` (D5)

**Files:** `src/worker/spawn.rs` (`sub_flow_source`, its module doc `:50-62`, `find_task`).

- [ ] **Step 1: failing test** (`spawn.rs` tests):

```rust
#[test]
fn an_iteration_runs_the_loops_errors_and_finally_as_its_own() {
    let src = "id: f\nnamespace: a.b\nerrors: [{id: fe, type: core.log, message: m}]\nfinally: [{id: ff, type: core.log, message: m}]\nafterExecution: [{id: fa, type: core.log, message: m}]\ntasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks: [{id: a, type: core.log, message: m}]\n    errors: [{id: e, type: core.log, message: m}]\n    finally: [{id: z, type: core.log, message: m}]\n";
    let sub = crate::spec::parse(&sub_flow_source(src, "each", uuid::Uuid::nil()).unwrap()).unwrap();
    let ids = |l: &[crate::models::Task]| l.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&sub.tasks), ["a"]);
    assert_eq!(ids(&sub.errors), ["e"], "the Loop's errors, not the flow's (ES:1037-1043)");
    assert_eq!(ids(&sub.finally_), ["z"]);
    assert!(sub.after_execution.is_empty(), "a LOOP iteration has no afterExecution (ExS:1331)");
}
```

- [ ] **Step 2: run, expect FAIL**, then implement. Rewrite the module doc's "Where this is knowingly narrower than Kestra" paragraph (`spawn.rs:50-62`) as a dated correction (it is history; do not delete it).
- [ ] **Step 3: ratchet, and bank in this task as in A4.** Expected new banks: `valids/finally-loop`, `valids/finally-loop-parallel` and `invalids/loop-switch-failed`. `SPAWNED_ENTRIES_DIFFERING_ONLY_INSIDE_SPAWNED` (`EX:4826`) moves with them.
- [ ] **Step 4: mutation.** Keep clearing `errors`: the test goes red, and so does the ratchet's bank of `loop-switch-failed`. Restore.
- [ ] **Step 5: full-suite gate.**
- [ ] **Step 6: commit** `feat(workflow): a Loop iteration runs the Loop's errors and finally`.

---

### Task A7: the walker descends into the new lists; property treatment regenerated

**Files:** `src/spec/keys.rs`, `src/spec/schema/property-treatment.json`, `src/spec/schema/README.md` (counts).

- [ ] **Step 1: failing tests** (`keys.rs` tests; replace `finally_and_the_errors_two_takes_draw_nothing` at `:1636-1650`):

```rust
#[test]
fn a_typo_inside_a_flowable_finally_is_refused_at_its_path() {
    only(
        &format!("{HEAD}tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks: [{{id: a, type: core.log, message: m}}]\n    finally:\n      - id: z\n        type: core.log\n        message: m\n        mesage: typo\n"),
        "tasks.seq.z.mesage",
        "unknown_property",
    );
}

#[test]
fn a_typo_inside_a_loop_errors_is_refused_at_its_path() {
    only(
        &format!("{HEAD}tasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks: [{{id: a, type: core.log, message: m}}]\n    errors:\n      - id: e\n        type: core.log\n        message: m\n        mesage: typo\n"),
        "tasks.each.e.mesage",
        "unknown_property",
    );
}

#[test]
fn finally_and_the_loops_errors_are_model_keys_now() {
    silent_census(&format!("{HEAD}tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks: [{{id: a, type: core.log, message: m}}]\n    finally: [{{id: z, type: core.log, message: m}}]\n"));
}
```

  Check the path form against `spec::path::task_path` (`{parent path}.{id}`) before asserting.
- [ ] **Step 2: run, expect FAIL**, then implement:
  - `silent()` loses `"finally" | "errors"` for flowables (Pause `finally` stays reported by `pause_gap` alone, so add `|| (t == Pause && key == "finally")` to `silent`);
  - `child_lists` gains them in `children()`/`error_children()` order;
  - delete `TWO` (`:1351`) and its uses in the cross-check.
- [ ] **Step 3: regenerate** `EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow --lib -- spec::schema`. Then `git diff --stat` and `git diff` on the file. Expected: `finally` on 8 flowables and `errors` on 3 go from `ignored` to `honoured`, and Pause `finally` goes from `ignored` to `refused`. Nothing else may move. Update the counts in `spec/schema/README.md` from the file. Run `--test flow_schema`.
- [ ] **Step 4: mutation.** Remove `finally` from `child_lists`' `Sequential` arm: the first test goes red. Restore.
- [ ] **Step 5: ratchet** (`LIVE`/`VALIDATE_REFUSED` unchanged; record) **and full-suite gate.**
- [ ] **Step 6: commit** `feat(workflow): the key walker reads into finally and the loops' errors`.

---

### Task A8: check Part A's banks and constants, write the #2a docs (EN). PR #2a boundary.

**Files:** `oracle-exec/baseline.txt`, `tests/executor.rs`, `oracle-exec/README.md`, `oracle-exec/handler_surface.py`, `oracle-exec/handler-surface.txt`, docs.

- [ ] **Step 1:** Check the bank total against spec §7.1. Expected: 13 entries banked across A4 and A6, so `(301, 242, 59)`. Record the measured figure and every expected entry that did not bank.
- [ ] **Step 2:** Re-read every constant moved in A4 and A6 against the printout. Fix any that drifted, with a dated comment.
- [ ] **Step 3:** Rewrite the baseline comment of every entry that did not bank with its remaining cause.
- [ ] **Step 4: docs (EN), as dated notes; nothing deleted:**
  - this spec: a Status line for #2a, and a dated "as built" note under each section the build changed;
  - roadmap `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md` §2:
    - the split, with row #2's measured banks;
    - `Pause` `finally:` moved to #9 (D14);
    - a note under the 2026-10-26 note 3 that its `snapshot.source` premise is false (spec "False premises" 1);
    - Rule 2's correction (premise 2);
    - #6 gains flow-level `outputs:` evaluation (spec §7.1);
    - #7 must remove `errors`/`finally`/`afterExecution` runs on restart (spec §9);
  - `docs/en/specs/2026-10-19-workflow-no-silent-drops-design.md` §10: a dated note that #2a closed the `finally`/`errors` follow-up, and that the snapshot instruction was a false premise;
  - `docs/en/specs/2026-09-01-workflow-programme-design.md` §9: a dated note for #2a.
- [ ] **Step 5: ratchet and full-suite gate**, both green.
- [ ] **Step 6: commit** `docs(workflow): package #2a as built` (plus `test(workflow): …` if Step 2 fixed a constant).
- [ ] **Step 7: hand back to the controller for the whole-branch review and PR #2a.** Part B starts on top of it.

---

## Part B — PR #2b: `afterExecution:` and Rule 2

### Task B1: the post-terminal phase in `advance` (D8, D10)

**Files:**
- Create: `src/executor/after_execution.rs`.
- Modify: `src/executor/mod.rs` (`advance`'s early return; `pub use`), `src/executor/plan.rs` (`Plan::after_execution`, flattened last, then drained), `src/worker/spawn.rs` (`find_task` also walks `after_execution`).

**Interfaces:**

```rust
/// Kestra `ExecutorService.handleAfterExecution` (`ES:1005-1023`): once the
/// execution is terminal, run `afterExecution:` one task at a time. The
/// execution's state is never touched (`ES:1026`): no `SetExecutionState`, no
/// retry (spec D11), no pause (D12).
pub(crate) fn drive(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command>;
/// Kestra `ExecutionService.isTerminated(flow, execution)` (`ExS:1325-1338`).
pub fn terminated(plan: &Plan, state: &ExecutionState) -> bool;
/// Terminal and not [`terminated`]: the phase is running.
pub fn drives_after(plan: &Plan, state: &ExecutionState) -> bool;
```

- [ ] **Step 1: failing tests** (`end_hooks_drive_tests`; extend `run` so that it keeps calling `advance` after the execution is terminal until there is nothing to do, and returns the execution's history):

```rust
/// golden `after-execution-error`: CREATED RUNNING FAILED, and `end` runs after.
#[test]
fn after_execution_runs_after_the_execution_is_terminal_and_changes_nothing() {
    let (plan, state, flow) = run(AFTER_ERROR, &["fail"]);
    assert_eq!(state.history, [RunState::Created, RunState::Running, RunState::Failed]);
    assert_eq!(order(&plan, &state), ["fail", "hello", "end"]);
    assert_eq!(last_of(&plan, &state, "end"), Some(RunState::Success));
    assert_eq!(flow, RunState::Failed);
    assert!(terminated(&plan, &state));
}

/// A failing after-task does not change the execution (ES:1026), and is not retried (D11).
#[test]
fn a_failing_after_task_changes_nothing_and_is_not_retried() { /* retry on the task; one attempt, FAILED; history unchanged */ }

/// D10: terminal is not terminated while the list runs.
#[test]
fn terminated_waits_for_the_after_list() { /* step until `end` is RUNNING: last state terminal, terminated == false, drives_after == true */ }

/// D8: KILLING runs nothing; KILLED runs the list (ES:192-193, ES:1015).
#[test]
fn after_execution_runs_once_killed_and_not_while_killing() { /* apply kill_leaves; assert no after run while KILLING; after KILLED, `end` created */ }

/// No round that makes the execution terminal also starts the phase, and a RETRYING round never does.
#[test]
fn a_retrying_execution_does_not_start_after_execution() { /* flow retry RETRY_FAILED_TASK: no `end` run before the final FAILED */ }

/// No command of the phase writes the execution.
#[test]
fn the_phase_emits_no_execution_state_retry_or_pause() { /* drive over every corpus flow with afterExecution: (plan_of), with a terminal history, asserting the command kinds */ }
```

- [ ] **Step 2: run, expect FAIL**, then implement. Run the A1 prefix test (`after_execution` is appended last, so it stays green) and `cargo test -p egret-workflow --lib -- executor`.
- [ ] **Step 3: mutation:**
  - (a) Start the phase in the same round as the terminal: `a_retrying_execution…` or the history assertion goes red.
  - (b) Call `retry::scan` in the phase: `a_failing_after_task…` goes red.
  - (c) Run the phase under `KILLING`: the kill test goes red.

  Restore each.
- [ ] **Step 4: ratchet.** The oracle runs through the worker, so no Part B entry is expected to bank before B3. If one does, bank it here as in A4. **Full-suite gate.**
- [ ] **Step 5: commit** `feat(workflow): afterExecution runs once the execution is terminal`.

---

### Task B2: what is refused or warned inside `afterExecution:` (D11, D12)

**Files:** `src/spec/validate.rs` (`validate_for_run` gains the refusal), `src/spec/keys.rs` (the warning).

**Messages (egret's own):**
- `unsupported_after_execution_task` (error), path `afterExecution.<id>` (nested: the task's path): ``task '<id>' is a <Pause|Exit> inside afterExecution, which egret does not run: the execution has already ended, and a <Pause> would pause it again (<an Exit> would set its state). Roadmap package <#9|#20> lifts it``.
- `not_honoured` (warning) on a `retry:` reaching an `afterExecution:` task, path `<task path>.retry` (or `retry` for the flow's): ``an afterExecution task is not retried on egret: the execution has already ended. Roadmap package #13 lifts it``.

- [ ] **Step 1: failing tests:** `a_pause_inside_after_execution_is_refused_at_start` (through `build_plan`), `an_exit_nested_in_an_if_inside_after_execution_is_refused`, `a_retry_inside_after_execution_warns_thirteen`, `a_flow_retry_with_after_execution_warns_thirteen`, `a_pause_in_tasks_is_not_touched_by_this_rule`.
- [ ] **Step 2: run, expect FAIL**, then implement. Walk `flow.after_execution` with `Task::children()`/`error_children()` recursively.
- [ ] **Step 3: census.** Ratchet: `LIVE`/`VALIDATE_REFUSED` unchanged (spec §7.2). Record.
- [ ] **Step 4: mutation.** Only walk the top level: the nested-`Exit` test goes red. Restore.
- [ ] **Step 5: full-suite gate. Commit** `feat(workflow): Pause and Exit inside afterExecution are refused; retry there is warned`.

---

### Task B3: the worker runs the phase; the parent hears at "terminated" (D10)

**Files:** `src/worker/claim.rs` (the settle tail's `terminal:` at `:3487-3497`), `src/worker/render.rs:1504`, `tests/end_hooks.rs` (new; copy the setup of an existing DB test file such as `tests/kill_route.rs`).

- [ ] **Step 1: measure.** List every gate in `worker/render.rs` and `worker/claim.rs` that reads `drives_forward` or `is_terminal()` on the execution: `grep -n "drives_forward\|history.last().*is_terminal\|is_terminal()" src/worker/render.rs src/worker/claim.rs`. Classify each one: widen to `drives_forward || drives_after`, or keep, with the reason. Record the table in the report.
- [ ] **Step 2: failing DB tests** (`tests/end_hooks.rs`):
  - `an_after_execution_leaf_is_claimed_and_run_after_the_execution_ended`. Assert the execution's `state`, `history` and `ended_at` are unchanged by the after-run's settle, and that the after-run's row reaches `SUCCESS`.
  - `the_flow_trigger_event_is_written_once_at_the_terminal_state`: one `flow_trigger_events` row for the terminal state, none for the after-run.
  - `a_subflow_parent_hears_the_child_only_after_its_after_execution`. The child has `afterExecution: [Sleep PT0S]`; the parent's `Subflow` run stays `RUNNING` until the child's after-run is terminal (`DE:752-759`).
  - `a_set_variables_inside_after_execution_runs` (the widened `render_updates` gate).
  - `an_if_inside_after_execution_renders_and_runs` (golden `after-execution-flowable`).
- [ ] **Step 3: run, expect FAIL**, then implement. `Settled.terminal` becomes `Some(last)` when `executor::terminated(plan, after)` and not `terminated(plan, before)`. A forced settle (`forced: true`) keeps today's rule, because it has no plan.
- [ ] **Step 4: mutation.** Report the parent at the terminal state again: the subflow test goes red. Restore.
- [ ] **Step 5: ratchet, and bank in this task as in A4** (expected: the 8 Part B entries of spec §7.1, with `EX:7527`'s kill-name list emptied and a dated comment: `flow-concurrency-queue-killed-after-execution` banked, and nothing was killed in its harvest). Measure that `oracle-exec/driver.rs`'s tick loop (`:1160-1241`) runs the after-runs: an entry like `after-execution-error` should now report "banked". If it does not, read the driver's stop condition and report before changing it. **Full-suite gate.**
- [ ] **Step 6: commit** `feat(workflow): the worker runs afterExecution, and a parent hears its child once it is terminated`.

---

### Task B4: the remaining kinds inside `afterExecution:` (D12's measurement)

**Files:** `tests/end_hooks.rs`.

- [ ] **Step 1:** One DB test each: a `Subflow`, a `Loop` (values `[1, 2]`), a `LoopUntil` (two iterations) and a `WorkingDirectory` (a `core.log` child; scripts are not needed) inside `afterExecution:`. Each runs to the end: every after-run terminal, and the execution's history unchanged. Use the worker loop the other DB tests use, with a bounded number of ticks, so a stall fails rather than hangs.
- [ ] **Step 2:** For any kind that stalls or errors, do not patch around it. Add the kind to B2's refusal (`unsupported_after_execution_task`, naming #2's follow-up row: ask the controller which number), keep the failing test as `#[ignore = "refused since package #2b: …"]` with the reason, and report.
- [ ] **Step 3: ratchet and full-suite gate. Commit** `test(workflow): Subflow, Loop, LoopUntil and WorkingDirectory inside afterExecution`.

---

### Task B5: Rule 2 — `workflow.execution_is_idle` (D13, spec §6)

**Files:**
- Create: `migrations/0210_workflow_execution_is_idle.sql` (spec §6's SQL, verbatim).
- Modify: `src/worker/commands.rs` (`run_liveness`'s second query `:2591-2597`; the doc at `:2669-2671`), `src/storage/wd_sweep.rs:163-169`, `src/storage/prune.rs` (the "has not ended" set).
- Test: `tests/migration_0210_execution_is_idle.rs` (new, modelled on `tests/migration_0197_wd_swept.rs`), plus the sweep tests.

**Interfaces:** the superseded query becomes:

```sql
SELECT e.id,
       GREATEST(COALESCE(e.ended_at, e.updated_at),
                COALESCE((SELECT max(r.ended_at) FROM workflow.task_runs r WHERE r.execution_id = e.id),
                         COALESCE(e.ended_at, e.updated_at))),
       now()
  FROM workflow.executions e
 WHERE e.id = ANY($1) AND workflow.execution_is_idle(e.id)
```

The `_wd` selection gains `AND workflow.state_is_terminal(t.state)`. The pruner's "an execution that has not ended" becomes `NOT workflow.execution_is_idle(e.id)`. Re-read `prune.rs` first and change only the predicate that decides "ended".

- [ ] **Step 0: check the number.** `ls migrations | tail -3`. If 0210 is taken on main or by an open PR the controller names, renumber above both (memory: migration numbers are a shared namespace).
- [ ] **Step 1: failing tests:**
  - `execution_is_idle_is_false_while_a_run_is_live_after_the_end`, `…_true_when_terminal_and_every_run_terminal`, `…_false_while_running` (DB; insert rows directly as `migration_0197_wd_swept.rs` does).
  - `a_superseded_kept_container_is_not_judged_while_after_execution_runs`: a `run_liveness` call with an execution that is terminal and has a live run. Its missing-row run is not in `superseded`.
  - `the_superseded_retention_runs_from_the_last_runs_end`.
  - `the_wd_sweep_leaves_a_live_block_of_a_terminal_execution` (a `RUNNING` block, `updated_at` 25 h old). Keep `the_wd_sweep_removes_a_force_closed_blocks_snapshots` green.
  - `the_pruner_keeps_an_execution_whose_after_execution_runs`.
- [ ] **Step 2: run, expect FAIL**, then implement. Rewrite `sweep_decision`'s premise paragraph (`commands.rs:2669-2671`). The premise is now "idle", the predicate is named, and the paragraph says that `Exit` siblings already broke the old premise (spec False premise 2).
- [ ] **Step 3: mutation.** Revert each reader to `state_is_terminal(e.state)` in turn; its test goes red. Restore.
- [ ] **Step 4:** `cargo test -p egret-workflow --lib -- worker::commands worker::k8s storage`, `--test migration_0210_execution_is_idle`, the live Docker sweep cases only if Docker is up (record whether they ran), **the ratchet and the full-suite gate.**
- [ ] **Step 5: commit** `feat(workflow): sweeps and the pruner read execution_is_idle, not terminal (roadmap Rule 2)`.

---

### Task B6: the execution pages follow the run until it is idle (D15)

**Files:**
- `frontend/src/components/workflows/executionState.ts`: `executionLive(execution, runs)`.
- `frontend/src/components/workflows/executions/polling.ts`: `executionFinished`.
- `frontend/src/components/workflows/executions/topology/ExecutionTopologyTab.tsx:67-79`, `gantt/ExecutionGanttTab.tsx:91`, `timing.ts:50`.
- Their `__tests__`.

Read `docs/en/frontend-rules.md` first. This task changes data rules only: no new component, no copy, no API regeneration.

- [ ] **Step 1: failing vitest:**
  - `executionFinished` is false for `{state: "FAILED", runs: [{state: "RUNNING"}]}` and true for `{state: "FAILED", runs: [{state: "SUCCESS"}]}`;
  - the topology tab keeps its `refetchInterval` while an after-run is live;
  - the Gantt bar of a live after-run keeps growing (`timing.ts`).
- [ ] **Step 2: run, expect FAIL** (`npm --prefix frontend run test -- executions`), then implement.
- [ ] **Step 3: mutation.** Ignore `runs` in `executionLive`: the polling test goes red. Restore.
- [ ] **Step 4:** `npm --prefix frontend run lint` and `npm --prefix frontend run typecheck` (or the `just` targets the repo uses: `grep -n "frontend" justfile`). Then the Rust full-suite gate (unchanged code, still required).
- [ ] **Step 5: commit** `feat(workflows-ui): execution pages keep following while afterExecution runs`.

---

### Task B7: check Part B's banks and constants, write the #2b docs (EN). PR #2b boundary.

- [ ] **Step 1:** Check the Part B bank total against spec §7.1. Expected: `(301, 250, 51)`. Record the measured figure and every expected entry that did not bank, and rewrite its baseline comment with the remaining cause.
- [ ] **Step 2:** Re-read every constant moved in B3 against the printout, as in A8.
- [ ] **Step 3: docs (EN):**
  - this spec's as-built notes;
  - roadmap row #2: done, with the measured banks;
  - the debt spec §4.7: a dated note that `sweep_decision` now reads `execution_is_idle`;
  - programme §9: a dated note for #2b;
  - `oracle-exec/README.md`: the end-hooks section, with the entries that did not bank and why.
- [ ] **Step 4: ratchet and full-suite gate**, both green. **Commit** `docs(workflow): package #2b as built`.

---

### Task B8: docs in Vietnamese, and the platform docs

- [ ] **Step 1:** `docs/vi/specs/2026-10-26-workflow-end-hooks-design.md` and `docs/vi/plans/2026-10-26-workflow-end-hooks.md` mirror EN section by section. Their header says EN wins. Mirror the notes added to the VI roadmap, the VI no-silent-drops spec §10, the VI debt spec and the VI programme spec.
- [ ] **Step 2:** The `egret-data` Platform docs (repo `CLAUDE.md`'s docs rule), on the workflow page for flows, explain:
  - `finally:` on every block;
  - `afterExecution:` runs after the execution has ended, cannot be killed, is not retried, and refuses `Pause`/`Exit`;
  - the execution page keeps updating until the after-tasks finish.

  *Correction, 2026-10-28 (Task B7, Q3 as built):* "cannot be killed" above is superseded. The Platform docs must say instead: the Kill button stays on an ended execution while its `afterExecution:` tasks run, and a kill stops them without changing the execution's own state; a kill that cannot apply yet answers `kill_not_applied` (retry); a parent's kill also stops a child's running `afterExecution:`. `Pause`, `Exit` and `LoopUntil`/`WaitFor` are refused inside `afterExecution:`, and a `retry:` there is warned. The page follows the after-tasks for at most 30 minutes after the end.

  This change goes to that repo's docs PR; the controller opens it.
- [ ] **Step 3: commit** `docs(workflow): package #2 in Vietnamese`.

## Final gate (controller, after the whole-branch review of each PR)

On the branch rebased onto fresh `origin/main`, with the Global Constraints' environment, one command at a time:
1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test -p egret-workflow --no-fail-fast`. Ratchet: after #2a, 301 / 59 / 59 / 0 expected (the measured figure governs); after #2b, 301 / 51 / 51 / 0. `LIVE` 295, `VALIDATE_REFUSED` 25 and `UNKNOWN_TYPE_REFUSED` 21, unless a task measured and recorded otherwise.
4. `cargo test -p egret-server` (OpenAPI: no DTO changes, so expect no diff).
5. #2b only: `npm --prefix frontend run test`, `lint` and `typecheck`.
6. #2b only: the migration number re-checked against main and open PRs.

Known and accepted: the 7 `sql_starrocks_live` tests fail without an engine, and `runtime::a_cancelled_token_stops_both_tickers` can flake under load (it passes alone).
