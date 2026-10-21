-- Rejected Ranger plugin credentials, per engine (spec
-- 2026-10-20-ranger-surface-auth, A12). The Governance Overview raises
-- `engine_auth_failing` for an engine whose latest rejection is newer than its
-- latest successful download.
--
-- One row per engine whose username parsed as an engine id AND that exists:
-- the writer's INSERT selects from `engine_mgmt.engines`, so a caller inventing
-- `engine-<uuid>` usernames adds no row. An unparseable username has no key to
-- count under and is not stored.
--
-- `count` is a lower bound: the writer coalesces rejections per engine in
-- memory (at most one upsert per engine per window), so a flood of bad
-- credentials costs one write per window, and the rejections still pending
-- when the flood stops are not added.
--
-- `ok_at` is the engine's latest successful download recorded while a
-- rejection was outstanding. The ledger `policy.ranger_plugin_downloads` cannot
-- say this: it is keyed by profile and plugin id, not by engine, so a healthy
-- engine sharing the profile would hide a failing one. Rows are never deleted
-- on success; a success newer than `last_at` clears the finding.
CREATE TABLE policy.ranger_auth_failures (
    engine_id uuid        PRIMARY KEY,
    count     bigint      NOT NULL DEFAULT 1 CHECK (count > 0),
    last_at   timestamptz NOT NULL DEFAULT now(),
    ok_at     timestamptz
);

-- No foreign key into engine_mgmt (a separate seam). A deleted engine's row is
-- removed by this trigger instead, the way 0187 couples the two schemas, so a
-- row never outlives its engine and the table stays bounded by live engines.
CREATE FUNCTION engine_mgmt.forget_ranger_auth_failures() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM policy.ranger_auth_failures WHERE engine_id = OLD.id;
    RETURN NULL;
END $$;

CREATE TRIGGER engines_delete_forgets_ranger_auth_failures
    AFTER DELETE ON engine_mgmt.engines
    FOR EACH ROW EXECUTE FUNCTION engine_mgmt.forget_ranger_auth_failures();
