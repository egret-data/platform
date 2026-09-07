-- Workflow W3b Task 4: the sub-executions a `Loop` spawns.
--
-- A `Loop` does not iterate inside its execution. Each value produces a
-- separate SUB-EXECUTION that runs the loop's body, and the body's tasks never
-- get a run in the parent -- measured over the whole golden: 41 entries carry
-- 142 sub-executions between them and not one loop-body task appears in its
-- parent's `taskRunList`. Migration 0134 has no way to say that, so
-- `Command::SpawnExecution` had nothing to write and `worker::claim::drive`
-- refused it.
--
-- These four columns are Kestra's `loopRun` (`{taskId, taskRunId, index,
-- value}`) written the way this schema addresses things: the taskrun by
-- `(parent_execution_id, parent_run_ix)`, which is the `RunIx` 0134 already
-- keys `task_runs` on, plus the index and the value.
ALTER TABLE workflow.executions
  -- The execution whose `Loop` run spawned this one. NULL on a top-level
  -- execution. ON DELETE CASCADE, not SET NULL: a sub-execution is a part of
  -- its parent's record rather than a thing that outlives it -- unlike
  -- `flow_id`, which is a link to the DRAFT and must survive the draft's
  -- deletion.
  ADD COLUMN parent_execution_id uuid REFERENCES workflow.executions(id) ON DELETE CASCADE,
  -- The `RunIx` of the `Loop` run inside the parent -- i.e. which taskrun
  -- spawned this. Kestra's `loopRun.taskRunId`, addressed positionally.
  ADD COLUMN parent_run_ix       integer,
  -- Kestra's `loopRun.index`: the flow's own numbering of its iterations,
  -- **0-based**. Measured on the artifact rather than read off a class: over
  -- `oracle-exec/traces.json` (harvested from the pinned digest) the 670 loop
  -- sub-executions carry indices whose per-spawning-taskrun sets are
  -- `{0}`, `{0,1}`, `{0,1,2}` and `{0,1,2,3}` -- contiguous from 0, never
  -- from 1, in every one of the 265 groups.
  ADD COLUMN loop_index          integer,
  -- Kestra's `loopRun.value`: the iteration value, already resolved to the
  -- string `executor::flowable::loop_::iteration_values` produces.
  ADD COLUMN loop_value          text;

-- All four or none of them: a sub-execution that knows its parent but not its
-- index is not orderable, and the ordering is the whole of what the oracle
-- compares `spawned` on. A CHECK rather than a rule every writer must
-- remember -- the writer that forgets returns 200, it does not fail.
ALTER TABLE workflow.executions
  ADD CONSTRAINT workflow_executions_loop_run_all_or_nothing CHECK (
    (parent_execution_id IS NULL
     AND parent_run_ix IS NULL AND loop_index IS NULL AND loop_value IS NULL)
    OR
    (parent_execution_id IS NOT NULL
     AND parent_run_ix IS NOT NULL AND loop_index IS NOT NULL AND loop_value IS NOT NULL)
  );

-- Two iterations of one loop run claiming the same index is exactly what
-- `oracle-exec/normalise.py` has to BAIL on ("the spawned order is not
-- determined"), because the tiebreak would silently fall back to the creation
-- instant -- and the creation instant carries no information at all: measured
-- over `traces.json`, all 670 loop sub-executions share their PARENT
-- execution's `startDate`, and all 220 sibling groups share one instant
-- between them. Making the collision unrepresentable here is cheaper than
-- detecting it there.
CREATE UNIQUE INDEX workflow_executions_loop_run_idx
  ON workflow.executions (parent_execution_id, parent_run_ix, loop_index)
  WHERE parent_execution_id IS NOT NULL;
