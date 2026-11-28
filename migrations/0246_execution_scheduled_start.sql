-- Subflow scheduleDate (package #12b): the instant a deferred child execution
-- should start. NULL for every normal execution; set on a Subflow child whose
-- scheduleDate rendered to a FUTURE instant, which is parked CREATED with no
-- first settle. Independent of scheduled_date (the scheduler tick identity,
-- 0156/0244): a Subflow child is not a Schedule/ScheduleOnDates fire, so its
-- deferral instant cannot live in scheduled_date (bound by that CHECK and part
-- of the schedule-tick uniqueness index) -- hence a dedicated column with no
-- CHECK coupling. The waker (`claim::wake`) claims a CREATED child once this is
-- due (`scheduled_start <= now()`) and drives its first settle.
ALTER TABLE workflow.executions ADD COLUMN scheduled_start timestamptz;

CREATE INDEX workflow_executions_scheduled_start
  ON workflow.executions (scheduled_start)
  WHERE scheduled_start IS NOT NULL;
