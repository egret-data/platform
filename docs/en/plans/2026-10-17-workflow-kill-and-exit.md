# Kill an execution, and the `Exit` task: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** `docs/en/specs/2026-10-17-workflow-kill-and-exit-design.md`. Read it first; this plan refers to its sections and its default decisions D1-D7.

**Goal:** `POST …/executions/{id}/kill` stops a running execution (`KILLING` → `KILLED`), the worker really stops the handler (container, plugin host, SQL), a dead worker's kill is finished by the next one, and `io.kestra.plugin.core.execution.Exit` runs, banking the four `valids/exit*` corpus entries.

**Architecture:** the kill is data, not a message: the route writes `KILLING` into the execution and its leased runs in one transaction; the pure `advance()` turns a `KILLING` execution into `KILLED` commands; the worker learns of a kill by polling its run's state and by the new `AND state = 'RUNNING'` fences, cancels the handler, and writes `KILLED` under its lease. `Exit` is executor-level, with no worker handler.

**Tech Stack:** Rust (axum, sqlx, utoipa), Postgres, Docker (bollard) for the live test, React + TypeScript (orval client) for the UI.

**Branch / worktree:** `feat/workflow-kill-exit` in `data-portal/egret-kill`, from `origin/main` `6e995c1f`. Spec commit `93155a2e`.

## Global Constraints

- States: `RunState::Killing` is **non-terminal**; wire name `KILLING`. `KILLED` is terminal and already exists.
- Kill route: `POST /api/ws/{workspace}/workflows/executions/{id}/kill?cascade=true|false`, `cascade` default `true`; `202` + execution DTO; `404` outside the workspace; `409` with code `execution_terminated` on a terminal execution; permission `WF_MANAGE`; audited.
- Fences: `renew_lease` and `apply_outcome_with` both gain `AND state = 'RUNNING'`.
- Kill poll: config key `workflow.kill_poll_ms`, default `5000`.
- Cascade reaches loop children (`parent_execution_id` set AND `parent_run_ix IS NOT NULL`) and subflow children (`trigger_execution_id = <parent id>` AND `trigger_run_ix IS NOT NULL`), recursively. A **replay** (`parent_execution_id` set AND `parent_run_ix IS NULL`) is NOT a child and is never killed by cascade, and neither is an execution a Flow trigger fired (`trigger_execution_id` set AND `trigger_run_ix IS NULL`).
  - *Correction, 2026-10-18:* this line first gave the subflow predicate as `execution_trigger->>'type' = 'io.kestra.plugin.core.flow.Subflow'` AND `execution_trigger->'variables'->>'executionId' = <parent id>`. That matches no row: `subflow::spawn_one` writes `trigger_execution_id` / `trigger_run_ix` and leaves `execution_trigger` NULL. Task 6 built the link-column predicate above (spec §3.2 note).
- `Exit.state` ∈ {`SUCCESS`, `WARNING`, `KILLED`, `FAILED`, `CANCELLED`}, default `SUCCESS`.
- `oracle-exec/golden.json` is never edited to match egret. The ratchet (`crates/egret-workflow/tests/executor.rs`) must report the four `valids/exit*` entries banked; record before/after counts.
- Never use the shared dev stack (`:5432`, `deploy/docker-compose.dev.yml`, shared buckets). DB tests run on a private Postgres container `egret-kill-postgres` on host port **5452** (`DATABASE_URL=postgres://egret:egret@localhost:5452/egret_test`) with `EGRET_ALLOW_DB_SKIP` unset.
- One cargo command at a time. `rg` is NOT installed: use `grep -rn`.
- Frontend follows `docs/en/frontend-rules.md`; regenerate the client with `npm run gen:api`, never bare orval.
- Commits: Conventional Commits, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

---

### Task 1: `RunState::Killing`, and no migration (spec §2)

**Files:**
- Modify: `crates/egret-workflow/src/executor/state.rs` (enum at `:447`, `ALL` / `slot()` / wire names `:652-710`, terminality predicates, the `State` mapping `:1114`)
- Modify: every other `match` on `executor::RunState` the compiler flags, and every predicate that lists a SUBSET of states (find with `grep -rn "RunState::Killed" crates/egret-workflow/src`)
- Test: in `executor/state.rs`'s tests and `crates/egret-workflow/tests/execution_timestamps.rs` (it pins `workflow.state_is_terminal` against every variant)

