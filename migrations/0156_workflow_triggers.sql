-- 0156, not 0154: origin/main ends at 0153 and the unmerged SP4a branch holds 0154 and 0155, so 0156 collides with neither in either merge order.
--
-- Workflow W12: the trigger loop's queue. See
-- docs/en/specs/2026-09-24-workflow-w12-trigger-loop-design.md §3.
--
-- One row per `Schedule` trigger that a non-deleted flow's draft source declares,
-- whatever the flow's `state` (drafts fire too; `state` is read at fire time).
-- `scheduler::sync` is the only writer outside the loop. It runs inside the
-- transaction of `FlowRepo::create`, `update` and `soft_delete`, and once per
-- flow when the scheduler starts, which is what gives a flow saved before this
-- migration its rows (spec §3.3).
CREATE TABLE workflow.triggers (
  flow_id              uuid        NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  -- The trigger's `id:` in the YAML.
  trigger_id           text        NOT NULL,
  -- Copied from the flow for filtering without a join. Not an authority: the
  -- loop reads the flow row in the same statement that claims this one.
  workspace_id         uuid        NOT NULL,
  -- `cron|withSeconds|timezone`, readable rather than a digest. A change
  -- re-initialises the dates; any other edit to the flow leaves them alone.
  spec_key             text        NOT NULL,
  -- The flow's YAML `disabled:` OR the trigger's own. A column, not a
  -- fire-time predicate, so the claim can skip it without consuming a tick.
  disabled             boolean     NOT NULL,
  -- NULL = off the queue, and `last_error` says why. Written NULL by `sync` when
  -- the schedule is refused or has no future tick (`0 0 30 2 *`), and by the loop
  -- when the source no longer parses, resolves or plans at fire time. The next
  -- `sync` of the flow re-initialises it.
  next_evaluation_date timestamptz,
  evaluated_at         timestamptz,
  last_error           text,
  updated_at           timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (flow_id, trigger_id)
);
CREATE INDEX workflow_triggers_due ON workflow.triggers (next_evaluation_date)
  WHERE next_evaluation_date IS NOT NULL AND NOT disabled;

-- The tick an execution was fired for, so a double fire is unrepresentable
-- rather than merely prevented (spec §3.2).
ALTER TABLE workflow.executions ADD COLUMN scheduled_date timestamptz;
-- Set iff the execution was fired by a `Schedule`. 0140's
-- `workflow_executions_trigger_record_is_whole` already guarantees that a
-- non-NULL `execution_trigger` carries a string `type`, so the right-hand side
-- is never NULL and this CHECK cannot pass on an unknown.
ALTER TABLE workflow.executions ADD CONSTRAINT workflow_executions_scheduled_date_iff_schedule
  CHECK ((scheduled_date IS NOT NULL)
         = (execution_trigger IS NOT NULL
            AND execution_trigger->>'type' = 'io.kestra.plugin.core.trigger.Schedule'));
CREATE UNIQUE INDEX workflow_executions_one_per_schedule_tick
  ON workflow.executions (flow_id, (execution_trigger->>'id'), scheduled_date)
  WHERE scheduled_date IS NOT NULL;
