-- Workflow package 7a (docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md,
-- D7 and D12): a restart in place raises `attempt_number` on ANY execution.
--
-- 0139 tied `attempt_number > 1` to the replay shape (a parent with no loop
-- columns), because only `worker::replay::spawn_one` wrote it. A restart now
-- writes it too, as Kestra's `ExecutionService.restart` calls
-- `ExecutionMetadata.nextAttempt()` (`ES:358`), on a top-level execution, a
-- `Subflow` child or a loop iteration alike. `origin_offset_ms` stays the
-- replay's alone: a restart leaves it unchanged, so a `CREATE_NEW_EXECUTION`
-- retry still measures `maxDuration` from the original creation, and a
-- non-zero offset still requires the replay shape.
ALTER TABLE workflow.executions
  DROP CONSTRAINT workflow_executions_metadata_belongs_to_a_replay;

ALTER TABLE workflow.executions
  ADD CONSTRAINT workflow_executions_metadata_belongs_to_a_replay CHECK (
    attempt_number >= 1
    AND (origin_offset_ms = 0
         OR (attempt_number > 1
             AND parent_execution_id IS NOT NULL
             AND parent_run_ix IS NULL))
  );

-- **An emptied run is not live work** (spec D1, D4). A restart empties a run
-- Kestra would delete -- an `errors:`/`finally:`/`afterExecution:` run, a
-- `WorkingDirectory`'s children -- and the walk builds it back only when its
-- turn comes; `Command::RestartLoop` has done the same to a `LoopUntil`'s
-- descendants since W8. `persist` writes such a row with `history = []` and
-- state `CREATED`, and a run the walk never reaches again (an `errors:`
-- branch after the restarted execution succeeds) keeps that row for good.
-- Read as live, it left the execution never idle: the pruner, the container
-- sweep and a later restart all saw it as still running. Such a row is not a
-- run at all (Kestra has none), so it is left out here. No other row has an
-- empty history: `Command::CreateRun` writes `[CREATED]`.
--
-- The predicate is ONE function, `workflow.run_is_live`, shared by every
-- query that asks whether a run is still live work: this one, the kill's
-- descendants and its `afterExecution:` phase count, and the force-close
-- (final review M6). Before it, those three still read an emptied row as
-- live, and the force-close wrote a `[KILLED]` history onto it.
CREATE FUNCTION workflow.run_is_live(state text, history jsonb)
RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
AS $$
  SELECT NOT workflow.state_is_terminal(state) AND jsonb_array_length(history) > 0
$$;

CREATE OR REPLACE FUNCTION workflow.execution_is_idle(execution uuid)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT workflow.state_is_terminal(e.state)
     AND NOT EXISTS (
       SELECT 1 FROM workflow.task_runs r
        WHERE r.execution_id = e.id
          AND workflow.run_is_live(r.state, r.history))
    FROM workflow.executions e
   WHERE e.id = execution
$$;
