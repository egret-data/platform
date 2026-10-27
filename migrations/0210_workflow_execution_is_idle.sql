-- A terminal execution can still have live work: an `Exit` sibling (since
-- kill-and-exit) and `afterExecution:` (since package #2b). "Idle" is what the
-- sweeps mean when they ask whether an execution is over.
CREATE OR REPLACE FUNCTION workflow.execution_is_idle(execution uuid)
RETURNS boolean
LANGUAGE sql STABLE
AS $$
  SELECT workflow.state_is_terminal(e.state)
     AND NOT EXISTS (
       SELECT 1 FROM workflow.task_runs r
        WHERE r.execution_id = e.id
          AND NOT workflow.state_is_terminal(r.state))
    FROM workflow.executions e
   WHERE e.id = execution
$$;
