-- Debt package D6 (spec §3.1): when a WorkingDirectory block's `_wd/{block}/`
-- snapshots were removed. `worker::working_dir::discard` stamps it after a
-- delete that succeeded; `storage::wd_sweep` removes what a crash or a failed
-- delete left, 24 h after the block's run ended, and stamps it, so each
-- crashed block is listed once rather than on every pass.
ALTER TABLE workflow.task_runs ADD COLUMN working_dir_swept_at timestamptz;
ALTER TABLE workflow.task_runs
  ADD CONSTRAINT workflow_task_runs_working_dir_swept_on_block CHECK (
    working_dir_swept_at IS NULL OR working_dir IS NOT NULL
  );
CREATE INDEX workflow_task_runs_wd_unswept
  ON workflow.task_runs (ended_at)
  WHERE working_dir IS NOT NULL AND working_dir_swept_at IS NULL;