**Interfaces:**
- Produces: `RunState::Killing` (non-terminal, wire `"KILLING"`), mapping to `models::State::Killing`.

- [x] **Step 1: tests first.** Add to `executor/state.rs`'s tests:

```rust
#[test]
fn killing_is_a_live_state_that_round_trips() {
    assert!(RunState::ALL.contains(&RunState::Killing));
    assert!(!RunState::Killing.is_terminal());
    assert_eq!(RunState::Killing.as_str(), "KILLING");
    assert_eq!(RunState::from_str("KILLING").unwrap(), RunState::Killing);
}
```

Use the file's actual names for the wire conversion (`as_str` / `FromStr` / `wire()`: read `:680-720` first and match them).

- [x] **Step 2: run, expect compile failure** (`no variant Killing`): `cargo test -p egret-workflow --lib executor::state`.
- [x] **Step 3: add the variant** after `Killed`, with a doc comment: non-terminal; written by the kill route (on the execution and on leased runs) and by `advance()` (on flowable runs); left only by `KILLED`. Add it to `ALL` and `slot()`. Fix every exhaustive match the compiler reports. Then audit every **subset** predicate (the W8 lesson: a predicate defined by what it omits goes wrong silently): for each, write down in the task report whether `Killing` belongs in it and why. At minimum: "is live / not terminal" sets include it; "claimable" (`RUNNING`) does not; "paused" does not; failure aggregation does not.
- [x] **Step 4: confirm no migration is needed.** On the private Postgres, after `cargo run -p egret-bin -- migrate` with `DATABASE_URL` set to it:

```sql
SELECT conrelid::regclass, conname, pg_get_constraintdef(oid)
  FROM pg_constraint WHERE conrelid IN ('workflow.task_runs'::regclass, 'workflow.executions'::regclass);
SELECT indexname, indexdef FROM pg_indexes WHERE schemaname = 'workflow' AND tablename IN ('task_runs', 'executions');
```

Record every row that mentions `state` and say whether `KILLING` passes it. If one refuses it, STOP and report NEEDS_CONTEXT with the constraint (the migration number must be checked against main).
- [x] **Step 5: run** `cargo test -p egret-workflow --lib executor::state` and `cargo test -p egret-workflow --test execution_timestamps` (DB). Expect PASS.
- [x] **Step 6: commit** `feat(workflow): a KILLING run state, live until KILLED`.

**Done when:** the variant exists, every subset predicate is ruled on in the report, and no constraint refuses `KILLING`.

---

### Task 2: `advance()` closes a `KILLING` execution (spec §5)

**Files:**
- Modify: `crates/egret-workflow/src/executor/mod.rs` (`advance` at `:175`)
- Test: `crates/egret-workflow/src/executor/mod.rs` tests (or a new `executor/kill.rs` with the logic and its tests, called from `advance`; prefer the new file, the module is 490 KB)

**Interfaces:**
- Consumes: `RunState::Killing` (Task 1).
- Produces: `pub(crate) fn executor::kill::advance_killing(plan: &Plan, state: &ExecutionState) -> Vec<Command>`; `advance` calls it first when `state.history.last() == Some(&RunState::Killing)` and returns its result.

- [x] **Step 1: table test first** in `executor/kill.rs`. Build states with the module's existing test helpers (read how `executor/allow_failure.rs`'s tests build a `Plan` and `ExecutionState`). Cases and expected commands:
  1. execution `Killing`, one leaf run `Killing` → no command (wait for the worker).
  2. execution `Killing`, one leaf run `Killed` → `SetExecutionState { Killed }`.
  3. execution `Killing`, a `Sequential` parent `Running` with children `[Success, Killing]` → `SetRunState { parent, Killing }` only.
  4. same parent already `Killing`, children `[Success, Killed]` → `SetRunState { parent, Killed }`; the next round with everything terminal → `SetExecutionState { Killed }`.
  5. nested flowables two deep → the inner parent is closed before the outer one, one level per round.
  6. a task not yet created in a `Sequential` (children `[Success]` of three) → nothing is created; parent → `Killing` then `Killed`.
  7. a `Parallel` with one child `Killing`, one `Success` → no new child started.
  8. a run in `Retrying` under a `Killing` execution → no retry is reopened (`retry::reopen_due` is not called).
