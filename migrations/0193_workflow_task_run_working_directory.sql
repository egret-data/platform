-- A `WorkingDirectory` block's claim phase (spec §5.1, plan Task 5), kept
-- outside the run's history: a claim is work the worker does for the block,
-- not a state Kestra's trace shows. NULL on every run that is not such a block.
ALTER TABLE workflow.task_runs
  ADD COLUMN working_dir text;

ALTER TABLE workflow.task_runs
  ADD CONSTRAINT workflow_task_runs_working_dir_known CHECK (
    working_dir IS NULL
    OR working_dir IN ('PREPARING', 'PREPARED', 'FINISHING', 'FINISHED')
  );
