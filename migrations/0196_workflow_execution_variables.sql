-- Group C2 (spec §3.2): the execution's own variables, written by SetVariables
-- and UnsetVariables. NULL means "the flow's `variables:`", which is Kestra's
-- starting value (`Execution.newExecution` copies the flow's).
ALTER TABLE workflow.executions ADD COLUMN variables jsonb;
ALTER TABLE workflow.executions
  ADD CONSTRAINT workflow_executions_variables_is_object
  CHECK (variables IS NULL OR jsonb_typeof(variables) = 'object');
