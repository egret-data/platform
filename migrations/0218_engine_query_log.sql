-- StarRocks query log (audit platform side): the named secret holding the
-- password of the `egret_query_log` StarRocks user the AuditLoader plugin
-- stream-loads audit events as. NULL until the engine's query log is set up.
ALTER TABLE engine_mgmt.engines ADD COLUMN query_log_secret_id uuid;

-- The loader principal is an engine user egret runs as: no human may be minted
-- this name (see `egret_core::engine_principals::QUERY_LOG_LOADER_PRINCIPAL`).
INSERT INTO username_ledger (username, source, retired_at)
VALUES ('egret_query_log', 'reserved', now())
ON CONFLICT DO NOTHING;
