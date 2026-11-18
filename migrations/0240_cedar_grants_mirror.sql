-- 0240_cedar_grants_mirror.sql
--
-- EO4: the unified-store mirror of `access_grants`'s decision-relevant columns.
-- A trigger on `access_grants` keeps it live-consistent for every future write,
-- so the `unified` read path (perm.cedar_grant_source = 'unified') never serves a
-- stale grant. `AccessGrantStore::fold` backfills the rows that existed before
-- this migration. The mirror carries the SAME shape `grants_for` reads, so the
-- unified query is the legacy query against this table.
--
-- Reversible: the switch (0239) merely chooses which table `grants_for` reads;
-- `access_grants` stays the write target and is untouched, so undo is instant.
-- Deleting `access_grants` and redirecting writes here is the deferred finalize
-- (EO4-D5), NOT this migration.
CREATE TABLE perm.cedar_grants (
    id             uuid PRIMARY KEY,                    -- mirrors access_grants.id
    principal_type text NOT NULL CHECK (principal_type IN ('user','group','service')),
    principal_id   uuid NOT NULL,
    role_id        uuid NOT NULL,
    scope_type     text NOT NULL CHECK (scope_type IN ('platform','workspace','object')),
    scope_id       uuid,
    object_type    text,
    effect         text NOT NULL CHECK (effect IN ('allow','deny')),
    expires_at     timestamptz
);
CREATE INDEX cedar_grants_principal_idx ON perm.cedar_grants (principal_type, principal_id);
CREATE INDEX cedar_grants_scope_idx ON perm.cedar_grants (scope_type, scope_id);

-- Keep the mirror in lock-step with access_grants writes (INSERT/UPDATE/DELETE).
CREATE OR REPLACE FUNCTION perm.sync_cedar_grants() RETURNS trigger AS $$
BEGIN
    IF (TG_OP = 'DELETE') THEN
        DELETE FROM perm.cedar_grants WHERE id = OLD.id;
        RETURN OLD;
    END IF;
    INSERT INTO perm.cedar_grants
        (id, principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, expires_at)
    VALUES
        (NEW.id, NEW.principal_type, NEW.principal_id, NEW.role_id, NEW.scope_type,
         NEW.scope_id, NEW.object_type, NEW.effect, NEW.expires_at)
    ON CONFLICT (id) DO UPDATE SET
        principal_type = EXCLUDED.principal_type, principal_id = EXCLUDED.principal_id,
        role_id = EXCLUDED.role_id, scope_type = EXCLUDED.scope_type, scope_id = EXCLUDED.scope_id,
        object_type = EXCLUDED.object_type, effect = EXCLUDED.effect, expires_at = EXCLUDED.expires_at;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER access_grants_mirror
    AFTER INSERT OR UPDATE OR DELETE ON access_grants
    FOR EACH ROW EXECUTE FUNCTION perm.sync_cedar_grants();
