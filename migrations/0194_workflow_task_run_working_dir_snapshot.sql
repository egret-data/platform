-- A `WorkingDirectory` block's snapshot pointer (spec §4.3, §5.2): the
-- object URI of the directory the next child starts from. Written under the
-- fence of the claim that produced it (the prepare claim, or the child whose
-- copy-out was admitted); read by the next child's claim. NULL on every run
-- that is not a block, and on a block before its prepare claim.
ALTER TABLE workflow.task_runs
  ADD COLUMN working_dir_snapshot text;

ALTER TABLE workflow.task_runs
  ADD CONSTRAINT workflow_task_runs_working_dir_snapshot_on_block CHECK (
    working_dir_snapshot IS NULL OR working_dir IS NOT NULL
  );
