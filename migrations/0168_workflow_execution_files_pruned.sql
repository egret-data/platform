-- 0168 (written as 0170; renumbered in W9a Task 14 on rebasing onto
-- origin/main, which owns 0161-0165 for Data Apps; relative order kept).
--
-- DOWN-COMPAT (a dev/kind database that already ran this under its old number):
-- sqlx keys applied migrations by version, so a database that ran the old
-- number 170 ("workflow execution files pruned") will refuse the renumbered files
-- (a missing or mismatched migration). The branch has not merged; there is no
-- production database in this state. Recover one of two ways:
--   1. reset it: `just reset-testdata` (drops and re-seeds the dev data); or
--   2. undo by hand, in this order, then let the server re-apply 0166-0168:
--        DROP TABLE IF EXISTS workflow.loop_file_values;
--        ALTER TABLE workflow.executions
--          DROP COLUMN IF EXISTS files_pruned_at,        -- takes workflow_executions_unswept_idx with it
--          DROP COLUMN IF EXISTS files_swept_at;
--        DROP TABLE IF EXISTS workflow.execution_files;
--        ALTER TABLE workflow.executions
--          DROP COLUMN IF EXISTS replayed_from_execution_id;  -- takes its FK, CHECKs and index
--        DELETE FROM public._sqlx_migrations
--         WHERE (version, description) IN
--           ((161, 'workflow execution files'),
--            (169, 'workflow loop file values'),
--            (170, 'workflow execution files pruned'));
--      Match on the description as well as the version: origin/main's own
--      0161-0165 (Data Apps) share those numbers on a database that took main
--      first. Use the schema your `_sqlx_migrations` lives in (`public` by default).
-- The local branch feat/sp4d-debt-and-guardrails also holds 0166-0168:
-- whichever branch merges second renumbers its files.
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
