-- W9b: namespace files (docs/en/specs/2026-10-05-workflow-w9b-namespace-files-design.md §4.1).
-- One row per revision; a path's state is its highest revision's row. A namespace file
-- exists only while its row does. No FK to a namespace: there is no namespace table, and
-- a namespace with no flow can hold files.
CREATE TABLE workflow.namespace_files (
  id                   bigserial   PRIMARY KEY,
  workspace_id         uuid        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  namespace            text        NOT NULL CHECK (namespace <> ''),
  -- normalised by nsfiles::path::NsPath: a leading '/', a directory ends with '/'
  path                 text        NOT NULL CHECK (left(path, 1) = '/' AND path <> '/'),
  revision             integer     NOT NULL CHECK (revision >= 1),
  is_directory         boolean     NOT NULL,
  size                 bigint      NOT NULL DEFAULT 0 CHECK (size >= 0),
  deleted              boolean     NOT NULL DEFAULT false,
  created_at           timestamptz NOT NULL DEFAULT now(),
  -- who wrote it: a user (the API) or an execution (a task); no FK, the row outlives its writer
  created_by_user      uuid,
  created_by_execution uuid,
  CONSTRAINT workflow_namespace_files_revision_key UNIQUE (workspace_id, namespace, path, revision),
  CONSTRAINT workflow_namespace_files_directory_shape
    CHECK (is_directory = (right(path, 1) = '/') AND (NOT is_directory OR (revision = 1 AND size = 0))),
  CONSTRAINT workflow_namespace_files_one_writer
    CHECK (num_nonnulls(created_by_user, created_by_execution) <= 1)
);
