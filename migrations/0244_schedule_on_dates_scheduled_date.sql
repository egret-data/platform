-- Package 11e: `ScheduleOnDates` fires on an idempotent date tick, like
-- `Schedule`, so it writes `scheduled_date` and gets the same double-fire
-- guard. 0156's CHECK admitted only the `Schedule` type; broaden it to admit
-- `ScheduleOnDates` too. `workflow_executions_one_per_schedule_tick` (0156,
-- 0192, 0243) keys on `(flow_id, trigger id, scheduled_date)` and is already
-- type-agnostic: no change.
ALTER TABLE workflow.executions DROP CONSTRAINT workflow_executions_scheduled_date_iff_schedule;
ALTER TABLE workflow.executions ADD CONSTRAINT workflow_executions_scheduled_date_iff_schedule
  CHECK ((scheduled_date IS NOT NULL)
         = (execution_trigger IS NOT NULL
            AND execution_trigger->>'type' IN (
              'io.kestra.plugin.core.trigger.Schedule',
              'io.kestra.plugin.core.trigger.ScheduleOnDates')));