- [x] **Step 2: run, expect compile failure**: `cargo test -p egret-workflow --lib executor::kill`.
- [x] **Step 3: implement** `advance_killing`: for every non-terminal flowable run, emit `SetRunState(Killing)` if it is not `Killing`, else `SetRunState(Killed)` (with the attempt closed, use the same command shape the executor uses for terminal flowables) when all its child runs are terminal; when every run is terminal and no command was emitted, emit `SetExecutionState { state: Killed }`. Children are found the way flowables already find them (`parent_ix`). In `advance`, the `Killing` check goes after the terminal early-return (`:176`) and before `retry::reopen_due` (`:184`).
- [x] **Step 4: run** the table test, then `cargo test -p egret-workflow --lib executor`. Expect PASS.
- [x] **Step 5: commit** `feat(workflow): advance closes a KILLING execution`.

**Done when:** all eight cases pass and the existing executor tests are unchanged.

---

### Task 3: the `Exit` task, banking `valids/exit*` (spec §6)

**Files:**
- Modify: `crates/egret-workflow/src/spec/registry.rs` (register `io.kestra.plugin.core.execution.Exit`, executor-level, no handler; follow how `Subflow` (`TaskKind::Executable`) or another executor-level type is registered)
- Modify: `crates/egret-workflow/src/executor/` (a new `executor/exit.rs`; `plan.rs` for its `TaskKind`)
- Modify: `crates/egret-workflow/src/executor/mod.rs:4235-4250` (remove the four `Exit` refusal entries)
- Modify: `crates/egret-workflow/oracle-exec/baseline.txt` (remove the entries that bank; the ratchet test tells you which)
- Test: `executor/exit.rs` tests; `crates/egret-workflow/tests/executor.rs` (the ratchet)

**Interfaces:**
- Consumes: `RunState::Killing`, `advance_killing` (Tasks 1-2).

- [x] **Step 1: measure the golden.** For each of `valids/exit`, `valids/exit-cancelled`, `valids/exit-killed`, `valids/exit-nested`, print its golden trace from `oracle-exec/golden.json` (every run's state history and the execution's). Write the four traces into the task report. They, not the Java reading, are the acceptance.
- [x] **Step 2: table test first** in `executor/exit.rs`: for each of the five states, an `Exit` inside an `If`'s `else` (the corpus shape) and inside a `Parallel` with a running sibling. Expected (spec §6): non-`KILLED` → the `Exit` run, every ancestor run and the execution get that state, the sibling is untouched (D5); `KILLED` → `SetExecutionState { Killing }` and nothing else. Plus: a templated `state: "{{ inputs.s }}"` renders; `state: NOPE` fails the run with a message naming `NOPE`. Where the Step 1 golden disagrees with these expectations, the golden wins: change the test and note it.
- [x] **Step 3: run, expect failure.**
- [x] **Step 4: implement**: register the type, give it a `TaskKind` that no worker claims, and handle its run in `advance()` when it is created (the way `Executable` runs are handled at creation). Remove the four refusal entries.
- [x] **Step 5: ratchet.** Run `cargo test -p egret-workflow --test executor` on the private DB before your change (record the counts) and after. Expected: the four `valids/exit*` entries report `PROGRESS NOT BANKED`; remove them from `baseline.txt`; rerun; green, with the baseline smaller by four. Any other entry that changes is a finding: report it, do not silence it.
- [x] **Step 6: commit** `feat(workflow): the Exit task ends an execution with a chosen state`.

**Done when:** the four entries are banked and removed from the baseline, and no other ratchet entry moved.

---

### Task 4: fences and the worker's kill path (spec §4.1, §4.2)

