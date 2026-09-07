-- Workflow W3b Task 2: the rows a real driver claims from.
--
-- W3a built `advance()` as a pure function over `(plan, state, now)` and judged
-- it through a test harness that held `ExecutionState` in memory. Nothing
-- persisted it. These two tables are that state on disk, and they exist because
-- the claim loop needs something to claim: `workflow.flows` (0132) holds what an
-- author WROTE, and nothing anywhere held what is RUNNING.
--
-- The split mirrors `executor::ExecutionState`, which is `{ runs, history }`:
-- one `executions` row per execution, one `task_runs` row per
-- `ExecutionState::runs` entry, keyed by that vector's index. The index is the
-- key rather than a surrogate because `advance()` addresses runs by `RunIx` --
-- their CREATION order -- and the oracle compares that order, so a store that
-- could not reproduce it would be storing a different thing.
--
-- `UNIQUE (execution_id, run_ix)` is load-bearing rather than hygiene: it is
-- what makes "the same successor created twice" unrepresentable, so two workers
-- that both advanced the same execution collide instead of duplicating. The
-- `SELECT ... FOR UPDATE` the worker takes on the execution row is what stops
-- them getting that far; the constraint is what happens if that ever fails.
CREATE TABLE workflow.executions (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  -- The draft this execution came from, if any. Nullable and ON DELETE SET NULL
  -- for the same reason `flows.folder_id` is: deleting a flow must not delete
  -- the record of what it did. `source` below is the copy this execution
  -- actually runs, so a deleted flow does not make a running execution
  -- unreadable.
  flow_id       uuid REFERENCES workflow.flows(id) ON DELETE SET NULL,
  namespace     text NOT NULL,
  -- The YAML's `id`, as on `workflow.flows`; `flow_id` is taken by the FK.
  flow_key      text NOT NULL,
  -- The flow source this execution runs, frozen at start. `executor::build_plan`
  -- is pure and deterministic over it, so the `Plan` is rebuilt each tick rather
  -- than stored: a stored plan is a second copy of a derivable thing that can
  -- disagree with the source beside it.
  source        text NOT NULL,
  -- `ExecutionState::history`, as Kestra's wire spellings. `state` is its last
  -- element, denormalised so a list page does not have to open the array.
  state         text NOT NULL,
  history       jsonb NOT NULL,
  created_at    timestamptz NOT NULL DEFAULT now(),
  updated_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE workflow.task_runs (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  execution_id    uuid NOT NULL REFERENCES workflow.executions(id) ON DELETE CASCADE,
  -- The `RunIx` -- the index into `ExecutionState::runs`, i.e. creation order.
  run_ix          integer NOT NULL,
  -- The `TaskIx` -- the index into `Plan::tasks`, and `task_id` is that task's
  -- author-written id, carried so a human reading this table does not have to
  -- rebuild the plan to know what a row is.
  task_ix         integer NOT NULL,
  task_id         text NOT NULL,
  -- The task's `type:`, and NULL exactly when the planned kind is NOT
  -- `TaskKind::Runnable` -- i.e. when the executor runs this task itself and no
  -- worker ever should. It is the claim predicate's whole way of telling a
  -- flowable from a runnable, because their RUNNING states are otherwise
  -- identical in this table: a `Parallel` goes CREATED RUNNING SUCCESS and its
  -- children go CREATED SUBMITTED RUNNING SUCCESS, and only the executor knows
  -- which is which (`executor::flowable::start`).
  type_id         text,
  parent_ix       integer,
  value           text,
  rendered        text,
  rendered_run_if text,
  state           text NOT NULL,
  history         jsonb NOT NULL,
  attempts        jsonb NOT NULL DEFAULT '[]'::jsonb,
  -- `TaskRun::not_before`, a `Tick`: milliseconds since the EXECUTION was
  -- created, not an absolute instant. Stored in the unit `executor` uses so the
  -- two cannot drift; `executions.created_at` is the origin.
  not_before_ms   bigint,
  -- The lease. NULL means unclaimed. Set by the claim, cleared when the
  -- worker's result commits, and reclaimed by the reaper when it is older than
  -- the worker timeout -- the failure `SKIP LOCKED` cannot prevent, which
  -- `crates/egret-automation/src/scheduler.rs`'s module doc describes for the
  -- identical statement.
  claimed_at      timestamptz,
  created_at      timestamptz NOT NULL DEFAULT now(),
  updated_at      timestamptz NOT NULL DEFAULT now(),
  UNIQUE (execution_id, run_ix)
);

-- The claim's predicate, as a partial index: only a runnable can ever be
-- claimed, so the index holds only runnables.
CREATE INDEX workflow_task_runs_claimable_idx
  ON workflow.task_runs (state, claimed_at)
  WHERE type_id IS NOT NULL;

CREATE INDEX workflow_executions_ws_idx ON workflow.executions (workspace_id);
CREATE INDEX workflow_executions_flow_idx ON workflow.executions (flow_id);
