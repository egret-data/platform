-- 0161: origin/main ends at 0160 and no remote branch holds a 0161 (checked on
-- every `origin/*` ref before naming this file).
--
-- Workflow W9a Task 3: the index of an execution's files, and the manual
-- replay's link. See docs/en/specs/2026-10-01-workflow-w9a-internal-storage-design.md
-- §3 (URIs), §4.2 (this table) and §4.4 (who may read a URI).
--
-- * **A file exists for a flow only once its row here exists.** The object
--   store holds the bytes; this table is what `fileExists`/`fileSize` answer
--   from and what every read consults first (`storage::resolve_readable_many`).
--   An object with no row is an orphan the pruner sweeps (spec §10).
--
-- * `execution_id` is the WRITING execution: the `{exec}` of the URI, which
--   inside a `Loop` is the iteration's own id (spec §3, the user's ruling of
--   2026-10-01). Composite FK with `workspace_id` on 0138's
--   `workflow_executions_id_workspace_key`, so a row can never name another
--   workspace's execution, and `ON DELETE CASCADE` because a file row is a part
--   of its execution's record, as `task_runs` is.
--
-- * `task_run_id` carries NO foreign key, for 0159's reasons on
--   `task_logs.task_run_id`. NULL for an input file (no task run wrote it).
--
-- * `uri` is checked with `left(...)`, not `LIKE`: `_` and `%` are wildcards in
--   LIKE (memory `sql-like-underscore-dropped-a-database`).
--
-- * `UNIQUE (workspace_id, uri)`: a URI carries no workspace (spec §3), so the
--   same string can exist in two workspaces and names two different objects.
--   A retried task run writes the same deterministic URIs and upserts its row.
CREATE TABLE workflow.execution_files (
  id            bigserial   PRIMARY KEY,
  workspace_id  uuid        NOT NULL,
  execution_id  uuid        NOT NULL,        -- the WRITING execution: a Loop iteration's own id (spec §3, §4.2)
  task_run_id   uuid,                        -- NULL for an input file; no FK, as task_logs.task_run_id (0159)
  uri           text        NOT NULL CHECK (left(uri, 10) = 'kestra:///'),
  size          bigint      NOT NULL CHECK (size >= 0),
  created_at    timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT workflow_execution_files_uri_key UNIQUE (workspace_id, uri),
  CONSTRAINT workflow_execution_files_execution_same_workspace
    FOREIGN KEY (execution_id, workspace_id)
    REFERENCES workflow.executions (id, workspace_id) ON DELETE CASCADE
);
CREATE INDEX workflow_execution_files_execution_idx
  ON workflow.execution_files (execution_id);

-- The manual replay's link (spec §4.4 item 3). routes.rs `replay_execution`'s own doc
-- says "no column carries it" and names this column as the fix. NO ACTION on delete,
-- for 0138's reason on trigger_execution_id.
--
-- Not `parent_execution_id`: 0138's `workflow_executions_loop_run_idx` gives
-- `(X, NULL, NULL)` to the retry replay (`worker::replay::spawn_due`), one per
-- execution. A manual replay is a top-level execution with a link of its own,
-- and `..._is_not_a_child` keeps the two link kinds apart.
ALTER TABLE workflow.executions
  ADD COLUMN replayed_from_execution_id uuid;
ALTER TABLE workflow.executions
  ADD CONSTRAINT workflow_executions_replayed_from_same_workspace
    FOREIGN KEY (replayed_from_execution_id, workspace_id)
    REFERENCES workflow.executions (id, workspace_id),
  ADD CONSTRAINT workflow_executions_replayed_from_not_self
    CHECK (replayed_from_execution_id IS DISTINCT FROM id),
  ADD CONSTRAINT workflow_executions_replayed_from_is_not_a_child
    CHECK (replayed_from_execution_id IS NULL OR parent_execution_id IS NULL);

-- What `readable_executions` walks for the replay chain: "which execution is
-- this one the replay of" is answered from the row itself, but the pruner and
-- a delete look the other way ("is anything a replay of X"), and an FK with
-- no index under it makes every delete of an executions row scan the table.
CREATE INDEX workflow_executions_replayed_from_idx
  ON workflow.executions (replayed_from_execution_id)
  WHERE replayed_from_execution_id IS NOT NULL;
