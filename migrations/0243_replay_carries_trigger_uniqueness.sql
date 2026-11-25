-- Package 11a: a replay carries the original execution's trigger record
-- verbatim (Kestra `Execution.java:422`), and 0156/0157's CHECKs couple
-- `scheduled_date` / `flow_trigger_event_id` to it, so the replay row repeats
-- the original's `(flow_id, trigger id, scheduled_date | event id)`.
-- These two indexes are one-execution-per-FIRE guarantees; a replay is an
-- intentional re-run, not a second fire, so it is excluded (as 0192 excluded
-- backfills).
DROP INDEX workflow.workflow_executions_one_per_schedule_tick;
CREATE UNIQUE INDEX workflow_executions_one_per_schedule_tick
  ON workflow.executions (flow_id, (execution_trigger->>'id'), scheduled_date)
  WHERE scheduled_date IS NOT NULL AND backfill_id IS NULL
    AND replayed_from_execution_id IS NULL;

DROP INDEX workflow.workflow_executions_one_per_flow_trigger_event;
CREATE UNIQUE INDEX workflow_executions_one_per_flow_trigger_event
  ON workflow.executions (flow_trigger_event_id, flow_id, (execution_trigger->>'id'))
  WHERE flow_trigger_event_id IS NOT NULL AND replayed_from_execution_id IS NULL;
