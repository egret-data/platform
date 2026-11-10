-- StarRocks query log (audit platform side): the named secret holding the
-- password of the `egret_query_log` StarRocks user the AuditLoader plugin
-- stream-loads audit events as. NULL until the engine's query log is set up.
ALTER TABLE engine_mgmt.engines ADD COLUMN query_log_secret_id uuid;

-- The loader principal is an engine user egret runs as: no human may be minted
-- this name (see `egret_core::engine_principals::QUERY_LOG_LOADER_PRINCIPAL`).
INSERT INTO username_ledger (username, source, retired_at)
VALUES ('egret_query_log', 'reserved', now())
ON CONFLICT DO NOTHING;

-- `engine:audit` ("Read statement text in engine query logs", open question 1
-- decided (b)): the Query log tab shows the statement text only to holders of
-- it. Every platform role that may update an engine may read what it ran.
-- `platform_admin` needs no row: its Cedar template is unrestricted.
INSERT INTO role_permissions (role_id, permission)
SELECT rp.role_id, 'engine:audit'
FROM role_permissions rp
JOIN roles r ON r.id = rp.role_id
WHERE r.scope = 'platform' AND rp.permission = 'engine:update'
ON CONFLICT DO NOTHING;
