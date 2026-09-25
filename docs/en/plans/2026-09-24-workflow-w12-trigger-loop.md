# W12 — The trigger loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `Schedule` trigger declared in a flow's YAML fires in production — exactly once per tick, across any number of replicas, with missed ticks recovered as Kestra's default `ALL`.

**Architecture:** A new table `workflow.triggers` (one row per `Schedule`), kept in step with flow YAML by `scheduler::sync` inside every `FlowRepo` write transaction. A third ticker in `runtime::spawn` runs `scheduler::fire_due`: one trigger per transaction under `FOR UPDATE SKIP LOCKED`, the W6 pure `trigger::tick` on Postgres's clock, and the execution created **in the same transaction** that advances the schedule. A unique index on `workflow.executions` makes a double fire unrepresentable.

**Tech Stack:** Rust, sqlx (runtime queries — **no compile-time macros in this workspace**), tokio, Postgres.

**Spec:** `docs/{en,vi}/specs/2026-09-24-workflow-w12-trigger-loop-design.md`. Where this plan and the spec disagree, stop and report; do not pick one.

**A warning about this document.** Code blocks are **shapes to verify, not text to paste**. On earlier branches four errors shipped inside a plan's illustrative code and plan prose was factually wrong six times in twenty-four. **Read the file before editing it; any disagreement between this plan and the code means the plan is wrong.**

---

## Global Constraints

- **The ratchet cannot judge this package.** A `Schedule`-fired execution has no link `harvest.py` walks. **Expected: 109 before and after.** Measure it at the branch base and at the end, and report it beside a number that could disagree (the golden-entry count), because "unchanged" alone is unfalsifiable.
- **Measure the gate at the branch base (`6fb7520e`) before Task 2**, with all three service variables exported: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` (**never** the dev DB in `.env`), `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`, `EGRET_TEST_STARROCKS_URL`. Record result lines / passed / failed / ignored. Reconcile every later gate by **test-name-set diff**, not arithmetic; extract `#[test]`, `#[tokio::test]` and `#[sqlx::test]`.
- **Never run two test suites at once** — they share `egret_test`.
- **Migration number: the next free one when Task 2 runs.** `0154` on `origin/main` today, but the unmerged SP4a branch holds `0154` and `0155`. Check `git ls-tree origin/main migrations/` **and** `ls ../egret-sp4a/migrations` before naming the file. **Never edit an already-applied migration.**
- **`src/trigger/` stays pure.** No `sqlx`, no `tokio`, no wall clock. `tests::no_wall_clock_in_the_trigger_path` must stay green; `FixedClock` joins `TestClock` there and reads no clock.
- **`trigger::tick`'s semantics do not change.** If a task seems to need them to, that is a **finding**: stop and report.
- **One clock: Postgres's.** Every "now" in `scheduler/` comes from `SELECT now()` in the current transaction.
- **The DB-gated suites must not self-skip.** `common::db()` returns `None` only when `EGRET_ALLOW_DB_SKIP` is set; never set it.
- **Mutation discipline.** Every mutation in a task's table is applied, shown to have landed (`git diff --stat` plus an occurrence count of the mutated token), run **with a run count and an unmutated control**, and reverted. A mutation that fails to **compile** prints no `test result:` line — grep for it. A surviving mutant is reported as unproved, never rounded to covered. A dying one is asked **which assertion** killed it.
- **A zero is reported beside a number that can disagree**; a sweep is reported as **N examined / M corrected**.
- **Use `/usr/bin/grep` with file arguments**, not `rtk grep` piped — `rtk` returns the same empty output and exit code for a present and an absent token.
- Commits: Conventional Commits, `git -c user.email=hoangquocvietbk2.13@gmail.com`, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. **Commit before you deliberately break anything.** Never commit `.superpowers/` or a scratchpad. **No PRs** — the controller opens them.
- **Never `git checkout` a commit mid-task.** W10 lost half a package to a detached HEAD. Before each commit, `git branch --show-current` must print `feat/workflow-w12-trigger-loop`.

## File Structure

```
migrations/NNNN_workflow_triggers.sql       NEW — workflow.triggers, executions.scheduled_date,
                                            its CHECK and the one-per-tick unique index
crates/egret-workflow/src/
  trigger/mod.rs                            + FixedClock (pure)
  scheduler/mod.rs                          NEW — module doc: the safety statement per SQL statement
  scheduler/sync.rs                         NEW — sync(conn, flow, gone)
  scheduler/fire.rs                         NEW — fire_due(pool) -> PassReport
  lib.rs                                    + pub mod scheduler
  repo.rs                                   create/update/soft_delete become transactional, call sync
  worker/claim.rs                           start_execution_in(conn, …) extracted; old fn wraps it
  worker/mod.rs                             re-export start_execution_in, NewExecution
  runtime.rs                                third ticker; RuntimeConfig gains scheduler fields
crates/egret-config/src/lib.rs              WorkflowConfig: scheduler_enabled, trigger_interval_secs
crates/egret-bin/src/main.rs                passes the two new fields
crates/egret-workflow/tests/
  scheduler_sync.rs                         NEW — Task 2
  scheduler_fire.rs                         NEW — Task 4
  runtime.rs                                + scheduler ticker, two-replica and end-to-end tests
crates/egret-workflow/oracle-exec/
  w12-schedule-trigger.json                 NEW — Task 1's measurement, verbatim
  README.md                                 + W12 coverage section
```

