-- Package D (docs/en/specs/2026-11-11-starrocks-delegation-and-lifecycle-design.md, section 2):
-- egret creates a StarRocks user for each person who may query an impersonate-mode StarRocks
-- engine, and drops it when they no longer may.

-- 1. The StarRocks users egret created, per engine. egret drops only these; a StarRocks user it
--    did not create is never touched. The rows go with the engine.
CREATE TABLE engine_mgmt.starrocks_users (
    engine_id  uuid NOT NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE,
    username   text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (engine_id, username)
);

-- 2. `root` is StarRocks' superuser: no person may be minted that name
--    (`egret_core::engine_principals::STARROCKS_ROOT_USER`), as 0209 reserved `egret_delegator`
--    and 0218 `egret_query_log`. A person who already holds it keeps it; the sync refuses it.
INSERT INTO username_ledger (username, source, retired_at)
VALUES ('root', 'reserved', now())
ON CONFLICT DO NOTHING;

-- 3. Wake the sync (`egret_engine_mgmt::starrocks_users`, channel `egret_starrocks_user_sync`)
--    after every committed write that can change who may query an engine. Triggers, not Rust
--    calls, for the reason 0209 gives for the ledger: they cover every writer. Postgres delivers a
--    NOTIFY only at commit, once per payload per transaction.
CREATE FUNCTION engine_mgmt.notify_starrocks_user_sync() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('egret_starrocks_user_sync', '*');
    RETURN NULL;
END
$$;

CREATE TRIGGER starrocks_user_sync_access_grants
    AFTER INSERT OR UPDATE OR DELETE ON access_grants
    FOR EACH STATEMENT EXECUTE FUNCTION engine_mgmt.notify_starrocks_user_sync();
CREATE TRIGGER starrocks_user_sync_user_groups
    AFTER INSERT OR UPDATE OR DELETE ON user_groups
    FOR EACH STATEMENT EXECUTE FUNCTION engine_mgmt.notify_starrocks_user_sync();
CREATE TRIGGER starrocks_user_sync_users
    AFTER UPDATE OF status, username OR DELETE ON users
    FOR EACH STATEMENT EXECUTE FUNCTION engine_mgmt.notify_starrocks_user_sync();
CREATE TRIGGER starrocks_user_sync_engine_workspaces
    AFTER INSERT OR DELETE ON engine_mgmt.engine_workspaces
    FOR EACH STATEMENT EXECUTE FUNCTION engine_mgmt.notify_starrocks_user_sync();
-- Platform admins are synced too (decided on 2026-11-11, 1): their grants are `access_grants`
-- rows (scope 'platform', above); the role that makes them admins is the `roles` row named
-- `platform_admin`, so a rename or delete of a role wakes the sync as well.
CREATE TRIGGER starrocks_user_sync_roles
    AFTER UPDATE OF name OR DELETE ON roles
    FOR EACH STATEMENT EXECUTE FUNCTION engine_mgmt.notify_starrocks_user_sync();
