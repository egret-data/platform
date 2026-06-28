-- DB connections are per-workspace (ADR-19): a connection name only needs to be
-- unique within its workspace, not globally. Two workspaces may legitimately
-- register a connection with the same name. The old global UNIQUE(name) was a
-- leftover from the register-then-grant model (Epic A deferred this).
ALTER TABLE databases DROP CONSTRAINT databases_name_key;
ALTER TABLE databases ADD CONSTRAINT databases_name_workspace_key UNIQUE (workspace_id, name);
