-- B3a: per-connection per-user delegation. When true, BI queries against this
-- connection run as the caller's per-user engine JWT (delegation) instead of the
-- shared service credentials. Default false = current shared-service behavior.
ALTER TABLE query.databases ADD COLUMN delegated boolean NOT NULL DEFAULT false;
