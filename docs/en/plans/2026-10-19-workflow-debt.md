# Workflow debt package — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** `docs/en/specs/2026-10-19-workflow-debt-design.md`. Read §1, the item table, first. Its default decisions are D1–D13; the plan adds none. The user answered the spec's two open questions on 2026-10-19:
- PR-1 Task 2's deadline reaches every kept container whose run is terminal (normal ends included).
- PR-2 Task 2.5 is built: publish accepts a reference absent from the caller's workspace, with Kestra's warning.

**Goal:** close the debt that groups B (kill), C1, C2 and WorkingDirectory left open, before the Kestra-parity roadmap starts. The debt is:
- a killed `delete: false` container is kept, and swept after a deadline;
- a kill that cannot load its execution closes it instead of looping;
- the `_wd` crash sweep, and the flake-335 guard;
- size caps and the Kestra alignments M5 and M7;
- the small code and doc minors.

**Architecture:** two PRs, built in this order. They can run in parallel worktrees under the autonomy rules.
- **PR-1, kill and WorkingDirectory** (Tasks 1.1–1.7):
  - `TaskRunner::cancel` learns the task's `RunnerSpec`. Docker stops a `delete: false` container and does not remove it.
  - The two orphan sweeps share one pure decision, which now has a deadline for kept work.
  - A new `worker::force_close` writes a terminal state in plain SQL when `claim::load` cannot plan the execution.
  - A new `storage::wd_sweep` removes a crashed block's `_wd/{block}/`, with migration 0196 as its done mark.
  - The oracle setup's `TRUNCATE` is widened.
- **PR-2, C2, C1 and docs** (Tasks 2.1–2.6):
  - caps in `executor/update.rs::compute`;
  - `render_updates` builds its contexts before it computes;
  - `persist` gives a restarted run a fresh id;
  - the minors;
  - the conditional publish change;
  - the docs.

**Tech stack:** Rust (sqlx, tokio, axum), Postgres, the Docker CLI for the live cases, Python 3 (`handler_surface.py`, only if PR-2 Task 5 moves census counts).

