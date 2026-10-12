-- 0183_workflow_execution_timestamps.sql
--
-- W7b Task 1 (spec `docs/en/specs/2026-10-11-workflow-w7b-executions-design.md`
-- §3): real start and end times for executions and task runs. The executions
-- list's `root_only`/`parent=` filters need no new index (see the end of this
-- file).
--
-- Until now the only "end" a run had was `updated_at`, which moves on every
-- write. The gantt needs an instant that is written once per life of the row,
-- by the write that ends it, and kept while the row stays terminal. It is
-- cleared when the row leaves a terminal state, and written afresh when a
-- `LoopUntil` rebuilds the row.
--
-- WHO WRITES THESE COLUMNS
--
-- Every write is part of the same statement that writes `state`, through the
-- macros in `crates/egret-workflow/src/worker/stamps.rs`, which hold the full
-- rule. In short, the rule is judged PER TRANSITION, not by "is":
--
-- * `started_at` is set to `now()` by the write that takes the row into
--   `RUNNING` for the FIRST time. It is kept after that, so a resumed pause
--   or a retry does not move it. It is `NULL` when the history does not hold
--   `RUNNING` (a `SKIPPED` run, or a `LoopUntil` descendant that
--   `RestartLoop` emptied).
-- * `ended_at` is set by the write in which the row goes from a non-terminal
--   state to a terminal one. It is kept while the row stays terminal (an
--   outputs write, a terminal-to-terminal move). It is `NULL` when the row is
--   not terminal (a `RETRY_FAILED_TASK` reopen clears it).
--
-- A write that only touches a row that was already running or terminal does
-- NOT stamp it. That is what keeps the rows below honest.
--
-- Existing rows keep `NULL` in all four columns. Nothing is guessed from
-- `updated_at`; the UI marks such times as estimated (spec §6.3).

ALTER TABLE workflow.executions
  ADD COLUMN started_at timestamptz,
  ADD COLUMN ended_at   timestamptz;

ALTER TABLE workflow.task_runs
  ADD COLUMN started_at timestamptz,
  ADD COLUMN ended_at   timestamptz;

-- The one terminal list in SQL. It is exactly `models::State::is_terminated`
-- (spec §3.2's authority), which is `RunState::is_terminal` plus
-- `RESUBMITTED`. No `RunState` spells `RESUBMITTED`, so no writer produces it
-- today. `tests/execution_timestamps.rs` pins this function against every
-- variant of both enums, so a terminal state added in Rust and not here fails
-- a test rather than a write in production.
CREATE FUNCTION workflow.state_is_terminal(state text) RETURNS boolean
  LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
  AS $$ SELECT state IN ('SUCCESS', 'FAILED', 'RETRIED', 'KILLED', 'SKIPPED',
                         'WARNING', 'CANCELLED', 'RESUBMITTED') $$;

-- Only a terminal row has an end, and an end is never before the start. The
-- writers cannot break either: the helper's `GREATEST` covers a transaction
-- whose `now()` predates the one that set `started_at`, which a lock wait can
-- produce. These catch PART of a bypass, an end on a live row or an end
-- before its start. They do not catch a bypass that writes a terminal state
-- and leaves `ended_at` NULL; that row is indistinguishable from a pre-0183
-- one. Existing rows hold NULLs and satisfy both.
ALTER TABLE workflow.executions
  ADD CONSTRAINT workflow_executions_ended_only_when_terminal
    CHECK (ended_at IS NULL OR workflow.state_is_terminal(state)),
  ADD CONSTRAINT workflow_executions_ends_after_it_starts
    CHECK (ended_at >= started_at);

ALTER TABLE workflow.task_runs
  ADD CONSTRAINT workflow_task_runs_ended_only_when_terminal
    CHECK (ended_at IS NULL OR workflow.state_is_terminal(state)),
  ADD CONSTRAINT workflow_task_runs_ends_after_it_starts
    CHECK (ended_at >= started_at);

-- NO NEW INDEX for the list's `parent=<id>` filter, and that is a
-- measurement. The spec (§3.3) asked for `(workspace_id, parent_execution_id)`
-- and `(workspace_id, trigger_execution_id)`, partial on non-null. With them
-- built, `EXPLAIN` of the list's own statement chose NEITHER: it answers
-- `parent_execution_id = $9 OR trigger_execution_id = $9` with a BitmapOr over
-- the two partial indexes that already lead with those columns --
-- `workflow_executions_loop_run_idx` (0135, rebuilt in 0138) and
-- `workflow_executions_trigger_idx` (0138) -- and filters the workspace on the
-- one or two rows they return. An id is selective on its own; a leading
-- `workspace_id` adds nothing to it. Two indexes nothing reads would be a
-- write cost on every child execution, so they are not built.
-- `tests/execution_timestamps.rs::the_parent_filter_is_served_by_the_partial_indexes`
-- pins the plan, so a change that loses it fails there.
--
-- `root_only` (`parent_execution_id IS NULL AND trigger_execution_id IS NULL`)
-- cannot use a partial index on NOT NULL and does not need one: it is the
-- workspace scan every list already is (`workflow_executions_ws_idx`), with a
-- filter.