**Files:**
- Modify: `crates/egret-workflow/src/worker/claim.rs` (`renew_lease` `:1845`, `apply_outcome_with` fence `:2031`, `under_lease_with` `:1769-1835`, the callers that match `Leased`)
- Modify: `crates/egret-config/src/lib.rs` (`WorkflowConfig` `:593`: `kill_poll_ms: u64`, `#[serde(default = "default_kill_poll_ms")]`, default 5000, validated `>= 100`), and where `WorkerTiming` is built from config
- Modify: `crates/egret-workflow/src/worker/cancel.rs` (module doc: the kill path now has a second site)
- Test: `crates/egret-workflow/src/worker/claim.rs` tests (DB-backed, the module's existing harness)

**Interfaces:**
- Consumes: `RunState::Killing` (Task 1); `advance()` handling `Killing` (Task 2).
- Produces: `Leased::Killed` (a new variant: the run was found `KILLING` under this worker's lease); `WorkerTiming::kill_poll: Duration`; `async fn finish_kill(pool, claimed: &Claimed, held: DateTime<Utc>, handler: &dyn TaskHandler, task: &TaskInput, grace: Duration) -> Result<Committed>` used again by Task 5.
  - *Correction, 2026-10-18:* as built, `finish_kill(pool, claimed, held, reg: &dyn Registry, task)` takes no grace. Each handler applies its own grace inside `cancel` (spec §4.2 note), and `Registry::release` runs after `KILLED` commits.

- [x] **Step 1: tests first** (DB):
  1. `a_killing_run_refuses_the_workers_outcome`: claim a run, set it `KILLING` by SQL, call `apply_outcome` with the held lease → `LeaseLost`, row unchanged.
  2. `a_killing_run_refuses_a_renewal`: same, `renew_lease` → `None`.
  3. `the_worker_cancels_and_writes_killed`: a fake handler that sleeps 60 s and records `cancel` calls; `kill_poll = 100 ms`; set the run `KILLING` 300 ms after the handler starts; assert within 2 s: `cancel` called once with the configured grace, run `KILLED` with `ended_at`, attempt closed, `claimed_at` cleared, and the execution advanced (with a `Killing` execution and no other runs → `KILLED`).
  4. `a_cancel_that_errs_still_ends_killed`: the fake's `cancel` returns `Err` → run still `KILLED`, error only in tracing.
  5. `a_reaped_run_is_still_lost_not_killed`: the fence moved by another claim (state still `RUNNING`) → `Leased::Lost`, `cancel` NOT called (the W5a re-attach rule, `claim.rs:1715-1721`).
- [x] **Step 2: run, expect failures.**
- [x] **Step 3: implement.** Add `AND state = 'RUNNING'` to both fences. In `under_lease_with`, add a `kill_poll` interval to the existing `select!` loop: `SELECT state, claimed_at FROM workflow.task_runs WHERE id = $1`; on `KILLING` with `claimed_at = held` → drop the work future, return `Leased::Killed { held }`. A renewal returning `None` also reads the row once: `KILLING` under `held` → `Leased::Killed`, otherwise `Leased::Lost` as today. Every caller of `under_lease_with` handles `Killed` by calling `finish_kill`, which calls `handler.cancel(task, grace)` (log `Err`), then in one transaction writes `state = 'KILLED', ended_at = <the timestamp helper the module uses>, claimed_at = NULL` (attempt closed the way a failed attempt is closed) `WHERE id = $1 AND claimed_at = $2 AND state = 'KILLING'`, and runs `advance()` and persists its commands exactly as `apply_outcome_with` does. 0 rows → `Committed::LeaseLost`.
- [x] **Step 4: run** the five tests, then `cargo test -p egret-workflow --lib worker::claim` and `--lib worker::cancel`.
- [x] **Step 5: commit** `feat(workflow): the worker stops a killed run's handler and records KILLED`.

**Done when:** the five tests pass and the existing worker tests are unchanged.

---

### Task 5: a dead worker's kill is finished (spec §4.3)

**Files:**
- Modify: `crates/egret-workflow/src/worker/claim.rs` (`claim` `:475-500`; the tick that hands claimed runs to handlers)
- Check: `crates/egret-workflow/src/worker/reaper.rs:42-44` (already clears any expired lease; add a test, change only if it filters on `RUNNING`)
- Test: `claim.rs` / `reaper.rs` tests (DB)

**Interfaces:**
- Consumes: `finish_kill` (Task 4).

- [x] **Step 1: tests first** (DB):
  1. `the_reaper_frees_an_expired_killing_lease`: a `KILLING` run with `claimed_at` older than `working_timeout` → reaped (`claimed_at` NULL), state still `KILLING`.
  2. `claim_takes_a_killing_run_only_to_finish_the_kill`: a `KILLING` run with `claimed_at` NULL; tick with a fake handler → `run` NOT called, `cancel` called once, run `KILLED`.
  3. `claim_order_is_unchanged_for_running_runs` (guard the existing ordering test still holds with a `KILLING` row interleaved).
- [x] **Step 2: run, expect failures** (1 may already pass: then it is a guard; say so).
- [x] **Step 3: implement**: `claim`'s predicate becomes `state IN ('RUNNING', 'KILLING') AND claimed_at IS NULL` (keep the `type_id` / `working_dir` clause and the order), `RETURNING` the state; the tick dispatches a `KILLING` claim to `finish_kill` with the leased `claimed_at` instead of running the handler. A `KILLING` run in a `working_dir` phase (`PREPARING`/`FINISHING`) has no handler work to stop: `finish_kill` with a no-op cancel. *(Superseded 2026-10-17 by spec D8: a killed WorkingDirectory block's finish claim runs discard-only, and the `KILLING` claim arm is leaf-only.)*
- [x] **Step 4: run** the tests and `cargo test -p egret-workflow --lib worker`.
- [x] **Step 5: commit** `feat(workflow): a worker finishes a kill its predecessor started`.

---

### Task 6: the kill route (spec §3)

**Files:**
- Modify: `crates/egret-workflow/src/routes.rs` (beside `resume_execution` `:2828`; the route list in the module doc `:34`; the router; replace the `# The kill finding` comment `:2176-2236` with a short pointer to the spec, keeping its history in the spec §1 table)
- Create: `crates/egret-workflow/src/worker/kill.rs` (the transaction; the route calls it)
- Modify: `openapi.json` (regenerated), `frontend/src/api/` (regenerated with `npm run gen:api`)
- Test: `crates/egret-workflow/tests/kill_route.rs` (new, DB; model on the existing route tests for resume)

**Interfaces:**
- Consumes: `RunState::Killing`, `advance()` (Tasks 1-2).
- Produces: `pub async fn kill_execution(pool: &PgPool, workspace: Uuid, execution: Uuid, cascade: bool) -> Result<KillOutcome>`, `enum KillOutcome { Killing, Killed }`; route `POST /api/ws/{workspace}/workflows/executions/{id}/kill`, query `KillQuery { cascade: Option<bool> }`.

- [x] **Step 1: tests first** (DB, through the router):
  1. a running execution with one leased leaf → `202`, execution `KILLING`, the run `KILLING` with `claimed_at` unchanged.
  2. an execution whose only run is waiting (not leased) → `202`, execution `KILLED`, run `KILLED` with `ended_at`.
  3. a paused execution (D2) → `KILLED`, the paused run `KILLED`.
  4. terminal → `409`, code `execution_terminated`; unknown id / other workspace → `404`; a caller without `WF_MANAGE` → `403`.
  5. cascade: a parent with a loop child and a subflow child (build them as `worker::subflow` and the loop spawner do), both running → both `KILLING`/`KILLED`; with `cascade=false` → children untouched.
  6. a replay of the execution (`parent_execution_id` set, `parent_run_ix` NULL) is NOT killed by cascade.
  7. an audit record is written (model on resume's audit).
- [x] **Step 2: run, expect failures.**
- [x] **Step 3: implement** `worker::kill::kill_execution` per spec §3.2: one transaction; lock the execution `FOR UPDATE`; collect descendants with a recursive CTE over both links (Global Constraints), lock them in id order; for each execution: leased non-terminal leaf runs → `KILLING`; other non-terminal leaf runs (and `Executable` runs) → `KILLED` with `ended_at`; execution → `KILLING`; then load the state and run `advance()` once, persisting its commands, so an execution with nothing running answers `KILLED`. The route maps the outcome to `202` + the execution DTO.
- [x] **Step 4: run** the tests; regenerate `openapi.json` the way the repo does (find the generator: `grep -rn "openapi.json" justfile`) and the client with `npm run gen:api` in `frontend/`; the committed-openapi test must pass (`grep -rn committed_openapi crates`).
- [x] **Step 5: commit** `feat(workflow): kill an execution, with its subflow and loop children`.

---

### Task 7: live kill on Docker (spec §8)

**Files:**
- Modify: `crates/egret-workflow/tests/container_docker_live.rs` (it has its own `main`, see `Cargo.toml:372-386`; add two cases in its style)

- [x] **Step 1: write the two cases.**
  1. `kill_stops_a_running_container`: publish and start a flow with one `io.kestra.plugin.scripts.shell.Commands` task running `sleep 600` on the Docker runner; wait until its container exists; call `kill_execution(…, cascade = true)`; run the worker; assert within `kill_poll + grace + 10 s` the run is `KILLED`, the execution `KILLED`, and no container named for that run and attempt exists (list by the name scheme W5a uses).
  2. `a_kill_survives_the_worker_dying`: as above, but stop the worker task (abort its tokio task) right after the kill is written and before its poll fires (kill_poll set high for the first worker); shorten `working_timeout` so the reaper frees the lease; start a second worker; assert the same end state and that the second worker called `cancel` (the container is gone).
- [x] **Step 2: run** `env -u EGRET_ALLOW_DOCKER_SKIP DATABASE_URL=… cargo test -p egret-workflow --test container_docker_live -- --nocapture`. Both must run (not skip) and pass; record durations.
- [x] **Step 3: mutation.** Remove `handler.cancel` from `finish_kill` → case 1 must fail on "container still exists". Restore; rerun green. Record both.
- [x] **Step 4: commit** `test(workflow): a killed container stops, also after its worker dies`.

---

### Task 8: frontend (spec §7, D6)

**Files:**
- Modify: `frontend/src/components/workflows/executions/actions.ts` (`canKill`, beside `canResume` `:43` / `canReplay` `:52`)
- Modify: `frontend/src/routes/workflows/ExecutionDetail.tsx` (the button and the dialog, beside Resume/Replay)
- Modify: `frontend/src/components/workflows/StateChip.tsx`, `frontend/src/components/workflows/executionState.ts` (`KILLING` is live, not terminal: it is NOT added to `TERMINAL_STATES` `:7`), `frontend/src/components/workflows/topology/topologyModel.ts` (state colour)
- Test: `frontend/src/components/workflows/__tests__/StateChip.test.tsx`, a new `executions/__tests__/actions.kill.test.ts`, an `ExecutionDetail` test for the dialog

- [x] **Step 1: tests first**: `canKill` is enabled for every non-terminal state (including `PAUSED`, excluding `KILLING`, where the button reads "Killing…" and is disabled); the dialog's checkbox "Also kill subflow executions" defaults on and sends `cascade`; a `409 execution_terminated` shows an inline message, not a toast-only error; `StateChip` renders `KILLING` with its own label and colour.
- [x] **Step 2: run** `npm test -- --run <files>`; expect failures.
- [x] **Step 3: implement**, following `docs/en/frontend-rules.md` (read it first; it is normative) and the existing Resume/Replay patterns on that page.
- [x] **Step 4: run** the tests, `npm run lint`, `npm run typecheck` (use the repo's script names from `frontend/package.json`).
- [x] **Step 5: live check in the browser** on a private stack (not the shared dev stack): kill a running `sleep 600` flow from the page; see `KILLING` then `KILLED`; take a screenshot for the report.
- [x] **Step 6: commit** `feat(workflows): kill an execution from its page`.

---

### Task 9: docs, gate, PR

**Files:**
- Modify: `docs/en/specs/2026-09-01-workflow-programme-design.md` and the VI mirror (§9: execution-level kill and `Exit` shipped, dated note, append-only)
- Modify: `docs/en/specs/2026-10-17-workflow-kill-and-exit-design.md` + VI (status line: built and verified, with the ratchet numbers)
- Modify: this plan (tick the boxes)

- [x] **Step 1: docs** as listed; `grep -rn "kill\b" crates/egret-workflow/src/worker/handlers/sql.rs crates/egret-workflow/src/worker/cancel.rs` and correct any text that still says the execution-level kill does not exist.
- [ ] **Step 2: rebase on fresh `origin/main`**; if a migration was added (Task 1 said none), re-check its number.
- [ ] **Step 3: full local gate**, one command at a time, private Postgres (+ a private Redis for `egret-dataapi`'s `serving_shared_store` via `EGRET_TEST_SHARED_STORE_URL`): `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p egret-workflow --no-fail-fast`; `cargo test -p egret-query`; `cargo test -p egret-dataapi`; the Docker live suite; frontend `npm test -- --run`, lint, typecheck. Sum `test result:` lines from logs with grep (never trust `tail`). A red test is re-run alone on the branch and on `origin/main` before it is called pre-existing.
- [ ] **Step 4: commit docs; push; PR** (body: what changed, the ratchet numbers, the live durations, the mutation result, the gate counts; ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`). Merge per the autonomy rules.
