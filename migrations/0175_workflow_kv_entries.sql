-- W9c: the KV store (docs/en/specs/2026-10-07-workflow-w9c-kv-store-design.md §4.1).
-- One row per revision; a key's state is its highest revision's row. A KV value exists only
-- once its row exists. No FK to a namespace: there is no namespace table, and a namespace
-- with no flow can hold keys. The unique constraint's index is also the latest-row index.
CREATE TABLE workflow.kv_entries (
  id                   bigserial   PRIMARY KEY,
  workspace_id         uuid        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  namespace            text        NOT NULL CHECK (namespace <> ''),
  -- storage::kv::key::check_key: Kestra's regex (KVStore.java:137), at most 350 bytes
  key                  text        NOT NULL
    CHECK (octet_length(key) <= 350 AND key COLLATE "C" ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]*$'),
  revision             integer     NOT NULL CHECK (revision >= 1),
  value_type           text        NOT NULL
    CHECK (value_type IN ('STRING','NUMBER','BOOLEAN','DATETIME','DATE','DURATION','JSON')),
  size                 bigint      NOT NULL CHECK (size >= 0),
  description          text,
  -- now() + ttl at the commit, by the database's clock; NULL never expires
  expires_at           timestamptz,
  deleted              boolean     NOT NULL DEFAULT false,
  deleted_at           timestamptz,
  -- who soft-deleted it: a re-claimed Delete recognises its own earlier delete (§8.4, §11)
  deleted_by_user      uuid,
  deleted_by_execution uuid,
  deleted_by_task_run  uuid,
  created_at           timestamptz NOT NULL DEFAULT now(),
  created_by_user      uuid,
  created_by_execution uuid,
  created_by_task_run  uuid,
  CONSTRAINT workflow_kv_entries_revision_key UNIQUE (workspace_id, namespace, key, revision),
  CONSTRAINT workflow_kv_entries_one_writer
    CHECK (num_nonnulls(created_by_user, created_by_execution) <= 1),
  CONSTRAINT workflow_kv_entries_task_run_of_an_execution
    CHECK (created_by_task_run IS NULL OR created_by_execution IS NOT NULL),
  CONSTRAINT workflow_kv_entries_deleted_shape
    CHECK (deleted = (deleted_at IS NOT NULL)
           AND (deleted OR num_nonnulls(deleted_by_user, deleted_by_execution, deleted_by_task_run) = 0)),
  CONSTRAINT workflow_kv_entries_one_deleter
    CHECK (num_nonnulls(deleted_by_user, deleted_by_execution) <= 1
           AND (deleted_by_task_run IS NULL OR deleted_by_execution IS NOT NULL))
);
-- The expiry sweep's scan (§11).
CREATE INDEX workflow_kv_entries_expires_at ON workflow.kv_entries (expires_at)
  WHERE expires_at IS NOT NULL;
