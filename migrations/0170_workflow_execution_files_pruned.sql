-- 0170: origin/main holds 0161-0165 and this branch 0161 + 0169; 0170 is clear
-- of both (this branch's own 0161 is renumbered at rebase, W9a Task 14).
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
