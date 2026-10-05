-- 0168 (written as 0170; renumbered in W9a Task 14 on rebasing onto
-- origin/main, which owns 0161-0165 for Data Apps; relative order kept).
--
-- Old number 170 ("workflow execution files pruned"). A database that applied that, or this
-- file before its header gained these comments, must be reset: recovery in
-- crates/egret-workflow/oracle-exec/README.md (W9a release note, "Database").
--
-- Workflow W9a Task 13: the retention pruner of execution files and the
-- orphan sweep (docs/en/specs/2026-10-01-workflow-w9a-internal-storage-design.md
-- §10; `egret_workflow::storage::prune`).
--
-- * `files_pruned_at` is what turns "no such file" into "it was deleted at
--   retention": `storage::resolve_readable_many` answers `Unreadable::Pruned`
--   for a readable URI with no index row whose execution has it set. It is set
--   on the loop root and every iteration under it, in the statement that
--   deletes their index rows, and only after their objects were deleted.
-- * `files_swept_at` marks a FAILED/KILLED loop family whose prefixes were
--   listed for orphans (objects uploaded and never indexed), so it is swept
--   once rather than every hour.
ALTER TABLE workflow.executions
  ADD COLUMN files_pruned_at timestamptz,   -- set when the pruner deleted this execution's files
  ADD COLUMN files_swept_at  timestamptz;   -- set when the orphan sweep of a FAILED/KILLED execution ran

-- Task 13 fix round 1. The sweep's candidates (`storage::prune::sweep_candidates`,
-- which spells the states as literals so this predicate matches): FAILED or
-- KILLED loop roots neither swept nor pruned, oldest first. Once swept or
-- pruned a row leaves the index, so it stays the size of the backlog.
CREATE INDEX workflow_executions_unswept_idx
  ON workflow.executions (updated_at, id)
  WHERE loop_index IS NULL AND files_swept_at IS NULL AND files_pruned_at IS NULL
    AND state IN ('FAILED', 'KILLED');

-- Executions that ended before W9a wrote no file under internal storage:
-- nothing to sweep. Marked so the first pass does not list every one of them.
UPDATE workflow.executions SET files_swept_at = now()
 WHERE state IN ('FAILED', 'KILLED');
