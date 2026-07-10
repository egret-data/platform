-- Unified grant store (Effort B / B1). Expand phase: access_grants is the
-- read-model for the PDP; the two legacy tables remain the write path and are
-- mirrored into access_grants by triggers until B1b flips writers and drops them.

CREATE TABLE access_grants (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    principal_type text NOT NULL CHECK (principal_type IN ('user','group','service')),
    principal_id   uuid NOT NULL,
    role_id        uuid NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    scope_type     text NOT NULL CHECK (scope_type IN ('platform','workspace','object')),
    scope_id       uuid,
    object_type    text,
    granted_by     uuid,
    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (principal_type, principal_id, role_id, scope_type, scope_id)
);
CREATE INDEX access_grants_principal_idx ON access_grants (principal_type, principal_id);
CREATE INDEX access_grants_scope_idx ON access_grants (scope_type, scope_id);

-- Backfill: platform assignments.
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
SELECT principal_type, principal_id, role_id, 'platform', NULL
FROM platform_role_assignments
ON CONFLICT DO NOTHING;

-- Backfill: workspace memberships (map role NAME -> workspace-scoped role id).
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
SELECT wm.principal_type, wm.principal_id, r.id, 'workspace', wm.workspace_id
FROM workspace_members wm
JOIN roles r ON r.scope = 'workspace' AND r.name = wm.role
ON CONFLICT DO NOTHING;

-- Sync triggers: mirror legacy writes into access_grants during the expand phase.
CREATE FUNCTION sync_platform_grant() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (TG_OP = 'INSERT') THEN
        INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
        VALUES (NEW.principal_type, NEW.principal_id, NEW.role_id, 'platform', NULL)
        ON CONFLICT DO NOTHING;
    ELSIF (TG_OP = 'DELETE') THEN
        DELETE FROM access_grants
        WHERE principal_type = OLD.principal_type AND principal_id = OLD.principal_id
          AND role_id = OLD.role_id AND scope_type = 'platform';
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_platform_grant_trg
AFTER INSERT OR DELETE ON platform_role_assignments
FOR EACH ROW EXECUTE FUNCTION sync_platform_grant();

CREATE FUNCTION sync_workspace_grant() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE rid uuid;
BEGIN
    IF (TG_OP = 'INSERT') THEN
        SELECT id INTO rid FROM roles WHERE scope = 'workspace' AND name = NEW.role;
        INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
        VALUES (NEW.principal_type, NEW.principal_id, rid, 'workspace', NEW.workspace_id)
        ON CONFLICT DO NOTHING;
    ELSIF (TG_OP = 'DELETE') THEN
        SELECT id INTO rid FROM roles WHERE scope = 'workspace' AND name = OLD.role;
        DELETE FROM access_grants
        WHERE principal_type = OLD.principal_type AND principal_id = OLD.principal_id
          AND role_id = rid AND scope_type = 'workspace' AND scope_id = OLD.workspace_id;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_workspace_grant_trg
AFTER INSERT OR DELETE ON workspace_members
FOR EACH ROW EXECUTE FUNCTION sync_workspace_grant();