---

## Task 1: Measure what Kestra stores for a `Schedule`-fired execution

**Files:** create `crates/egret-workflow/oracle-exec/w12-schedule-trigger.json`; modify `docs/{en,vi}/specs/2026-09-24-workflow-w12-trigger-loop-design.md` §5 only if the measurement contradicts it.

**Interfaces:** Produces the exact JSON shape of `execution.trigger` for a schedule, which Task 4's `schedule_trigger_record` reproduces.

`ScheduledExecution`'s doc (`trigger/mod.rs`) records the text form of `trigger.date/next/previous` as **unmeasured**. It is measured here, before code depends on it.

- [ ] **Step 1: Start the pinned artifact.** `cd crates/egret-workflow/oracle-exec && docker compose up -d`, then wait for `GET http://localhost:8080/api/v1/configs` to answer 200 with the basic-auth credentials in `docker-compose.yml` (read `harvest.py`'s `_auth()` for the exact header).
- [ ] **Step 2: Create a flow with a per-minute schedule in a non-UTC zone**, so the zone's effect on the text is visible:

```yaml
id: w12_probe
namespace: w12
triggers:
  - id: every_minute
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "*/1 * * * *"
    timezone: Asia/Ho_Chi_Minh
tasks:
  - id: a
    type: io.kestra.plugin.core.log.Log
    message: "{{ trigger.date }}"
```

`POST /api/v1/flows` with `Content-Type: application/x-yaml`.
- [ ] **Step 3: Wait for two executions** (poll `GET /api/v1/executions/search?namespace=w12&flowId=w12_probe`, ≤ 3 minutes). Fetch each with `GET /api/v1/executions/{id}`.
- [ ] **Step 4: Save both `trigger` objects verbatim** into `w12-schedule-trigger.json`, with the image digest, the capture time and the rendered `message` of task `a`:

```json
{
  "artifact": "kestra/kestra@sha256:28f59bf1…",
  "captured_at": "<UTC instant>",
  "executions": [
    { "id": "…", "trigger": { "id": "every_minute", "type": "…", "variables": { … } },
      "rendered_log_message": "…" }
  ]
}
```

- [ ] **Step 5: Write down the answers** in the file's top-level `"findings"` array: the key set of `variables`; the exact string form of `date` (offset or `Z`? fractional seconds?); whether `previous`/`next` are present on the first fire; whether any other key appears. **Each answer quotes the value it came from.**
- [ ] **Step 6: `docker compose down -v`.** If the artifact could not be run, say so and amend spec §5 to "chosen: RFC 3339 in the trigger's zone, no fractional seconds" — both languages.
- [ ] **Step 7: Commit** — `docs(workflow): W12 Task 1 — what Kestra stores for a Schedule trigger`.

---

## Task 2: The table, `sync`, and transactional flow writes

**Files:** create `migrations/NNNN_workflow_triggers.sql`; create `crates/egret-workflow/src/scheduler/{mod.rs,sync.rs}`; modify `crates/egret-workflow/src/lib.rs`, `crates/egret-workflow/src/repo.rs`; create `crates/egret-workflow/tests/scheduler_sync.rs`.

**Interfaces:**
- Consumes: `spec::parse(&str) -> Result<Flow, ParseError>`; `models::{Flow, Trigger, TriggerKind, Schedule}`; `trigger::{ScheduleTrigger, TriggerState, FixedClock}`; `ScheduleTrigger::resolve(&Trigger) -> Result<ScheduleTrigger, ScheduleError>`; `ScheduleTrigger::is_disabled()`.
- Produces: `pub async fn scheduler::sync(conn: &mut PgConnection, flow: &FlowRow, gone: bool) -> egret_core::Result<()>`; the table below; `FixedClock` in `trigger/mod.rs`:

```rust
/// A clock that answers one instant. Pure: the caller read it elsewhere
/// (in `scheduler/`, from Postgres's `now()`).
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub DateTime<Utc>);
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> { self.0 }
}
```

- [ ] **Step 1: Write the migration.**

```sql
-- Workflow W12: the trigger loop's queue. See
-- docs/en/specs/2026-09-24-workflow-w12-trigger-loop-design.md §3.
CREATE TABLE workflow.triggers (
  flow_id              uuid        NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  trigger_id           text        NOT NULL,
  workspace_id         uuid        NOT NULL,
  spec_key             text        NOT NULL,
  disabled             boolean     NOT NULL,
  next_evaluation_date timestamptz,
  evaluated_at         timestamptz,
  last_error           text,
  updated_at           timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (flow_id, trigger_id)
);
CREATE INDEX workflow_triggers_due ON workflow.triggers (next_evaluation_date)
  WHERE next_evaluation_date IS NOT NULL AND NOT disabled;

ALTER TABLE workflow.executions ADD COLUMN scheduled_date timestamptz;
ALTER TABLE workflow.executions ADD CONSTRAINT workflow_executions_scheduled_date_iff_schedule
  CHECK ((scheduled_date IS NOT NULL)
         = (execution_trigger IS NOT NULL
            AND execution_trigger->>'type' = 'io.kestra.plugin.core.trigger.Schedule'));
CREATE UNIQUE INDEX workflow_executions_one_per_schedule_tick
  ON workflow.executions (flow_id, (execution_trigger->>'id'), scheduled_date)
  WHERE scheduled_date IS NOT NULL;
```

Check the CHECK against existing rows before relying on it: `SELECT count(*) FROM workflow.executions WHERE execution_trigger->>'type' = 'io.kestra.plugin.core.trigger.Schedule'` on `egret_test` must be 0, **beside** the count of webhook-triggered rows (non-zero after the webhook suite has run).

- [ ] **Step 2: Write the failing tests** in `tests/scheduler_sync.rs` (use `common::db()`, `common::seed_workspace`, and `FlowRepo` directly — **not** SQL — so the tests go through the writers):

```rust
const CRON_A: &str = "id: f\nnamespace: t\ntriggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"0 2 * * *\"\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";

async fn row(db: &Db, flow: Uuid, trig: &str) -> Option<(Option<DateTime<Utc>>, bool, Option<String>)> {
    sqlx::query_as("SELECT next_evaluation_date, disabled, last_error FROM workflow.triggers \
                    WHERE flow_id = $1 AND trigger_id = $2")
        .bind(flow).bind(trig).fetch_optional(&db.pool).await.unwrap()
}
```

Tests, one per row of spec §6's sync block plus the refusal paths:
  - `create_writes_one_row_per_schedule_with_the_next_tick` — `next_evaluation_date` is the next 02:00 UTC after the DB's `now()` (read `now()` in the test; assert `> now` and `<= now + 1 day`, and that its time is 02:00:00).
  - `editing_an_unrelated_task_keeps_the_schedule` — set `next_evaluation_date` to a sentinel past value by SQL, `update` with a changed `message`, assert the sentinel survived.
  - `changing_the_cron_reinitialises` — same sentinel, change `0 2` → `0 3`, assert the sentinel is gone and the new time is 03:00.
  - `removing_a_schedule_deletes_its_row` and `soft_delete_deletes_every_row`.
  - `a_disabled_flow_or_trigger_sets_the_flag_and_keeps_the_dates` — both YAML switches, each with the sentinel.
  - `a_refused_schedule_is_stored_off_queue_with_its_reason` — `cron: "0 0 * * * *"` (6 fields, `withSeconds` false) → `next_evaluation_date` NULL, `last_error` contains `invalid cron`.
  - `a_webhook_or_flow_trigger_gets_no_row` — control that the table holds schedules only.

Run: `cargo test -p egret-workflow --test scheduler_sync`. Expected: FAIL to compile (`scheduler` does not exist).

- [ ] **Step 3: Implement `scheduler::sync`.**

```rust
/// Bring `workflow.triggers` in line with `flow.source`. Called inside the
/// transaction that wrote `flow`. `gone = true` for a delete.
pub async fn sync(conn: &mut PgConnection, flow: &FlowRow, gone: bool) -> Result<()> {
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()").fetch_one(&mut *conn).await.map_err(db_err)?;
    let parsed = if gone { None } else { crate::spec::parse(&flow.source).ok() };
    let schedules: Vec<&Trigger> = parsed.iter()
        .flat_map(|f| f.triggers.iter())
        .filter(|t| matches!(t.kind, TriggerKind::Schedule(_)))
        .collect();
    let flow_disabled = parsed.as_ref().is_some_and(|f| f.disabled);

    let keep: Vec<String> = schedules.iter().map(|t| t.id.clone()).collect();
    sqlx::query("DELETE FROM workflow.triggers WHERE flow_id = $1 AND NOT (trigger_id = ANY($2))")
        .bind(flow.id).bind(&keep).execute(&mut *conn).await.map_err(db_err)?;

    for t in schedules {
        let TriggerKind::Schedule(s) = &t.kind else { unreachable!() };
        let key = spec_key(s);
        let disabled = flow_disabled || t.disabled;
        let (next, err) = match ScheduleTrigger::resolve(t) {
            Ok(st) => (TriggerState::initialise(&st, "", "", &FixedClock(now)).next_evaluation_date, None),
            Err(e) => (None, Some(e.to_string())),
        };
        // Dates are kept only when the spec_key is unchanged AND the row is on the
        // queue. A row off the queue (refused, or taken off by `fire_due` for a
        // broken source) is re-initialised by the next save -- spec §4 point 6.
        // `disabled` is written every time and never touches the dates.
        sqlx::query(
            "INSERT INTO workflow.triggers \
               (flow_id, trigger_id, workspace_id, spec_key, disabled, next_evaluation_date, last_error) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (flow_id, trigger_id) DO UPDATE SET \
               disabled = EXCLUDED.disabled, updated_at = now(), \
               spec_key = EXCLUDED.spec_key, \
               next_evaluation_date = CASE WHEN workflow.triggers.spec_key = EXCLUDED.spec_key \
                      AND workflow.triggers.next_evaluation_date IS NOT NULL \
                 THEN workflow.triggers.next_evaluation_date ELSE EXCLUDED.next_evaluation_date END, \
               evaluated_at = CASE WHEN workflow.triggers.spec_key = EXCLUDED.spec_key \
                      AND workflow.triggers.next_evaluation_date IS NOT NULL \
                 THEN workflow.triggers.evaluated_at ELSE NULL END, \
               last_error = CASE WHEN workflow.triggers.spec_key = EXCLUDED.spec_key \
                      AND workflow.triggers.next_evaluation_date IS NOT NULL \
                 THEN workflow.triggers.last_error ELSE EXCLUDED.last_error END")
            .bind(flow.id).bind(&t.id).bind(flow.workspace_id).bind(&key)
            .bind(disabled).bind(next).bind(err)
            .execute(&mut *conn).await.map_err(db_err)?;
    }
    Ok(())
}

/// `cron|withSeconds|timezone` — the three fields that decide WHEN. A string,
/// not a digest: nothing needs it short, and a readable key is debuggable.
fn spec_key(s: &Schedule) -> String {
    format!("{}|{}|{}", s.cron.as_deref().unwrap_or(""), s.with_seconds,
            s.timezone.as_deref().unwrap_or(""))
}
```

Verify against the code rather than trust: `TriggerState::initialise` ignores its `namespace`/`flow_id` for the date (read it). **Why the `IS NOT NULL` term exists:** `lateMaximumDelay` and `when` are refused by `resolve` but are not part of `spec_key`, so without it a refused row whose author then deletes `lateMaximumDelay` would keep `next_evaluation_date = NULL` for ever. Write `fixing_a_refused_schedule_puts_it_back_on_the_queue` first, run it against a version of the SQL **without** the term, and watch it fail. (The term is keyed on the date, not on `last_error`, on purpose: `fire_due` also writes `last_error` for a consumed tick — templated inputs — and re-initialising that row would drop its backlog.)

- [ ] **Step 4: Make the four writers transactional and call `sync`.** In `repo.rs`, `create`, `update` and `soft_delete` change from `.fetch_one(&self.db.pool)` to a `tx`:

```rust
let mut tx = self.db.pool.begin().await.map_err(db_err)?;
let row: std::result::Result<FlowRow, sqlx::Error> = sqlx::query_as(/* unchanged SQL */)
    /* unchanged binds */
    .fetch_one(&mut *tx).await;
let row = match row { /* unchanged conflict mapping */ };
crate::scheduler::sync(&mut tx, &row, false).await?;
tx.commit().await.map_err(db_err)?;
Ok(row)
```

`soft_delete` must `RETURNING` the row (or re-select it inside `tx`) and call `sync(&mut tx, &row, true)`. **`set_live` is exempt** — add a one-line comment naming why. The conflict-mapping helpers (`namespace_conflict`, `effective_identity`) read through `&self.db.pool` **after** a failed statement; a failed statement aborts the transaction, so drop `tx` first (roll back) and keep those reads on the pool.

- [ ] **Step 5: The writer scanner.** In `tests/scheduler_sync.rs`, `every_flows_writer_calls_sync`: walk `crates/*/src/**/*.rs`, cut each file at its first `#[cfg(test)]`, find string literals containing `INSERT INTO workflow.flows`, `UPDATE workflow.flows` or `DELETE FROM workflow.flows` (tolerate the `\` line continuations used in this repo), and for each find the enclosing `fn` by scanning backwards for `fn `. Assert each enclosing function's body contains `scheduler::sync(` **or** is in:

```rust
const EXEMPT: &[(&str, &str)] = &[
    ("set_live", "changes neither source nor deleted_at; state is read at fire time"),
];
```

**Positive control:** assert the scanner found exactly `{create, update, soft_delete, set_live}` — four — so it cannot pass by finding nothing. Watch it fail before Step 4 lands (it will: none call `sync`).

- [ ] **Step 6: Run** `cargo test -p egret-workflow --test scheduler_sync --test repo --test routes` — PASS. Then `cargo test -p egret-workflow --lib trigger::` — `no_wall_clock_in_the_trigger_path` still PASS.
- [ ] **Step 7: Mutations** (each: evidence it landed, 2 runs, control):

| mutation | must kill |
|---|---|
| `CASE WHEN … spec_key = …` → always `EXCLUDED.next_evaluation_date` | `editing_an_unrelated_task_keeps_the_schedule` |
| delete the `sync` call in `update` | `every_flows_writer_calls_sync` **and** `changing_the_cron_reinitialises` |
| delete the `sync` call in `soft_delete` | `soft_delete_deletes_every_row` |
| `flow_disabled \|\| t.disabled` → `t.disabled` | the flow-level half of the disabled test |
| drop the `DELETE … NOT (trigger_id = ANY($2))` | `removing_a_schedule_deletes_its_row` |

- [ ] **Step 8: Full gate; commit** — `feat(workflow): W12 — workflow.triggers and sync on every flow write`.

---

## Task 3: `start_execution_in` — create an execution inside a caller's transaction

**Files:** modify `crates/egret-workflow/src/worker/claim.rs` (around `start_execution_with_trigger`, ~line 1381), `crates/egret-workflow/src/worker/mod.rs`; tests in `crates/egret-workflow/tests/worker.rs`.

**Interfaces:**
- Produces:

```rust
/// Everything a caller supplies to create an execution. A struct because the
/// alternative is a seventh positional argument, and a run of uniform `None`s
/// is the shape that hid W6 Task 6's surviving mutant.
pub struct NewExecution<'a> {
    pub workspace_id: Uuid,
    pub flow_id: Option<Uuid>,
    pub source: &'a str,
    pub inputs: serde_json::Value,
    pub trigger: Option<serde_json::Value>,
    pub scheduled_date: Option<DateTime<Utc>>,
}
pub async fn start_execution_in(conn: &mut PgConnection, new: NewExecution<'_>) -> Result<Uuid>;
```

`start_execution_with_trigger(pool, …)` keeps its signature and becomes: begin, `start_execution_in(&mut tx, NewExecution { …, scheduled_date: None })`, commit.

- [ ] **Step 1: Write the failing test** `start_execution_in_commits_nothing_when_the_caller_rolls_back`: begin a tx on the test pool, call `start_execution_in`, **drop the tx without committing**, assert `common::execution_count(ws) == 0`; then the same with commit → 1. And `scheduled_date_is_refused_without_a_schedule_trigger`: `scheduled_date: Some(..)` with `trigger: None` → the migration's CHECK refuses it (assert the error, not a panic).
- [ ] **Step 2: Run** `cargo test -p egret-workflow --test worker start_execution_in` — FAIL (no such fn).
- [ ] **Step 3: Extract.** Move the body (parse, `build_plan`, INSERT, `ExecutionCtx`, `settle`) into `start_execution_in`, taking `conn`; add `scheduled_date` to the INSERT's column list and binds. **Do not change `settle`'s call.** `start_execution_with_trigger` becomes the wrapper.
- [ ] **Step 4: Run** `--test worker --test webhook_route --test executions_routes --test runtime` — PASS, and the name-set diff against the base shows only the two new tests.
- [ ] **Step 5: Mutation** — have the wrapper call `start_execution_in` on `pool.acquire()` instead of a `tx` (autocommit): nothing in the old suites may notice (that is a finding about them, report it), and the new rollback test must die.
- [ ] **Step 6: Commit** — `refactor(workflow): start an execution inside a caller's transaction`.

---

## Task 4: `fire_due` — one trigger per transaction

**Files:** create `crates/egret-workflow/src/scheduler/fire.rs`; modify `scheduler/mod.rs`; create `crates/egret-workflow/tests/scheduler_fire.rs`.

**Interfaces:**
- Consumes: `scheduler::sync`, `worker::{start_execution_in, NewExecution}`, `trigger::{tick, TriggerState, TickOutcome, ScheduleTrigger, FixedClock, ScheduledExecution}`, Task 1's measured shape.
- Produces:

```rust
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PassReport { pub fired: usize, pub skipped: usize, pub errored: usize, pub frozen: usize }
/// One pass: every due trigger evaluated at most once. Returns when none is left.
pub async fn fire_due(pool: &PgPool) -> Result<PassReport>;
```

- [ ] **Step 1: Test helpers** in `tests/common/mod.rs` (Task 5's `tests/runtime.rs` uses them too):

```rust
/// Put a trigger `ticks` minutes behind: next_evaluation_date = now() - ticks min,
/// rounded to the minute so a `*/1` cron agrees with it.
async fn make_due(db: &Db, flow: Uuid, trig: &str, ticks: i32) {
    sqlx::query("UPDATE workflow.triggers SET next_evaluation_date = \
                 date_trunc('minute', now()) - make_interval(mins => $3) \
                 WHERE flow_id = $1 AND trigger_id = $2")
        .bind(flow).bind(trig).bind(ticks).execute(&db.pool).await.unwrap();
}
async fn scheduled_dates(db: &Db, flow: Uuid) -> Vec<DateTime<Utc>> {
    sqlx::query_scalar("SELECT scheduled_date FROM workflow.executions \
                        WHERE flow_id = $1 ORDER BY scheduled_date")
        .bind(flow).fetch_all(&db.pool).await.unwrap()
}
```

Flows are created with `common::seed_flow` (it goes through `FlowRepo::create`, so `sync` has already written the row). Use a `*/1 * * * *` cron.

- [ ] **Step 2: Write the failing tests** — one per row of spec §6 that `fire_due` owns:
  - `a_due_trigger_fires_once_and_moves_on` — `make_due(1)`, two `fire_due` calls → exactly one execution; its `execution_trigger` equals Task 1's shape; its `scheduled_date` equals the old `next_evaluation_date`; the row's new `next_evaluation_date` is one minute later.
  - `all_recovers_every_missed_tick_one_per_pass` — `make_due(5)`: pass 1 → 1 execution, … pass 5 → 5, ascending, `PassReport.fired == 1` each time.
  - `a_trigger_that_is_behind_does_not_starve_the_rest` — A `make_due(5)`, B `make_due(1)`; one pass → one execution each.
  - `a_disabled_row_is_never_claimed_and_does_not_block` — the oldest due row has `disabled = true` (set via YAML + update), a younger enabled one fires; the disabled row's dates are byte-identical after.
  - `every_off_switch_leaves_the_dates_frozen` — `state = 'disabled'` (SQL), `deleted_at` (via `soft_delete` — then assert the row is **gone**, which is the stronger form), YAML `disabled: true` on flow, on trigger. Zero executions; dates unchanged where the row survives.
  - `yaml_disagreeing_with_the_column_does_not_fire_and_resyncs` — set `disabled = false` by hand on a flow whose YAML says `disabled: true`; one pass → no execution, column back to `true`, dates unchanged.
  - `a_broken_trigger_is_taken_off_the_queue_and_its_sibling_still_fires` — corrupt one flow's `source` by SQL (bypassing `sync`), both due → one fires, the broken one has `next_evaluation_date` NULL and `last_error` set.
  - `templated_inputs_are_refused` — `inputs: {x: "{{ now() }}"}` on the schedule, a matching flow input → no execution, `last_error` names `inputs.x`, the tick is consumed (next date advanced).
  - `fire_and_advance_are_atomic` — install a test-only DB trigger that raises on `UPDATE workflow.triggers` **for this flow id only**, run a pass, assert zero executions and unchanged dates; drop the DB trigger in the test's cleanup **and** at its start.

```sql
-- Named per test flow so parallel test binaries cannot collide; created in
-- `public`, not `pg_temp` (a session-local function cannot back a trigger
-- that the pool's other connections fire). DROP both in cleanup AND at start.
CREATE FUNCTION public.w12_fail_<flow_uuid_hex>() RETURNS trigger LANGUAGE plpgsql AS
$$ BEGIN
     IF NEW.flow_id = '<flow_uuid>' THEN RAISE EXCEPTION 'w12 injected'; END IF;
     RETURN NEW;
   END $$;
CREATE TRIGGER w12_fail_<flow_uuid_hex> BEFORE UPDATE ON workflow.triggers
  FOR EACH ROW EXECUTE FUNCTION public.w12_fail_<flow_uuid_hex>();
```

`fire_due` returns `Err` for this pass (the transaction aborted); the test asserts the `Err`, zero executions for the flow, and unchanged dates.

  - `the_unique_index_refuses_a_second_fire` — after one fire, hand-insert a second execution row with the same `(flow_id, execution_trigger, scheduled_date)` → unique violation.

Run: `cargo test -p egret-workflow --test scheduler_fire` — FAIL to compile.

- [ ] **Step 3: Implement `fire_due`.**

```rust
pub async fn fire_due(pool: &PgPool) -> Result<PassReport> {
    let mut report = PassReport::default();
    let (mut seen_flows, mut seen_ids): (Vec<Uuid>, Vec<String>) = (vec![], vec![]);
    loop {
        let mut tx = pool.begin().await.map_err(db_err)?;
        let claimed: Option<Claimed> = sqlx::query_as(
            "SELECT t.flow_id, t.trigger_id, t.next_evaluation_date, t.evaluated_at, \
                    f.workspace_id, f.source, f.namespace, f.flow_id AS flow_key, now() AS db_now \
               FROM workflow.triggers t JOIN workflow.flows f ON f.id = t.flow_id \
              WHERE t.next_evaluation_date <= now() AND NOT t.disabled \
                AND f.deleted_at IS NULL AND f.state <> 'disabled' \
                AND (t.flow_id, t.trigger_id) NOT IN \
                    (SELECT * FROM unnest($1::uuid[], $2::text[])) \
              ORDER BY t.next_evaluation_date LIMIT 1 \
              FOR UPDATE OF t SKIP LOCKED")
            .bind(&seen_flows).bind(&seen_ids)
            .fetch_optional(&mut *tx).await.map_err(db_err)?;
        let Some(c) = claimed else { return Ok(report) };
        seen_flows.push(c.flow_id); seen_ids.push(c.trigger_id.clone());
        match fire_one(&mut tx, &c).await? {
            Fired => report.fired += 1, Skipped => report.skipped += 1,
            Errored => report.errored += 1, Frozen => report.frozen += 1,
        }
        tx.commit().await.map_err(db_err)?;
    }
}
```

`fire_one` does, in order: `spec::parse(&c.source)` → on error `take_off_queue(reason)`; if `flow.disabled` or the trigger's `disabled` → `scheduler::sync(tx, &flow_row, false)` and return `Frozen` **without touching dates**; find the trigger by id, `ScheduleTrigger::resolve` → on error `take_off_queue`; build `TriggerState { namespace: c.namespace, flow_id: c.flow_key, trigger_id, next_evaluation_date, evaluated_at, backfill: None }`; `refuse_templated(&schedule.inputs)`; `trigger::tick(&st, &state, &inputs, &FixedClock(c.db_now))`; on `Fired(e)` → `start_execution_in(tx, NewExecution { workspace_id, flow_id: Some(c.flow_id), source: &c.source, inputs: json(e.inputs), trigger: Some(schedule_trigger_record(&e)), scheduled_date: Some(e.scheduled_date) })`; finally `UPDATE workflow.triggers SET next_evaluation_date, evaluated_at, last_error, updated_at = now()`.

**The templated-input refusal consumes the tick** (spec §5): advance the dates exactly as `tick` computed them, record the reason, create no execution. Read `tick`'s returned `state` for the dates — do not recompute.

`schedule_trigger_record(&ScheduledExecution) -> serde_json::Value` produces **exactly** Task 1's measured shape; its unit test compares against `oracle-exec/w12-schedule-trigger.json` loaded with `include_str!`, not against a literal typed into the test.

- [ ] **Step 4: Run** `--test scheduler_fire` — PASS. `--lib trigger::` — PASS.
- [ ] **Step 5: Mutations:**

| mutation | must kill |
|---|---|
| drop the final `UPDATE workflow.triggers` | `a_due_trigger_fires_once_and_moves_on` |
| drop the `seen` clause | `a_trigger_that_is_behind_does_not_starve_the_rest` |
| wrap `fire_one` in `run_until_idle` (tick to idle) | `all_recovers_every_missed_tick_one_per_pass` |
| drop `NOT t.disabled` | `a_disabled_row_is_never_claimed_and_does_not_block` |
| each of `deleted_at`, `state <> 'disabled'` dropped in turn | `every_off_switch_leaves_the_dates_frozen` |
| drop the YAML re-check after parse | `yaml_disagreeing_with_the_column_does_not_fire_and_resyncs` |
| `take_off_queue` leaves `next_evaluation_date` as is | the broken-sibling test (it will now re-claim the broken row first) |
| commit after `start_execution_in`, open a new tx for the UPDATE | `fire_and_advance_are_atomic` |
| drop `refuse_templated` | `templated_inputs_are_refused` |

- [ ] **Step 6: Full gate; commit** — `feat(workflow): W12 — fire due schedule triggers, one per transaction`.

---

## Task 5: The ticker, config, and the concurrency proof

**Files:** modify `crates/egret-workflow/src/runtime.rs`, `crates/egret-config/src/lib.rs`, `crates/egret-bin/src/main.rs` (~line 1752), `crates/egret-workflow/src/scheduler/mod.rs` (module doc); extend `crates/egret-workflow/tests/runtime.rs`.

**Interfaces:**
- Consumes: `scheduler::fire_due(&PgPool) -> Result<PassReport>`.
- Produces: `RuntimeConfig` gains `scheduler_enabled: bool` and `trigger_interval: Duration`; `WorkflowConfig` gains `scheduler_enabled: bool` (default `false`) and `trigger_interval_secs: u64` (default `1`), both added to `default_workflow()`.

- [ ] **Step 1: Write the failing tests** in `tests/runtime.rs` (extend the `fast()` helper with the two fields, `scheduler_enabled: false` by default so existing tests are unaffected):
  - `the_scheduler_is_off_by_default` — `WorkflowConfig` deserialised from `{}` has `scheduler_enabled == false`; and `spawn` with it off, a due trigger, 200 ms → zero executions.
  - `the_scheduler_gate_is_independent_of_the_worker_gate` — scheduler on, worker off → the execution **is created** and stays `CREATED`.
  - `two_scheduler_tickers_never_fire_one_tick_twice` — two `spawn`s against one pool, 10 flows each `make_due(3)`, wait until 30 executions exist or 10 s, then assert **each `(flow_id, scheduled_date)` appears once** (group by, `having count(*) > 1` returns zero rows) **and** the total is exactly 30. Asserted on rows, not on `PassReport`s.
  - `a_cron_runs_to_success_with_nothing_driving_it` — the end-to-end: worker and scheduler both on, `Handlers::with_core_tasks()`, one flow `*/1` + `core.log.Log`, `make_due(1)`, poll ≤ 15 s until an execution with `scheduled_date` is `SUCCESS`. **No call to `tick`, `fire_due` or `claim` in the test body.** Control: the same with `scheduler_enabled: false` must end with zero executions.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement.** In `runtime::spawn`, a third block after the reaper's, same shape (`interval`, `MissedTickBehavior::Skip`, `select!` on `cancel`), calling `crate::scheduler::fire_due(&pool)`; log `fired`/`errored` at `info` when non-zero, `warn` on `Err`, never break the loop. **Move the existing `if !cfg.enabled { return }`** so it gates only the worker and reaper, and give the scheduler its own gate; if `scheduler_enabled && !enabled`, `tracing::warn!("workflow scheduler on with the worker off: executions will be created and nothing will run them")`. Wire `main.rs` with the two config fields.
- [ ] **Step 4: Write the safety section** in `scheduler/mod.rs`'s doc, in `runtime.rs`'s voice: **one statement per SQL statement** — the claim (`SKIP LOCKED`, exclusion), `sync`'s upsert (serialised by the row lock; a `sync` that meets a claimed row waits), the final `UPDATE` (under the claim's lock), the unique index (the backstop, and which test proves it is real). Add a pointer line to `runtime.rs`'s module doc.
- [ ] **Step 5: Run** `--test runtime` 3 times — PASS each (it is a clock-driven suite; flakiness is a defect).
- [ ] **Step 6: Mutations:**

| mutation | must kill |
|---|---|
| drop `SKIP LOCKED` (keep `FOR UPDATE`) | **expected to survive** — blocking still serialises; report it and name the index as what holds |
| drop `FOR UPDATE OF t SKIP LOCKED` entirely | `two_scheduler_tickers_never_fire_one_tick_twice` (via the unique index error or a duplicate) — report which |
| drop the unique index **and** the lock | `two_scheduler_tickers_never_fire_one_tick_twice` by duplicate rows |
| flip the scheduler default | `the_scheduler_is_off_by_default` |
| gate the scheduler on `cfg.enabled` | `the_scheduler_gate_is_independent_of_the_worker_gate` |

- [ ] **Step 7: Full gate; commit** — `feat(workflow): W12 — the scheduler ticker, config-gated, off by default`.

---

## Task 6: Coverage statement, sweep, and the whole-branch gate

**Files:** modify `crates/egret-workflow/oracle-exec/README.md`; modify `crates/egret-workflow/src/trigger/mod.rs` doc lines that say `TriggerState` is constructed only in tests; modify `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §6.1 (the row now describes a built table).

- [ ] **Step 1: Add the W12 section** to the README's coverage statement: the ratchet moved **109 → 109**, and it would read 109 with `fire_due` deleted — say both halves.
- [ ] **Step 2: Sweep the proposition "a cron cannot fire in production"** across the repo — three populations: the phrase (`cannot fire`, `NOT BUILT: W12`, `Not built: W12`, `constructed only in tests`, `no caller outside`), ordinals/counts (`one caller`, `exactly one`), and generated data. Report **N examined / M corrected** per population.
- [ ] **Step 3: Ratchet** — run the oracle's golden test, record the figure and the golden-entry count beside it.
- [ ] **Step 4: Full local CI gate** (per `egret-run-full-ci-local-before-merge`), name-set diff against the base measured in Global Constraints. `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`.
- [ ] **Step 5: Commit** — `docs(workflow): W12 coverage statement and the sweep`.

---

## Final review

Dispatch a whole-branch review on the most capable model. Point it at **predicates and transactions**, not figures: (1) any path that writes `workflow.flows` without `sync`; (2) any path in `fire_one` that commits an execution without advancing the dates, or advances without committing; (3) any off-switch that consumes a tick; (4) whether `sync`'s `CASE` can resurrect a stale `next_evaluation_date` after a spec change and a revert (A → B → A). Assume the branch has one Critical of its own.
