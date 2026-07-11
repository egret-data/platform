-- B2-3: role hierarchy. A role may inherit a same-scope parent's permissions.
ALTER TABLE roles ADD COLUMN parent_role_id uuid REFERENCES roles(id) ON DELETE SET NULL;

-- Same-scope + acyclic cannot be a CHECK (it must read other rows). A trigger
-- enforces both as defense-in-depth behind the app-layer validation in RoleRepo.
CREATE OR REPLACE FUNCTION roles_parent_guard() RETURNS trigger AS $$
DECLARE
    cur uuid;
    parent_scope text;
    hops int := 0;
BEGIN
    IF NEW.parent_role_id IS NULL THEN
        RETURN NEW;
    END IF;
    -- Same scope.
    SELECT scope INTO parent_scope FROM roles WHERE id = NEW.parent_role_id;
    IF parent_scope IS DISTINCT FROM NEW.scope THEN
        RAISE EXCEPTION 'role parent must share scope (role %, parent scope %)', NEW.scope, parent_scope;
    END IF;
    -- Acyclic: walk up from the parent; if we reach NEW.id, it is a cycle.
    cur := NEW.parent_role_id;
    WHILE cur IS NOT NULL LOOP
        IF cur = NEW.id THEN
            RAISE EXCEPTION 'role parent cycle detected on role %', NEW.id;
        END IF;
        hops := hops + 1;
        IF hops > 64 THEN
            RAISE EXCEPTION 'role parent chain too deep on role %', NEW.id;
        END IF;
        SELECT parent_role_id INTO cur FROM roles WHERE id = cur;
    END LOOP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER roles_parent_guard_trg
    BEFORE INSERT OR UPDATE OF parent_role_id ON roles
    FOR EACH ROW EXECUTE FUNCTION roles_parent_guard();