**Branches and worktrees:**
- PR-1: `feat/workflow-debt-kill-wd`.
- PR-2: `feat/workflow-debt-c2-c1`.
- Both start from `origin/main` after C2 (#693) merges, each in its own worktree with a private Postgres, Valkey and target directory.
- PR-1 is built first, in `data-portal/egret-debt`, on Postgres `egret-debt-postgres` :5455 and Valkey `egret-debt-valkey` :6455. It was branched from the C2 head `4942843d` while #693 awaited its merge, and is moved onto `origin/main` with `git rebase --onto origin/main 4942843d` once #693 lands. PR-2 follows PR-1, on the same containers.
- The `path:line` citations were taken at `481d9101`. `4942843d` only renamed `TaskKind::Update` to `TaskKind::ExecutionUpdate` (the SQL guard in `tests/execution_timestamps.rs` read the old name as an `UPDATE`), which can shift a cited line by one or two.
- Migration **0196** is free today: `ls migrations | tail -1` is `0195_workflow_execution_variables.sql`. Re-check it with `git ls-tree origin/main migrations/ | tail -3` immediately before PR-1 Task 5, and again after every rebase (see the memory note *migration numbers are a shared namespace*).

## Facts measured for this plan (2026-10-19, at `481d9101`)

Paths are relative to `crates/egret-workflow/src/` unless another root is given.

- **Cancel and release.**
  - `TaskRunner::cancel(task, grace)` is declared at `worker/commands.rs:641` and `release(task, spec)` at `:651`.
  - Docker's `cancel` (`:1617-1643`) ends in `rm --force` whatever the spec says. Its `release` (`:1648-1656`) returns early on `!spec.delete`.
  - The Kubernetes `cancel` (`worker/k8s/runner.rs:1417-1438`) deletes the pod. Its `release` (`:1442-1450`) signals `ended` for a kept pod.
  - Callers of `runner.cancel`:
    - `worker/handlers/script.rs:774-776`, grace 0;
    - `worker/handlers/kestra.rs:802-807`, grace `kill_grace_s`;
    - the test spy at `worker/handlers/container.rs:348-355`.
  - The script handler's `release` re-parses the spec (`script.rs:786-797`); copy that.
- **The sweeps.**
  - `parse_docker_ps` (`worker/commands.rs:2563-2577`) drops `delete=false`. Its unit test, `the_docker_sweep_parses_names_and_leaves_delete_false_alone`, is at `:3149-3161`.
  - `run_liveness` (`:2456-2487`) answers `terminal` and `missing` only.
  - The pod sweep skips kept pods at `worker/k8s/runner.rs:1503-1511`.
  - The reaper tick calls the sweep (`runtime.rs:661-683`). The runners are built in `runtime::handlers_for` (`runtime.rs:750-778`).
- **Config.** `WorkflowConfig` is at `crates/egret-config/src/lib.rs:593`. Two fields to copy the shape of:
  - `storage_retention_days` (`:740-745`, default fn `:1005-1007`);
  - `kill_poll_ms` (`:690-702`).

  Workflow tuning keys are deliberately not chart values (`deploy/helm/egret/templates/configmap-env.yaml:44-51`).
- **The live kill cases.** `tests/container_docker_live.rs` has `kill_a_running_sleeper` (`:2418-2457`) and `a_killed_container_is_stopped_by_the_cancel_even_when_delete_is_false` (`:2505-2530`, which asserts the container is GONE: this plan flips it). The cases are listed in `daemon_cases()` (`:2641-2670`), and the helpers `container_running` and `container_exists` are in the file.
- **The kill paths.**
  - `commit_kill` (`worker/claim.rs:2409-2441`) retries one `40P01`.
  - `commit_kill_with` calls `load` first (`:2450-2453`).
  - `finish_claimed_kill` (`:1700-1736`) sends a row-content fault (`row_content_fault`, `:1740-1742`) to `commit_kill`.
  - `load` (`:4043-4136`) fails `Unplannable` at `:4078-4080`.
  - `WorkerError::is_row_content` is at `worker/mod.rs:822-830`.
  - Route: `routes.rs:3055-3098`, with the audit at `:3085-3096`.
  - Kill transaction: `worker/kill.rs` `kill_execution` (`:123-142`), `kill_in` (`:144-172`), `descendants` (`:191-215`) and `kill_loaded` (`:238-258`).
- **The deadlock fixture to copy** is `tests/kill_route.rs:722-830`. That file's helpers:
  - `setup`, `file_flow`, `start`, `subflow_child`, `run_row`, `exec_state`, `kill`;
  - `CancelRecorder` (`:190-222`), `last_attempt_state`.

  Its tests are `#[sqlx::test(migrations = "../../migrations")]`.
- **`_wd`.**
  - `working_dir_prefix` is at `storage/mod.rs:232-242`.
  - `discard` is at `worker/working_dir.rs:953-966`. It is called at `worker/claim.rs:1786` (discard-only) and `:1829`.
  - The staging sweeper to model on is `storage/nsfiles/staging.rs:118-206`, tested at `tests/nsfiles_store.rs:2700-2795`. It is spawned at `crates/egret-bin/src/main.rs:1928-1946`.
  - A `_wd` test fixture with a `MemStore` is `tests/working_dir_claims.rs:149-200` (`KILLED_BLOCK`, `install_mem_for_tests`, list by key).
- **Flake 335.**
  - The assertion is at `oracle-exec/driver.rs:341-348`.
  - The init is `egret_side()` at `tests/executor.rs:1452-1477`, inside a `tokio::sync::OnceCell`. It truncates only `workflow.executions` (`:1469`).
  - One pass files 351 − 6 − 4 = 341 rows (`driver.rs:799,803,1135`). Over the 345-file corpus that was 335.
- **C2.**
  - `compute` is at `executor/update.rs:85-112`. `current` is cloned for every op at `:94`.
  - `set` is at `:124-149`, `set_labels` at `:220-281`, `deep_merge` at `:169`.
  - The tests start at `:308` and use `cx()` and `obj()`.
  - `render_updates` is at `worker/render.rs:1470-1545` (doc `:1459-1460`), and `reserve_id` at `:678-685`.
  - `RestartLoop` is applied at `executor/state.rs:986-1002`.
  - The double lookup is at `executor/plan.rs:1365-1367`. The misnamed test is at `spec/unrunnable.rs:144`.
  - The M5 test is `tests/worker.rs:11538-11573`. The helpers `seed`, `start_updates`, `drain`, `core_handlers` and `execution_variables` are at `:117,11236,855,11258,11242`.
- **Caps that exist:**
  - `runners::env::MAX_OUTPUT_BYTES` = 1 MiB (`runners/env.rs:204`);
  - `labels::MAX_BACKFILL_LABELS` = 50, `MAX_LABEL_KEY_CHARS` = 128 and `MAX_LABEL_VALUE_CHARS` = 1024 (`labels.rs:6-8`), with their messages at `labels.rs:59-68`.
- **Publish.**
  - Subflow resolution is at `publish.rs:433-450` and `dependsOn` at `:532-558`.
  - The unscoped lookup is `FlowRepo::find_by_name` (`repo.rs:929-946`).
  - A scoped query shape is at `repo.rs:878-890`.
  - Runtime resolves by name in the execution's workspace (`worker/subflow.rs:1100-1117`).
  - The four security tests are at `tests/publish.rs:62,95,279,308`.
- **Oracle baseline** (C2's final printout): 301 golden, 73 mismatching / 73 baselined, 0 broken; `LIVE` 293 (`oracle-exec/driver.rs:951`), `VALIDATE_REFUSED` 27, `UNKNOWN_TYPE_REFUSED` 21.

## Global Constraints

- **The private stack only.** Never use `:5432`, `deploy/docker-compose.dev.yml`, `egret-dev-*` containers or shared buckets.
  - Every DB command runs with `DATABASE_URL=postgres://egret:egret@localhost:5455/egret_test` exported, and with `EGRET_ALLOW_DB_SKIP` unset.
  - Valkey, where needed, is `localhost:6455`.
  - The target directory is `CARGO_TARGET_DIR=<WORKTREE>/target`.
  - The controller fills `5455`, `6455` and `<WORKTREE>`.
- **Destructive tests touch only `MemStore` or a temporary database.** A red test of a delete or sweep must never reach a real bucket.
- **Docker live cases need a daemon and the pinned images.** They never `docker image prune`. A container the test creates is named by the test's own run ids.
- **One cargo command at a time.** Never `pkill` cargo. `rg` is not installed: use `grep -rn`.
- **The oracle ratchet stays green after every task:** `cargo test -p egret-workflow --test executor`.
  - Record the before and after numbers in the task report: mismatching, baselined, broken, LIVE.
  - `oracle-exec/golden.json` is never edited.
- **Run only the tests for what you touch.** The full suites run once, at the final gate.
- **Never commit `.superpowers/`.**
- **Commits** follow Conventional Commits and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Messages are verbatim**, as the spec gives them. Each new message is egret's own, and its doc comment says so.
- **Mutation discipline.** For each new guard, break it once by hand, see a named test go red, and restore it. The task report records the command and the red line.

## File map

| File | Responsibility | Task |
|---|---|---|
| `src/worker/commands.rs` | `TaskRunner::cancel(.., spec)`; Docker keep path; `parse_docker_ps` keep flag; `run_liveness` ended; `sweep_decision`; Docker sweep | 1.1, 1.2 |
| `src/worker/k8s/runner.rs` | `cancel` signature + doc (D2); pod sweep through `sweep_decision` | 1.1, 1.2 |
| `src/worker/handlers/{script,kestra,container}.rs` | pass the spec to `cancel`; spy records it | 1.1 |
| `crates/egret-config/src/lib.rs`, `src/runtime.rs` | `kept_container_retention_days`; runner wiring | 1.2 |
| `tests/container_docker_live.rs` | flip the kept case; add kept-then-swept | 1.1, 1.2 |
| `tests/kill_route.rs` | `commit_kill` deadlock test; force-close tests | 1.3, 1.4 |
| `src/worker/force_close.rs` (new), `src/worker/{kill,claim,mod}.rs`, `src/routes.rs` | force-close and its three call sites; audit `forced` | 1.4 |
| `migrations/0196_workflow_task_run_working_dir_swept.sql` (new) | `working_dir_swept_at` | 1.5 |
| `src/storage/wd_sweep.rs` (new), `src/storage/mod.rs`, `src/worker/working_dir.rs`, `crates/egret-bin/src/main.rs` | the sweep; `discard` stamps | 1.5 |
| `tests/working_dir_claims.rs` | sweep tests | 1.5 |
| `tests/executor.rs` | `TRUNCATE workflow.flows` in `egret_side` | 1.6 |
| docs (PR-1) | kill spec, WorkingDirectory spec and plan (EN+VI), programme §9 | 1.7 |
| `src/executor/update.rs` | caps; `current` per arm; tests | 2.1, 2.4 |
| `src/worker/render.rs` | contexts first (M5) | 2.2 |
| `src/worker/claim.rs` (`persist`), `src/worker/render.rs` (`Surface`) | fresh id for a restarted run (M7) | 2.3 |
| `tests/worker.rs` | M5 test update; M7 test | 2.2, 2.3 |
| `src/executor/plan.rs`, `src/spec/unrunnable.rs` | minors | 2.4 |
| `src/publish.rs`, `src/repo.rs`, `tests/publish.rs`, `oracle-exec/{driver.rs,README.md}` | scoped resolution (conditional) | 2.5 |
| docs (PR-2) | C1/C2 specs (EN+VI), schema README, programme §9 | 2.6 |

---

# Part 1 — PR-1: kill and WorkingDirectory

### Task 1.1: a killed or timed-out `delete: false` container is stopped, not removed (D1, D2)

**Files:**
- Modify: `src/worker/commands.rs`:
  - the trait at `:621-641`, which gains `spec: &crate::scripts::RunnerSpec`;
  - `DockerRunner::cancel` at `:1617-1643`;
  - `release` at `:1648-1656`, whose doc is updated.
- Modify: `src/worker/k8s/runner.rs:1398-1438`: the signature, and a doc sentence for D2 ("a pod cannot be stopped without being deleted; `spec.delete` is not read here").
- Modify: `src/worker/handlers/script.rs:774-776`: parse as `release` does (`:786-797`); a task that does not parse, or names another runner, uses `RunnerSpec::defaults(self.rt.runner_kind)`.
- Modify: `src/worker/handlers/kestra.rs:802-807`: pass `&RunnerSpec::defaults(self.rt.runner_kind)`.
- Modify: `src/worker/handlers/container.rs:348-355`: the spy records `spec.delete` in a new `cancel_deletes: Mutex<Vec<bool>>`.
- Modify: `src/worker/claim.rs:2343-2356`: `finish_kill`'s doc paragraph "On the runners egret has, the cancel already removed the container…" is rewritten to D1 and D2.
- Test: `script.rs` tests; `tests/container_docker_live.rs`.

**Interfaces:**
- Produces: `async fn cancel(&self, task: &TaskInput, grace: Duration, spec: &RunnerSpec) -> Result<()>`. Task 1.2 does not depend on it.

- [ ] **Step 1: write the failing unit test** in `worker/handlers/script.rs`'s tests, next to `release_passes_the_tasks_delete_to_the_configured_runner_only`:

```rust
/// **`cancel` hands the runner the task's own `delete`** (debt D1), as
/// `release` does: Docker then keeps a `delete: false` container, stopped.
/// A task naming the other runner, or one that does not parse, cancels with
/// the configured runner's defaults (`delete: true`), as before.
#[tokio::test]
async fn cancel_passes_the_tasks_delete_to_the_runner() {
    // Arrange
    let spy = Spy::ok(CommandsResult::default());
    let h = open(spy.clone());
    let mut kept = alias_spec();
    kept["taskRunner"] = serde_json::json!({"type": scripts::DOCKER_RUNNER, "delete": false});
    let mut other = alias_spec();
    other["taskRunner"] = serde_json::json!({"type": scripts::KUBERNETES_RUNNER});
    // Act
    h.cancel(&input(scripts::ALIAS_CONTAINER, alias_spec())).await.expect("cancel");
    h.cancel(&input(scripts::ALIAS_CONTAINER, kept)).await.expect("cancel");
    h.cancel(&input(scripts::ALIAS_CONTAINER, other)).await.expect("cancel");
    // Assert
    assert_eq!(
        spy.cancel_deletes.lock().unwrap().as_slice(),
        [true, false, true]
    );
}
```

- [ ] **Step 2: run it, expect a compile failure** (no `cancel_deletes`, and `cancel` takes no spec):

```bash
CARGO_TARGET_DIR=<WORKTREE>/target cargo test -p egret-workflow --lib -- handlers::script::tests::cancel_passes
```

- [ ] **Step 3: change the trait and the four implementations.** Docker's keep path, placed before today's `if !grace.is_zero()`:

```rust
if !spec.delete {
    // Debt D1 (2026-10-19), Kestra's `Docker.cleanupContainer`: kill, then
    // remove only when `delete` is true. `docker stop -t 0` is an immediate
    // SIGKILL; a grace gives the plugin host its SIGTERM first.
    let name = container_name(task);
    if let Err(e) = self.stop(task, &name, grace).await {
        // An absent or already-stopped container is not an error (measured
        // in Step 5); anything else is the daemon's, surfaced as before.
        return Err(e);
    }
    return Ok(());
}
```

  Read `DockerRunner::stop` (`worker/commands.rs:1658-…`) first. If it treats grace zero as "nothing to do", give it an explicit `-t 0` path. Do not add a second stop function.
- [ ] **Step 4: run the unit tests**: `cargo test -p egret-workflow --lib -- handlers::script handlers::kestra handlers::container worker::commands`. Expect PASS.
- [ ] **Step 5: measure `docker stop` on the daemon** before the live case relies on it. Record each exit code and stderr in the report; Step 3's error handling follows what is measured:

```bash
docker run -d --name debt-stop-probe alpine:3.20 sleep 600 && docker stop -t 0 debt-stop-probe; echo "running→$?"
docker stop -t 0 debt-stop-probe; echo "exited→$?"
docker rm debt-stop-probe && docker stop -t 0 debt-stop-probe; echo "absent→$?"
```

  Use the image the live suite already pins (`SHELL_IMAGE`), not a new pull.
- [ ] **Step 6: flip the live case.** Rename `a_killed_container_is_stopped_by_the_cancel_even_when_delete_is_false` to `a_killed_delete_false_container_is_stopped_and_kept`, in the function and in `daemon_cases()`. Its asserts become:

```rust
assert!(!container_running(name), "{name} is still running after its run was KILLED");
assert!(container_exists(name), "{name} was removed: `delete: false` keeps a killed container (debt D1)");
killed.assert_the_worker_cancelled_once();
// The case removes what it kept, so the daemon is left as it was found.
let _ = std::process::Command::new("docker").args(["rm", "--force", name]).status();
```

  Run it: `cargo test -p egret-workflow --test container_docker_live -- a_killed_delete_false`. Expect RED before Step 3 and PASS after; record both.
- [ ] **Step 7: run the ratchet** (`cargo test -p egret-workflow --test executor`). Expect no change. Then commit `feat(workflow): a killed delete:false container is stopped and kept, as Kestra`.

---

### Task 1.2: the kept-container deadline (D3)

**Files:**
- Modify: `crates/egret-config/src/lib.rs`:
  - the field `kept_container_retention_days: u32` after `storage_retention_days`, with a doc in its style that names `EGRET_WORKFLOW__KEPT_CONTAINER_RETENTION_DAYS=7` and "0 keeps them for ever";
  - `fn default_kept_container_retention_days() -> u32 { 7 }`.
- Modify: `src/worker/commands.rs`:
  - `RunLiveness` gains `ended: HashMap<Uuid, DateTime<Utc>>`, and `run_liveness` selects `COALESCE(ended_at, updated_at)`;
  - `parse_docker_ps` returns `(name, id, keep)`;
  - a new `pub(crate) fn sweep_decision(..)`;
  - `DockerRunner::with_kept_retention`;
  - the Docker sweep goes through `sweep_decision`.
- Modify: `src/worker/k8s/runner.rs:1485-1547`: the same decision, and `KubernetesRunner::with_kept_retention`.
- Modify: `src/runtime.rs:750-778`: map `0` to `None`, otherwise `Some(Duration::from_secs(days * 86_400))`, and pass it to both runners.
- Test: unit tests in `commands.rs`; `tests/runtime.rs` (default pinned); a live case.

**Interfaces:**
- Produces: `pub(crate) fn sweep_decision(live: &RunLiveness, id: Uuid, keep: bool, retention: Option<Duration>, now: DateTime<Utc>) -> bool` ("remove"). Both sweeps call it, and nothing else decides.

- [ ] **Step 1: write the failing unit tests** in `commands.rs`'s tests. First replace `the_docker_sweep_parses_names_and_leaves_delete_false_alone`:

```rust
/// The parser keeps every egret task container and REPORTS the keep flag;
/// the decision is `sweep_decision`'s (debt D3).
#[test]
fn the_docker_sweep_parses_names_and_the_keep_flag() {
    let a = uuid::Uuid::from_u128(1);
    let b = uuid::Uuid::from_u128(2);
    let out = format!(
        "egret-run-x-1\t{a}\t\negret-run-y-1\t{b}\tfalse\nsomething-else\t{a}\t\negret-run-z-1\tnot-a-uuid\t\n"
    );
    assert_eq!(
        parse_docker_ps(&out),
        vec![
            ("egret-run-x-1".to_string(), a, false),
            ("egret-run-y-1".to_string(), b, true),
        ]
    );
}

/// **One decision for both sweeps** (debt D3): a live run's work is never
/// touched; an ended run's is removed at once unless kept; a kept one goes
/// once its run ended more than `retention` ago, never with `None`; an
/// unknown run is left alone.
#[test]
fn sweep_decision_removes_kept_work_only_after_the_deadline() {
    use chrono::{Duration as D, Utc};
    let now = Utc::now();
    let (live, done_old, done_new, unknown) = (
        uuid::Uuid::from_u128(1),
        uuid::Uuid::from_u128(2),
        uuid::Uuid::from_u128(3),
        uuid::Uuid::from_u128(4),
    );
    let mut l = RunLiveness::default();
    l.terminal.extend([done_old, done_new]);
    l.ended.insert(done_old, now - D::days(8));
    l.ended.insert(done_new, now - D::days(6));
    l.missing.insert(unknown);
    let week = Some(std::time::Duration::from_secs(7 * 86_400));
    assert!(!sweep_decision(&l, live, false, week, now), "a live run's work stays");
    assert!(sweep_decision(&l, done_new, false, week, now), "not kept: removed at once");
    assert!(!sweep_decision(&l, done_new, true, week, now), "kept, 6 days: stays");
    assert!(sweep_decision(&l, done_old, true, week, now), "kept, 8 days: removed");
    assert!(!sweep_decision(&l, done_old, true, None, now), "retention 0: kept for ever");
    assert!(!sweep_decision(&l, unknown, false, week, now), "no row here: never ours to remove");
}
```

  And in `tests/runtime.rs`, beside the other default pins:

```rust
#[test]
fn the_kept_container_retention_defaults_to_seven_days() {
    assert_eq!(egret_config::Config::default().workflow.kept_container_retention_days, 7);
}
```

- [ ] **Step 2: run them, expect FAIL** (they do not compile):

```bash
cargo test -p egret-workflow --lib -- worker::commands::tests::sweep_decision worker::commands::tests::the_docker_sweep
cargo test -p egret-workflow --test runtime -- kept_container
```

- [ ] **Step 3: implement.** Order: config field, then `run_liveness`, then `sweep_decision`, then the parser, then the two sweeps, then the wiring. The pod sweep's kept-annotation `continue` (`k8s/runner.rs:1509-1511`) becomes a `keep` value passed to `sweep_decision`. Step 1's tests then pass.
- [ ] **Step 4: write the live case** `a_kept_container_is_swept_after_its_deadline`, and add it to `daemon_cases()`:
  - Kill a `delete: false` sleeper with `kill_a_running_sleeper` (Task 1.1 keeps it).
  - `DockerRunner::new().with_kept_retention(Some(Duration::from_secs(3600))).sweep_orphans(&w.pool)` leaves it (`container_exists` stays true).
  - Then `UPDATE workflow.task_runs SET ended_at = now() - interval '2 hours' WHERE id = $1`, and sweep again: it is gone.
  - The same with `with_kept_retention(None)`, after a day's backdate: kept.

  Run it: `cargo test -p egret-workflow --test container_docker_live -- a_kept_container`.
- [ ] **Step 5: mutation.** In `sweep_decision`, replace the kept arm with `false`, and see `sweep_decision_removes_kept_work_only_after_the_deadline` go red. Restore it.
- [ ] **Step 6: run the ratchet and commit** `feat(workflow): the orphan sweeps remove kept containers and pods after kept_container_retention_days`.

---

### Task 1.3: a test for `commit_kill`'s deadlock retry (D5)

**Files:**
- Test: `tests/kill_route.rs`, after `a_worker_that_loses_a_deadlock_to_a_kill_still_finishes_it`.

- [ ] **Step 1: write the test.** It passes at once; that is expected, because the code exists. Step 2 is its red.

```rust
/// **Debt D5: a kill-only claim whose `KILLED` commit loses a deadlock still
/// finishes it** (`claim::commit_kill`'s retry, which no test drove). The
/// child's run is `KILLING` with no lease, as a reaped kill leaves it. The
/// worker's commit locks the child, then (the child ends) its parent; the
/// test transaction holds the parent, then locks the child -- the reverse.
/// Its `deadlock_timeout` is raised so the worker is the victim. The retry
/// waits for the test's commit, finds the fence still `KILLING` under its
/// lease, and commits.
#[sqlx::test(migrations = "../../migrations")]
async fn a_kill_only_claim_that_loses_a_deadlock_still_commits_killed(pool: sqlx::PgPool) {
    // Arrange: a parent waiting on a subflow child whose run was claimed, reaped
    // and killed.
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let source = "id: kdead\nnamespace: t\ntasks:\n  - id: sub\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n";
    let parent = start(&pool, ws, source).await;
    let child = subflow_child(&pool, parent).await;
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert_eq!(claimed.len(), 1, "the child's run");
    let (run_id, attempt) = (claimed[0].id, claimed[0].attempt as usize);
    egret_workflow::worker::reap(&pool, std::time::Duration::ZERO).await.expect("reap");
    assert_eq!(kill(&app, ws, child, "?cascade=false", user).await.status(), 202);
    assert_eq!(run_row(&pool, child, "a").await.0, "KILLING");

    let mut kill_tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL deadlock_timeout = '60s'").execute(&mut *kill_tx).await.unwrap();
    let kill_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *kill_tx).await.unwrap();
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR UPDATE")
        .bind(parent).execute(&mut *kill_tx).await.unwrap();

    // Act: the kill-only claim commits KILLED and, reporting to the parent,
    // waits on it.
    let reg = std::sync::Arc::new(CancelRecorder::default());
    let worker = {
        let (pool, reg) = (pool.clone(), reg.clone());
        tokio::spawn(async move { egret_workflow::worker::tick(&pool, 10, &*reg).await })
    };
    let waited = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let blocked: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))",
            )
            .bind(kill_pid).fetch_optional(&pool).await.unwrap();
            if blocked.is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "the worker's KILLED commit never waited on the parent");
    // Close the cycle: the worker is the victim.
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR UPDATE")
        .bind(child).execute(&mut *kill_tx).await
        .expect("the lock is granted once the worker is aborted");
    kill_tx.commit().await.unwrap();

    // Assert
    let ticked = tokio::time::timeout(std::time::Duration::from_secs(30), worker)
        .await.expect("the worker returns").expect("the worker task");
    assert!(ticked.is_ok(), "the deadlock victim must retry its KILLED commit: {ticked:?}");
    let (state, claimed_at, ended_at, attempts) = run_row(&pool, child, "a").await;
    assert_eq!(state, "KILLED");
    assert_eq!(claimed_at, None);
    assert!(ended_at.is_some());
    assert_eq!(last_attempt_state(&attempts).as_deref(), Some("KILLED"));
    assert_eq!(reg.cancels(), vec![(run_id, attempt)], "cancelled once");
    assert_eq!(exec_state(&pool, child).await, "KILLED");
}
```

  The fixture assumes the kill-only claim's commit reports to the parent inside its transaction (`subflow::on_child_terminal` from `settle`). If the worker never waits on the parent, read where `commit_kill_with`'s `settle` reports, and move the test's first lock to the row it does take second. Record the reason in the test's doc.
- [ ] **Step 2: red by mutation.** In `commit_kill` (`worker/claim.rs:2418-2427`), replace the retry arm's body with `return Err(e)`, and see this test fail on `ticked.is_ok()`. Restore it. Record both runs:

```bash
cargo test -p egret-workflow --test kill_route -- a_kill_only_claim_that_loses_a_deadlock
```

- [ ] **Step 3: commit** `test(workflow): pin commit_kill's deadlock retry`.

---

### Task 1.4: force-close an execution a kill cannot load (D4)

**Files:**
- Create: `src/worker/force_close.rs` (module doc: spec §2.2, verbatim rules 1-6).
- Modify: `src/worker/mod.rs` (the `mod` line).
- Modify: `src/worker/kill.rs`:
  - `kill_in`: on a row-content fault from `claim::load`, force-close and return `KillOutcome::{Killing,Killed}` from the result, with a `forced` flag;
  - `descendants`: force-close that one descendant and continue.
- Modify: `src/worker/claim.rs:2450-2453` (`commit_kill_with`): on a row-content fault, force-close with `own = Some((claimed.id, claimed.claimed_at))`, and return `Ok(true)` when its own run was closed.
- Modify: `src/routes.rs:3075-3096`: the audit's detail gains `"forced": true` only when forced, so today's records keep their exact shape (`a_kill_writes_an_audit_record` stays green).
- Test: `tests/kill_route.rs`.

**Interfaces:**
- Produces:

  ```rust
  pub(super) async fn close(conn: &mut PgConnection, execution: Uuid, reason: &str, own: Option<(Uuid, DateTime<Utc>)>) -> Result<Closed>
  pub(super) struct Closed { pub state: RunState /* Killing | Killed */, pub reported: Vec<claim::Job> }
  ```

  The `reported` jobs are the parent reports from `spawn::on_execution_terminal` and `subflow::on_child_terminal`, run by the caller the way `kill_in` runs frames.
- `KillOutcome` gains no variant. `kill_execution` returns `(KillOutcome, bool /* forced */)` through a new `pub fn kill_execution_reporting`, so the route can audit `forced`; the existing `kill_execution` signature stays for the Docker live suite.

- [ ] **Step 1: write the failing tests** in `tests/kill_route.rs`:

```rust
/// Corrupt the stored source so `claim::load` fails `Unplannable`, the
/// row-content fault a kill used to loop on (debt §2.2).
async fn make_unloadable(pool: &sqlx::PgPool, ex: Uuid) {
    sqlx::query("UPDATE workflow.executions SET source = 'tasks: [' WHERE id = $1")
        .bind(ex).execute(pool).await.expect("corrupt the source");
}

/// **Debt D4: the route force-closes an execution it cannot load.** A reaped
/// run (no lease, no handler can be built) and a `CREATED` sibling both end
/// `KILLED` with the force-close message, the execution is `KILLED`, and the
/// audit says `forced`.
#[sqlx::test(migrations = "../../migrations")]
async fn the_route_force_closes_an_execution_it_cannot_load(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_log("unloadable")).await;
    egret_workflow::worker::claim(&pool, 10).await.unwrap();
    egret_workflow::worker::reap(&pool, std::time::Duration::ZERO).await.expect("reap");
    make_unloadable(&pool, ex).await;
    common::audit_mutations(&app);
    // Act
    let res = kill(&app, ws, ex, "", user).await;
    // Assert
    assert_eq!(res.status(), 202, "no longer a 500");
    assert_eq!(common::json(res).await["state"], "KILLED");
    let (state, claimed_at, ended_at, attempts) = run_row(&pool, ex, "a").await;
    assert_eq!((state.as_str(), claimed_at), ("KILLED", None));
    assert!(ended_at.is_some());
    assert_eq!(last_attempt_state(&attempts).as_deref(), Some("KILLED"));
    let message: Option<String> = sqlx::query_scalar(
        "SELECT message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'a'",
    ).bind(ex).fetch_one(&pool).await.unwrap();
    assert!(
        message.as_deref().is_some_and(|m| m.starts_with("egret could not load this execution (")
            && m.ends_with("so it was killed without running its flow: no errors:, finally: or Flow trigger ran")),
        "{message:?}"
    );
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
    let audit = common::audit_mutations(&app);
    assert_eq!(audit[0].2["forced"], true, "{audit:?}");
    // And nothing loops: a tick finds nothing to claim.
    finish_kills(&pool).await;
}

/// **Debt D4, the worker's side: a kill-only claim of a run whose execution
/// cannot be loaded commits `KILLED` once, instead of being reaped and
/// claimed again every `working_timeout` for ever.**
#[sqlx::test(migrations = "../../migrations")]
async fn a_kill_only_claim_of_an_unloadable_execution_closes_it(pool: sqlx::PgPool) {
    // Arrange: killed (KILLING, unleased) while loadable, then corrupted.
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_log("unloadable-claim")).await;
    egret_workflow::worker::claim(&pool, 10).await.unwrap();
    egret_workflow::worker::reap(&pool, std::time::Duration::ZERO).await.expect("reap");
    assert_eq!(kill(&app, ws, ex, "", user).await.status(), 202);
    make_unloadable(&pool, ex).await;
    // Act
    let reg = CancelRecorder::default();
    let ticked = egret_workflow::worker::tick(&pool, 10, &reg).await;
    // Assert
    assert!(ticked.is_ok(), "{ticked:?}");
    assert_eq!(run_row(&pool, ex, "a").await.0, "KILLED");
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
    assert!(reg.cancels().is_empty(), "no handler can be built, so nothing is cancelled");
    let again = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert!(again.is_empty(), "nothing is left to claim: {again:?}");
}

/// **A held leaf stays `KILLING` under its lease** (rule 2): its worker has
/// the input and can cancel; only the unleased sibling is closed now.
#[sqlx::test(migrations = "../../migrations")]
async fn a_force_close_leaves_a_held_leaf_to_its_worker(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let source = "id: held\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n\
         \x20     - id: leased\n        type: io.kestra.plugin.core.log.Log\n        message: l\n\
         \x20     - id: waiting\n        type: io.kestra.plugin.core.log.Log\n        message: w\n";
    let ex = start(&pool, ws, source).await;
    let claimed = egret_workflow::worker::claim(&pool, 1).await.unwrap();
    let held = claimed[0].task_id.clone();
    make_unloadable(&pool, ex).await;
    let res = kill(&app, ws, ex, "", user).await;
    assert_eq!(res.status(), 202);
    assert_eq!(common::json(res).await["state"], "KILLING");
    let (state, claimed_at, _, _) = run_row(&pool, ex, &held).await;
    assert_eq!(state, "KILLING");
    assert!(claimed_at.is_some(), "the lease is kept");
    let other = if held == "leased" { "waiting" } else { "leased" };
    assert_eq!(run_row(&pool, ex, other).await.0, "KILLED");
}
```

  Plus a cascade test, `a_cascade_force_closes_an_unloadable_child_and_kills_the_parent`. Use `parent_with_children`, corrupt the subflow child's source, kill the parent, and expect 202, the child `KILLED` and the parent `KILLED` after `finish_kills`.
- [ ] **Step 2: run them, expect FAIL**: `cargo test -p egret-workflow --test kill_route -- force_close unloadable`. The route answers 500 today.
- [ ] **Step 3: implement `force_close::close`** in SQL, following spec §2.2 rules 1-6. The attempt close appends `KILLED` to the last attempt's `history` when its last state is not terminal; `kill_route.rs:770-781` shows the `jsonb_set` shape. Measure how the attempt's `ended_at` is stamped: read `stamps::stamp_attempt_ends`, write the same key with the same unit, and record it in the report. The execution's `history` gets `KILLING`, then `KILLED`, unless it already ends `KILLING`; its `ended_at` is set only on `KILLED`.
- [ ] **Step 4: wire the three call sites and the audit.** The fault test is `row_content_fault` (`claim.rs:1740`); make it `pub(super)` if `kill.rs` needs it. Run Step 1's tests and the whole `kill_route` binary. Expect PASS.
- [ ] **Step 5: mutation.** In `commit_kill_with`, return the load error again, and see `a_kill_only_claim_of_an_unloadable_execution_closes_it` go red. Restore it.
- [ ] **Step 6: run the ratchet and `--test worker -- kill`, then commit** `feat(workflow): a kill closes an execution it cannot load instead of looping`.

---

### Task 1.5: the `_wd` crash sweep, migration 0196 (D6)

**Files:**
- Create: `migrations/0196_workflow_task_run_working_dir_swept.sql`.
- Create: `src/storage/wd_sweep.rs`, with `WdSweepConfig { every: 3600 s, older_than: 24 h, batch: 100 }`, `sweep(pool, fs, cfg) -> Result<u64>` and `spawn_wd_sweeper(..)`; `pub mod wd_sweep;` in `storage/mod.rs`.
- Modify: `src/worker/working_dir.rs:953-966`: `discard` stamps the column after a `delete_prefix` that succeeded. Its warn says "the `_wd` sweep will remove them", not "the pruner".
- Modify: `crates/egret-bin/src/main.rs:1928-1946`: spawn it beside the staging sweeper.
- Test: `tests/working_dir_claims.rs`; `tests/migration_0196_wd_swept.rs` (new; copy the shape of `tests/migration_0195_variables.rs`).

- [ ] **Step 0:** `git ls-tree origin/main migrations/ | tail -3`. If 0196 is taken, use the next free number everywhere in this task and in the docs.
- [ ] **Step 1: write the failing tests.** The migration test:

```rust
//! Migration 0196: `working_dir_swept_at` only on a WorkingDirectory block's run.
#[sqlx::test(migrations = "../../migrations")]
async fn swept_at_is_only_for_a_block_run(pool: sqlx::PgPool) {
    // Arrange: a run that is not a block (copy migration_0195's seed for one row).
    let run = seed_one_run(&pool).await;
    // Act
    let err = sqlx::query("UPDATE workflow.task_runs SET working_dir_swept_at = now() WHERE id = $1")
        .bind(run).execute(&pool).await.expect_err("not a block");
    // Assert
    assert!(err.to_string().contains("workflow_task_runs_working_dir_swept_on_block"), "{err}");
}
```

  In `tests/working_dir_claims.rs`:

```rust
/// **Debt D6: a crashed block's `_wd/{block}/` is swept 24 h after its run
/// ended, once.** The prepare claim writes the snapshot; the "crash" is the
/// execution ended by hand with no finish claim, so nothing discarded it.
#[sqlx::test(migrations = "../../migrations")]
async fn the_wd_sweep_removes_a_crashed_blocks_snapshots_once(pool: PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let db = egret_metastore::Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = egret_workflow::worker::start_execution(&pool, ws, None, KILLED_BLOCK, serde_json::json!({}))
        .await.expect("start");
    tick_with(&pool, 8, &registry(), WorkerTiming::default()).await.expect("the prepare claim");
    let block: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'block'",
    ).bind(ex).fetch_one(&pool).await.unwrap();
    let key = fs.working_dir_prefix(ws, ex, block)
        .strip_prefix("s3://").and_then(|r| r.split_once('/')).map(|(_, k)| k.to_string()).unwrap();
    let left = || async { fs.store.list(&key, None).await.expect("list").objects.len() };
    assert!(left().await > 0, "the fixture: a snapshot exists");
    // The crash: every row terminal, no finish claim ran.
    sqlx::query(
        "UPDATE workflow.task_runs SET state = 'FAILED', history = history || '[\"FAILED\"]'::jsonb, \
                claimed_at = NULL, ended_at = now() - interval '23 hours' WHERE execution_id = $1",
    ).bind(ex).execute(&pool).await.unwrap();
    sqlx::query(
        "UPDATE workflow.executions SET state = 'FAILED', history = history || '[\"FAILED\"]'::jsonb, \
                ended_at = now() WHERE id = $1",
    ).bind(ex).execute(&pool).await.unwrap();
    let cfg = egret_workflow::storage::wd_sweep::WdSweepConfig::default();
    // Act + Assert: young, kept.
    assert_eq!(egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg).await.unwrap(), 0);
    assert!(left().await > 0, "a block ended 23 h ago is not swept");
    // Old: swept, and stamped.
    sqlx::query("UPDATE workflow.task_runs SET ended_at = now() - interval '25 hours' WHERE id = $1")
        .bind(block).execute(&pool).await.unwrap();
    assert_eq!(egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg).await.unwrap(), 1);
    assert_eq!(left().await, 0, "every object under _wd/{{block}}/ is gone");
    let swept: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT working_dir_swept_at FROM workflow.task_runs WHERE id = $1",
    ).bind(block).fetch_one(&pool).await.unwrap();
    assert!(swept.is_some());
    // Once: a second pass finds nothing.
    assert_eq!(egret_workflow::storage::wd_sweep::sweep(&pool, &fs, &cfg).await.unwrap(), 0);
}
```

  Also add `a_normal_finish_stamps_swept_at`: a block that finishes normally has `working_dir_swept_at` set, and the sweep counts 0. Also add `a_failed_delete_leaves_swept_at_null_for_the_sweep`: use the failing-store pattern of `tests/nsfiles_store.rs:2786-2793` (`fail_deletes`), with the `MemStore` wrapper that test uses.

  The fixture's state names (`FAILED` on every run) must satisfy migration 0183's checks: `ended_at` only on terminal rows, and `ended_at >= started_at`. If one refuses, set `started_at` too.
- [ ] **Step 2: run them, expect FAIL**: `cargo test -p egret-workflow --test working_dir_claims -- wd_sweep normal_finish failed_delete` and `--test migration_0196_wd_swept`.
- [ ] **Step 3: write the migration**:

```sql
-- Debt package D6 (spec §3.1): when a WorkingDirectory block's `_wd/{block}/`
-- snapshots were removed. `worker::working_dir::discard` stamps it after a
-- delete that succeeded; `storage::wd_sweep` removes what a crash or a failed
-- delete left, 24 h after the block's run ended, and stamps it, so each
-- crashed block is listed once rather than on every pass.
ALTER TABLE workflow.task_runs ADD COLUMN working_dir_swept_at timestamptz;
ALTER TABLE workflow.task_runs
  ADD CONSTRAINT workflow_task_runs_working_dir_swept_on_block CHECK (
    working_dir_swept_at IS NULL OR working_dir IS NOT NULL
  );
CREATE INDEX workflow_task_runs_wd_unswept
  ON workflow.task_runs (ended_at)
  WHERE working_dir IS NOT NULL AND working_dir_swept_at IS NULL;
```

- [ ] **Step 4: implement `wd_sweep`** after `staging.rs`. One pass:
  - takes `pg_try_advisory_lock(<new key>)`, an 8-byte ASCII constant like `PRUNE_LOCK_KEY` (`storage/prune.rs:113`) with a test that it differs from the others;
  - selects up to `batch` rows: `t.id, e.id, e.workspace_id` from `task_runs t JOIN executions e`, `WHERE t.working_dir IS NOT NULL AND t.working_dir_swept_at IS NULL AND workflow.state_is_terminal(e.state) AND COALESCE(t.ended_at, t.updated_at) < now() - $older`;
  - runs `delete_prefix`, then the stamp, per row;
  - on a store failure, logs, leaves the row and ends the pass.

  Wire `discard` and `main.rs`. Run Step 1's tests; expect PASS.
- [ ] **Step 5: mutation.** Remove the stamp in `wd_sweep`, and see the "once" assertion fail (the second pass counts 1). Restore it.
- [ ] **Step 6:** `sqlx` offline data, if the crate uses it (`grep -rn "query!" src/storage/wd_sweep.rs`; this plan uses `query`/`query_scalar` only). Run the ratchet, then commit `feat(workflow): sweep the _wd snapshots a crashed WorkingDirectory left (migration 0196)`.

---

### Task 1.6: the flake-335 guard (D7)

**Files:**
- Modify: `tests/executor.rs:1469-1472`.

- [ ] **Step 1: reproduce, uncommitted.** Set `const LIVE: usize = 292;` in `oracle-exec/driver.rs:951`, then run:

```bash
cargo test -p egret-workflow --test executor 2>&1 | grep -E "must be empty|live versions|test result" | sort | uniq -c
```

  Expect one census failure and the other `egret_side()` tests failing "workflow.flows must be empty … left 341". Record the counts; this confirms spec §3.2's cause. Keep the `LIVE` edit for Step 3.
- [ ] **Step 2: the fix.** Inside the `is_process_clone` guard, replace the truncate:

```rust
// Debt D7 (2026-10-19): `flows` too. A `get_or_init` whose init panics
// stores nothing, so every later caller runs this again on the SAME clone;
// without emptying `flows` each one failed "workflow.flows must be empty,
// left 335" instead of the census assert that really failed (the flake of
// a7270435: 335 = one pass over the 345-flow corpus). `CASCADE` empties the
// tables that reference `flows`, executions included.
sqlx::query("TRUNCATE workflow.flows, workflow.executions CASCADE")
    .execute(&db.pool)
    .await
    .expect("empty the flow and execution tables");
```

- [ ] **Step 3: re-run Step 1's command** with `LIVE = 292` still set. Expect every failure to name the census assert and none "must be empty". Revert the `LIVE` edit (`git diff oracle-exec/driver.rs` must be empty), then run the ratchet green.
- [ ] **Step 4: commit** `test(workflow): a failed oracle setup reports its own cause, not "flows must be empty"`.

---

### Task 1.7: PR-1 docs (EN, then VI section by section)

**Files:**
- `docs/{en,vi}/specs/2026-10-17-workflow-kill-and-exit-design.md`. §4.2: a dated "(2026-10-19, debt package)" note after the `:101` bullet, saying that the user's ruling closes the open question, and stating D1, D2, D3 and D4 with their test names.
- `docs/{en,vi}/specs/2026-10-16-workflow-working-directory-design.md`:
  - `:101` and `:111`, and open point 2 at `:174`: the pruner does not reach `_wd`; the `_wd` sweep (migration 0196) does;
  - `{seq}.tar` becomes `prepare.tar` and `{child run}.tar`.
- `docs/{en,vi}/plans/2026-10-16-workflow-working-directory.md`: tick the 32 boxes whose work spec §3.3 found in code. Give the remaining 3 (Task 0's Kubernetes question, Task 1 Step 5) a dated "deferred: Kubernetes" note.
- `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9: a dated note for PR-1, giving what shipped, migration 0196, the new config key, and the ratchet counts from Task 1.6's run.
- `docs/en/specs/2026-10-19-workflow-debt-design.md` (and its VI mirror): `Status:` "PR-1 built and verified <date>".

- [ ] **Step 1:** write the edits. Every count names its source (the ratchet printout).
- [ ] **Step 2:** `cargo test -p egret-workflow --test executor` (the README counts are asserted; this PR changes none).
- [ ] **Step 3: commit** `docs(workflow): debt PR-1 — kill keeps delete:false, force-close, _wd sweep`.

## PR-1 final gate (controller, after the whole-branch review)

On the branch rebased onto fresh `origin/main`, on the private containers, one command at a time:
1. `ls migrations | tail -3`: 0196 is still unique.
2. `cargo fmt --all -- --check`
3. `cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo test -p egret-workflow` (all targets). The Docker live suites `container_docker_live` and `working_dir_docker_live` need a daemon and the pinned images; never set `EGRET_ALLOW_DOCKER_SKIP` at the gate.
5. `cargo test -p egret-config`, `cargo test -p egret-server`. `egret-query` and `egret-dataapi` are untouched; run them only if the rebase brought changes there.
6. There is no frontend change.

Known and accepted from earlier gates: the 7 `sql_starrocks_live` tests fail without an engine.

---

# Part 2 — PR-2: C2, C1 and docs

### Task 2.1: size caps on variables and labels (D8, D9)

**Files:**
- Modify: `src/executor/update.rs`:
  - constants `MAX_VARIABLES_BYTES = crate::runners::env::MAX_OUTPUT_BYTES` and `MAX_EXECUTION_LABELS: usize = 100`;
  - in `compute`, the `Set` arm checks `serde_json::to_vec(&result)?.len()`;
  - `set_labels` checks each new pair with `labels::MAX_LABEL_KEY_CHARS`/`MAX_LABEL_VALUE_CHARS` (messages as `labels.rs:59-68`), after Kestra's system/empty checks, and the merged length.
- Test: `update.rs` tests.

- [ ] **Step 1: write the failing tests:**

```rust
#[test]
fn set_variables_over_the_cap_fails_with_egrets_message() {
    let big = "x".repeat(MAX_VARIABLES_BYTES);
    let spec = serde_json::json!({"variables": {"big": big}}).to_string();
    let err = compute(UpdateOp::Set, &spec, &cx(), &Value::Null, &[]).unwrap_err();
    assert!(
        err.starts_with("`variables`: the execution's variables would be ")
            && err.ends_with(" bytes, over egret's limit of 1048576"),
        "{err}"
    );
}

#[test]
fn a_label_key_or_value_over_the_backfill_caps_fails() {
    let long_key = "k".repeat(129);
    let spec = serde_json::json!({"labels": {long_key.clone(): "v"}}).to_string();
    assert_eq!(
        compute(UpdateOp::Labels, &spec, &cx(), &Value::Null, &[]).unwrap_err(),
        format!("label key `{long_key}` is longer than 128 characters")
    );
    let spec = serde_json::json!({"labels": {"k": "v".repeat(1025)}}).to_string();
    assert_eq!(
        compute(UpdateOp::Labels, &spec, &cx(), &Value::Null, &[]).unwrap_err(),
        "the value of label `k` is longer than 1024 characters"
    );
}

#[test]
fn more_than_a_hundred_execution_labels_fails() {
    let current: Vec<Label> = (0..100)
        .map(|i| Label { key: format!("k{i}"), value: "v".into() })
        .collect();
    let err = compute(UpdateOp::Labels, r#"{"labels":{"one":"more"}}"#, &cx(), &Value::Null, &current)
        .unwrap_err();
    assert_eq!(err, "`labels`: the execution would carry 101 labels, over egret's limit of 100");
    // A key that already exists does not count twice.
    compute(UpdateOp::Labels, r#"{"labels":{"k0":"new"}}"#, &cx(), &Value::Null, &current)
        .expect("100 labels, one of them overridden");
}

#[test]
fn kestras_label_checks_still_run_first() {
    let spec = serde_json::json!({"labels": {"system.x": "v".repeat(2000)}}).to_string();
    assert!(compute(UpdateOp::Labels, &spec, &cx(), &Value::Null, &[])
        .unwrap_err()
        .starts_with("System labels can only be set by Kestra itself"));
}
```

- [ ] **Step 2: run them, expect FAIL**: `cargo test -p egret-workflow --lib -- executor::update`.
- [ ] **Step 3: implement.** Run `executor::update` (PASS), the ratchet (unchanged: 73/73), and `--test worker -- update labels variables`.
- [ ] **Step 4: mutation.** Remove the variables check, and see `set_variables_over_the_cap…` go red. Restore it. Then commit `feat(workflow): cap executions.variables at 1 MiB and a Labels task's labels`.

---

### Task 2.2: M5 — render against the pass's start (D10)

**Files:**
- Modify: `src/worker/render.rs:1459-1545`. Doc: "against the surface as this pass began, merged onto the current value, as Kestra's `handleWorkerTasks` + `handleExecutionUpdatingTasks`". Build `contexts: Vec<(RunIx, RenderContext)>` for every pending run (each `reserve_id` first) before the loop, then compute in run order with the live `surface.vars()` and `surface.current_labels()`.
- Modify: `tests/worker.rs:11538-11573`.

- [ ] **Step 1: change the test first.** Rename it `two_updates_in_one_parallel_render_the_start_and_merge_in_run_order`, add `variables:\n  x: 0\n` to the flow, and expect:

```rust
assert_eq!(
    execution_variables(&pool, ex).await,
    Some(serde_json::json!({"x": 2, "y": "0"})),
    "`two` renders the execution as the pass began (Kestra's frozen RunContext) \
     and merges onto `one`'s result (debt D10)"
);
```

  Keep the output assertion (`x` = "2").
- [ ] **Step 2: run it, expect FAIL** (`y` is `"1"`): `cargo test -p egret-workflow --test worker -- two_updates_in_one_parallel`.
- [ ] **Step 3: measure** whether `{{ vars.x }}` with no flow `variables:` fails or renders empty in egret's renderer (a `--lib` test of `render_str` with no `vars` key). Record it. Kestra's strict Pebble fails; if egret renders empty, note it in the spec's divergence list in Task 2.6. Do not change the renderer.
- [ ] **Step 4: implement.** Run the test (PASS), the I1 test `a_computed_update_beside_a_failing_one_is_success_and_written` (`overwrite: false` still sees the live value), the `worker::render` lib tests, and the ratchet (73/73).
- [ ] **Step 5: commit** `fix(workflow): updates in one pass render the pass's start, as Kestra (M5)`.

---

### Task 2.3: M7 — a restarted loop body gets a new task-run id (D12)

**Files:**
- Modify: `src/worker/claim.rs`, `persist` or `settle`, where `before` and `after` are both in hand. Find it with `grep -n "fn persist" -A40 src/worker/claim.rs`. A run whose `before.history` is non-empty and whose `after` history is shorter (`RestartLoop` emptied it) gets `UPDATE workflow.task_runs SET id = $3 WHERE execution_id = $1 AND run_ix = $2`, with a fresh `Uuid::new_v4()`. Use the statement at `claim.rs:4490-4500` as the model.
- Modify: `src/worker/render.rs`: a `Surface::forget_id(run)` so a later `reserve_id` in the same settle is fresh.
- Test: `tests/worker.rs`.

- [ ] **Step 1: write the failing test:**

```rust
/// **Debt D12 (M7): each `WaitFor` iteration recreates its body runs with new
/// ids, as Kestra's `retryWaitFor` + `TaskRun.of` do.** `deep_merge` appends
/// only list items that are new, so `ids` holds one id per iteration only if
/// the id changes.
#[sqlx::test(migrations = "../../migrations")]
async fn each_loop_until_iteration_has_its_own_task_run_id(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    // Arrange
    let (_db, ws) = seed(&pool).await;
    let flow = String::from(
        "id: m7\nnamespace: t\ntasks:\n\
         \x20 - id: until\n    type: io.kestra.plugin.core.flow.WaitFor\n    condition: \"false\"\n    \
         checkFrequency:\n      maxIterations: 3\n      interval: PT0S\n    failOnMaxReached: false\n    tasks:\n\
         \x20     - id: collect\n        type: io.kestra.plugin.core.execution.SetVariables\n        \
         variables:\n          ids:\n            - \"{{ taskrun.id }}\"\n",
    );
    let ex = start_updates(&pool, ws, &flow).await;
    // Act
    drain(&pool, &core_handlers()).await;
    // Assert
    let vars = execution_variables(&pool, ex).await.expect("variables written");
    let ids = vars["ids"].as_array().expect("a list");
    assert_eq!(ids.len(), 3, "one distinct id per iteration: {ids:?}");
    let distinct: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(distinct.len(), 3);
}
```

  If `interval: PT0S` is refused at validate, or `drain` does not pass a `not_before`, use the smallest interval the corpus uses (`PT0.2S`, `valids/waitfor-max-iterations`) and the drain helper that sleeps. Record which one was used.
- [ ] **Step 2: run it, expect FAIL** (one id): `cargo test -p egret-workflow --test worker -- each_loop_until_iteration`.
- [ ] **Step 3: implement.** Run the test (PASS), `--test worker -- loop until waitfor kill`, the `executor` lib tests and the ratchet. The ratchet is expected unchanged, because no golden field holds a task-run id. If an entry moves, stop and report.
- [ ] **Step 4: Docker, measured.** Add the live case `a_loop_until_body_with_delete_false_runs_a_new_container_each_iteration` to `container_docker_live.rs`: a `WaitFor` of 2 iterations over a `delete: false` script that echoes `{{ taskrun.id }}`. Expect two kept containers with different names, and two log lines with different ids. Then run it with Step 3's change reverted, so the report says whether egret re-attached before (spec §2.1's consequence). Remove the containers at the end.
- [ ] **Step 5: commit** `fix(workflow): a LoopUntil iteration gives its body runs new ids, as Kestra (M7)`.

---

### Task 2.4: code minors and test gaps

**Files:**
- `src/executor/update.rs`:
  - move `let current = …` (`:94`) into the `Set` and `Unset` arms;
  - add the tests below;
  - `set_variables_without_variables_fails` asserts the whole message.
- `src/executor/plan.rs:1365-1367`:

  ```rust
  (ModelKind::Leaf { spec }, _) if let Some(op) = super::update::UpdateOp::of(&task.r#type) => {
  ```

  If the toolchain (`rust-toolchain.toml`) lacks `if let` guards, write a `match` on `UpdateOp::of` inside the `Leaf` arm instead. Either way there is no `expect`.
- `src/spec/unrunnable.rs:144`: rename the test to `the_executor_level_types_pass_the_serve_gate_on_a_deployment_with_no_handlers`.

- [ ] **Step 1: write the tests** (in `update.rs`):

```rust
#[test]
fn a_whole_expression_that_renders_null_fails_naming_variables() {
    let err = compute(UpdateOp::Set, r#"{"variables":"{{ null }}"}"#, &cx(), &Value::Null, &[])
        .unwrap_err();
    assert!(err.starts_with("`variables`: the expression rendered `"), "{err}");
    assert!(err.ends_with("`, which is not a JSON object"), "{err}");
}

#[test]
fn deep_merge_inserts_an_absent_key_whose_new_value_is_null() {
    let a = obj(json!({"keep": 1}));
    let b = obj(json!({"new": null}));
    assert_eq!(Value::Object(deep_merge(&a, &b)), json!({"keep": 1, "new": null}));
}

#[test]
fn unset_on_null_variables_is_a_no_op_for_a_missing_leaf() {
    let change = compute(UpdateOp::Unset, r#"{"variables":["a"]}"#, &cx(), &Value::Null, &[])
        .unwrap();
    assert_eq!(change, Change::Variables(Map::new()));
}

#[test]
fn set_variables_without_variables_fails() {
    assert_eq!(
        compute(UpdateOp::Set, r#"{}"#, &cx(), &Value::Null, &[]).unwrap_err(),
        "`variables`: must not be null"
    );
}

#[test]
fn labels_list_and_form_errors_name_their_place() {
    assert_eq!(
        compute(UpdateOp::Labels, r#"{"labels":[{"key":"a"}]}"#, &cx(), &Value::Null, &[]).unwrap_err(),
        "`labels[0]`: expected a map with `key` and `value`"
    );
    assert_eq!(
        compute(UpdateOp::Labels, r#"{"labels":7}"#, &cx(), &Value::Null, &[]).unwrap_err(),
        "`labels`: expected a string, a list or a map"
    );
}
```

  The first test's exact rendered text (`` or `null`) is whatever the renderer gives; the asserts pin the frame, not the text. Record the text.
- [ ] **Step 2: run them.** The gap tests pass at once; each pins an existing rule. For each, mutate the rule it pins once (for example, drop the `Value::Null` arm in `deep_merge`), see it red, and restore it. Record each in the report.
- [ ] **Step 3: make the three code minors.** Run `cargo test -p egret-workflow --lib -- executor spec::unrunnable` and the ratchet.
- [ ] **Step 4: commit** `refactor(workflow): C2 minors — one UpdateOp lookup, no needless clone, tests that pin each message`.

---

### Task 2.5: publish accepts a reference absent from the caller's workspace

**Files:**
- Modify: `src/repo.rs`: add `FlowRepo::find_in_workspace(ws, namespace, flow_id) -> Result<Option<Uuid>>` (`deleted_at` not filtered, matching `find_by_name`'s choice at `:924-927`).
- Modify: `src/publish.rs`:
  - `resolve_task`'s `Subflow` arm (`:433-450`) and `resolve_trigger_dependencies` (`:532-558`) take `repo: &FlowRepo` and use the scoped lookup;
  - absent means `flow_ref = None`, plus, for a `Subflow` only, the warning `The subflow '<flowId>' not found in namespace '<namespace>'.` (Kestra's text, `FlowService.java:642`), pushed into the snapshot's `warnings`;
  - the module doc's steps 5 and 7 are rewritten.
- Modify: `tests/publish.rs:62,95,279,308`: rewrite the four tests (below), and keep the `Leaf`/`database` tests unchanged.
- Modify: `oracle-exec/driver.rs` (census constants with a dated doc line), and `oracle-exec/README.md` (the "Known over-refusals at publish" section `:13449-13468` becomes "Fixed 2026-10-19").

- [ ] **Step 1: rewrite the security tests first** (they fail today):

```rust
/// **Debt §5.1: a `Subflow` naming a flow only ANOTHER workspace has publishes
/// exactly as one naming a flow nobody has** -- with Kestra's warning, no
/// frozen id -- so publish still answers nothing about other workspaces.
#[tokio::test]
async fn a_subflow_absent_from_the_callers_workspace_publishes_with_the_warning_whoever_has_it() {
    let Some(db) = db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    let (other_ws, other_user) = common::seed_workspace(&db).await;
    let repo = egret_workflow::FlowRepo::new(db.clone());
    // Another workspace HAS `company.analytics/nightly_rollup`.
    common::draft(&repo, other_ws, other_user,
        "id: nightly_rollup\nnamespace: company.analytics\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: x\n",
    ).await;
    let src = |flow: &str| format!(
        "id: {flow}\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.subflow\n    namespace: company.analytics\n    flowId: {}\n",
        if flow == "foreign" { "nightly_rollup" } else { "nope" }
    );
    let foreign = common::draft(&repo, ws, user, &src("foreign")).await;
    let absent = common::draft(&repo, ws, user, &src("absent")).await;
    let refs = egret_workflow::WorkflowRefs::for_tests(db.clone()); // the real port; see Step 2
    let a = egret_workflow::publish(&repo, &refs, ws, user, foreign.id, "").await.expect("publishes");
    let b = egret_workflow::publish(&repo, &refs, ws, user, absent.id, "").await.expect("publishes");
    // Same shape: no frozen id, and the warning naming the pair.
    for (v, id) in [(&a, "nightly_rollup"), (&b, "nope")] {
        let warnings = v.snapshot["warnings"].to_string();
        assert!(warnings.contains(&format!("The subflow '{id}' not found in namespace 'company.analytics'.")), "{warnings}");
        assert!(v.snapshot["plan"]["tasks"][0]["flow_ref"].is_null(), "{}", v.snapshot);
    }
}
```

  Read `tests/publish.rs`'s other tests for the real names: how a version's snapshot is read, and how the real `WorkflowRefs` is built. The snippet's `snapshot`, `for_tests` and the JSON paths are illustrative where that file differs.

  Write the `dependsOn` twin the same way, with no warning. Write a runtime test in `tests/worker.rs`: a published flow whose `Subflow` target is absent runs, and its `Subflow` run fails with `subflow::resolve`'s existing not-found message, unchanged.
- [ ] **Step 2: run them, expect FAIL** (`NotFound` today): `cargo test -p egret-workflow --test publish -- subflow depends`.
- [ ] **Step 3: implement.** Run `--test publish`, `--test routes -- publish`, and the ratchet. Then read the census: the two entries `valids/task-runif` and `valids/trigger-flow-listener-no-inputs` should leave `VALIDATE_REFUSED` (27 → 25) and join `LIVE` (293 → 295). Measure it; do not assume. Update `driver.rs`'s constants with a dated doc line, and re-run until green. `baseline.txt` is expected unchanged.
- [ ] **Step 4: mutation.** Make the scoped lookup unscoped again (call `find_by_name`), and see the foreign case red: it freezes another workspace's id, or refuses. Restore it.
- [ ] **Step 5: commit** `fix(workflow): publish resolves subflow and dependsOn pairs in the caller's workspace and warns on an absent one, as Kestra`.

---

### Task 2.6: PR-2 docs (EN, then VI section by section)

**Files:**
- `docs/{en,vi}/specs/2026-10-18-workflow-core-tasks-design.md`:
  - **C1 build note:** the `{% %}` rule (debt spec §5.2).
  - **§3.1:** "Each one sees the result of the one before it" becomes D10's rule, with a dated note.
  - **The C2 build list gains a "(2026-10-19, debt package)" block:**
    - caps (D8, D9);
    - M5 (D10, plus Task 2.2 Step 3's measurement);
    - M6 recorded (debt spec §4.3, with Kestra's lines);
    - M7 (D12);
    - M8 recorded (§4.5).
- `crates/egret-workflow/src/spec/schema/README.md` §1: split the sentence at `:29-31`; rename the recipe's container and user (`kestra-schema-export`, `admin@egret.local`); nothing else.
- `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9:
  - VI `:511` gains EN `:523`'s `~~…~~ *(…retired 2026-10-17…)*`;
  - EN and VI gain the dated C2 note (from the C2 spec's Status: ratchet 84 → 73, LIVE 281 → 293, `UNKNOWN_TYPE_REFUSED` 33 → 21);
  - EN and VI gain the PR-2 note;
  - EN `:541`'s "Group C2 … is next" gets a dated pointer to the new note. Do not delete it.
- `crates/egret-workflow/oracle-exec/README.md`: Task 2.5's section (the two over-refusals closed, user decision 2026-10-19).
- `docs/{en,vi}/specs/2026-10-19-workflow-debt-design.md`: `Status:` "PR-2 built and verified <date>".

- [ ] **Step 1:** write the edits. Each count names its source.
- [ ] **Step 2:** `cargo test -p egret-workflow --test executor` (the README counts are asserted) and `--test flow_schema`, since the README sits beside its inputs.
- [ ] **Step 3: commit** `docs(workflow): debt PR-2 — C2 divergences, C1 render_map note, schema README, programme §9`.

## PR-2 final gate (controller, after the whole-branch review)

On the branch rebased onto fresh `origin/main` (after PR-1, if PR-1 merged first), one command at a time:
1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test -p egret-workflow` (all targets, including `container_docker_live` for Task 2.3 Step 4)
4. `cargo test -p egret-server` (the OpenAPI and generated-client checks). Run `egret-query` and `egret-dataapi` only if the rebase touched them; `egret-dataapi` needs `EGRET_TEST_SHARED_STORE_URL`.
5. There is no frontend change.

Known and accepted: the 7 `sql_starrocks_live` tests fail without an engine.
