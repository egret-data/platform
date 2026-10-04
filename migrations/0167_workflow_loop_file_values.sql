-- 0167 (written as 0169; renumbered in W9a Task 14 on rebasing onto
-- origin/main, which owns 0161-0165 for Data Apps; relative order kept:
-- after 0166_workflow_execution_files, before 0168).
--
-- Workflow W9a Task 11, fix round 1: the values of a `Loop` over a file
-- (spec §9.3), one row per record, in file order.
--
-- * **Not in `task_runs.rendered`.** That column is read by every load of the
--   execution (`worker::claim::load`, each time an iteration ends), by every
--   iteration's render surface, and returned by the task-run API; a values
--   file is up to `workflow.storage.max_read_bytes` (10 MiB). The loop run's
--   `rendered` holds `"{uri}\n{count}"` instead, and an iteration's value is
--   read here by index when it is spawned.
-- * **Frozen per run.** Written once, in the transaction that renders the
--   loop run's `values:` (the same one that writes `rendered`), and never
--   updated: a run that is rendered again (a restart clears `rendered`)
--   replaces its rows in that same transaction.
-- * `ON DELETE CASCADE` from the execution, as `task_runs` (0134): the values
--   are part of the loop run's record and go with it at retention.
-- * `idx` is the iteration's `loop_index` (0135); no duplicate is removed --
--   Kestra's URI path has no `distinct()` (the user's ruling of 2026-10-03).
CREATE TABLE workflow.loop_file_values (
  execution_id  uuid    NOT NULL REFERENCES workflow.executions(id) ON DELETE CASCADE,
  run_ix        integer NOT NULL,
  idx           integer NOT NULL CHECK (idx >= 0),
  value         text    NOT NULL,
  PRIMARY KEY (execution_id, run_ix, idx)
);
