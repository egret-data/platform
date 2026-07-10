-- Fix forward on 0068: mirror workspace-member role UPDATEs into access_grants
-- (upsert-driven role changes fire UPDATE, previously unmirrored → stale grant),
-- and guard unresolvable role names so the trigger never aborts a legacy write.

CREATE OR REPLACE FUNCTION sync_workspace_grant() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE rid uuid;
BEGIN
    IF (TG_OP = 'INSERT') THEN
        SELECT id INTO rid FROM roles WHERE scope = 'workspace' AND name = NEW.role;
        IF rid IS NULL THEN RETURN NULL; END IF;
        INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
        VALUES (NEW.principal_type, NEW.principal_id, rid, 'workspace', NEW.workspace_id)
        ON CONFLICT DO NOTHING;
    ELSIF (TG_OP = 'DELETE') THEN
        SELECT id INTO rid FROM roles WHERE scope = 'workspace' AND name = OLD.role;
        IF rid IS NULL THEN RETURN NULL; END IF;
        DELETE FROM access_grants
        WHERE principal_type = OLD.principal_type AND principal_id = OLD.principal_id
          AND role_id = rid AND scope_type = 'workspace' AND scope_id = OLD.workspace_id;
    ELSIF (TG_OP = 'UPDATE') THEN
        SELECT id INTO rid FROM roles WHERE scope = 'workspace' AND name = OLD.role;
        IF rid IS NOT NULL THEN
            DELETE FROM access_grants
            WHERE principal_type = OLD.principal_type AND principal_id = OLD.principal_id
              AND role_id = rid AND scope_type = 'workspace' AND scope_id = OLD.workspace_id;
        END IF;
        SELECT id INTO rid FROM roles WHERE scope = 'workspace' AND name = NEW.role;
        IF rid IS NOT NULL THEN
            INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
            VALUES (NEW.principal_type, NEW.principal_id, rid, 'workspace', NEW.workspace_id)
            ON CONFLICT DO NOTHING;
        END IF;
    END IF;
    RETURN NULL;
END $$;

DROP TRIGGER sync_workspace_grant_trg ON workspace_members;
CREATE TRIGGER sync_workspace_grant_trg
AFTER INSERT OR UPDATE OR DELETE ON workspace_members
FOR EACH ROW EXECUTE FUNCTION sync_workspace_grant();
