-- Package 8a (flow concurrency): the `QUEUED` execution state.
--
-- `state` is plain text with no CHECK listing the vocabulary (as for KILLING,
-- see tests/execution_timestamps.rs), so `QUEUED` needs no constraint change.
-- Audited: `workflow.state_is_terminal` lists only the terminal states, so
-- QUEUED is not terminal; `workflow.execution_is_idle` (0210/0225) is true
-- only for a terminal state with no live run, so a QUEUED execution is not
-- idle; `*_ended_only_when_terminal` refuses an `ended_at` on a QUEUED row.
--
-- The drain (Task 3) promotes the oldest QUEUED execution of a flow; this
-- partial index serves that lookup and stays tiny (QUEUED rows are rare).
-- The per-flow live count reads `workflow_executions_flow_idx` (flow_id).
CREATE INDEX workflow_executions_queued_idx
  ON workflow.executions (flow_id, created_at, id)
  WHERE state = 'QUEUED';
