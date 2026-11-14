-- 0227_ws_assignment_group_origin.sql
--
-- SP3c-3 Task 5. The implicit workspace-assignment grant is now AUTHORED as an
-- explicit READ grant whose principal is the managed `ws:<slug> members` group
-- (SP3c-3 Tasks 1-4), not the old `origin='workspace_assignment'` +
-- `kind='workspace_members'` grant. To keep bind/unbind idempotent and "one
-- grant per (catalog, workspace)" WITHOUT colliding with the human-authored
-- `explicit` grants on the same catalog, the new grant carries its own origin
-- value `workspace_assignment_group` and keeps `origin_workspace_id` set — so a
-- second partial unique index keyed on that origin plays exactly the role
-- `grants_one_per_assignment` played for the old implicit grant.
--
-- Task 4's reversible migration (`migrate_ws_assignments_in_tx`) rewrites every
-- pre-existing `workspace_assignment` grant into this SAME shape, so a migrated
-- grant and a freshly-authored one are byte-identical rows — the dedup, the
-- unbind DELETE and the idempotent re-bind all see them the same way.
--
-- The OLD `workspace_assignment` origin value, its coupling and its
-- `grants_one_per_assignment` index are KEPT: the Task-4 undo restores a grant
-- to `origin='workspace_assignment'`, so that origin must stay legal. Both the
-- origin-value CHECK and the origin/origin_workspace_id coupling CHECK are
-- therefore widened to admit the new value rather than replaced.
--
-- 0154 declared both CHECKs inline (unnamed), so Postgres auto-named them
-- (`grants_origin_check`, `grants_check2`, ... — the number depends on
-- declaration order and is not safe to hardcode). This drops them by MATCHING
-- their definition (every CHECK on `perm.grants` whose text mentions
-- `workspace_assignment` — exactly these two, and no other check on the table
-- does) and re-adds both with explicit, stable names.

DO $$
DECLARE
    c record;
BEGIN
    FOR c IN
        SELECT con.conname, pg_get_constraintdef(con.oid) AS def
          FROM pg_constraint con
         WHERE con.conrelid = 'perm.grants'::regclass
           AND con.contype = 'c'
    LOOP
        -- The origin-value list CHECK and the origin/origin_workspace_id
        -- coupling CHECK are the only two that name `workspace_assignment`.
        IF c.def ILIKE '%workspace_assignment%' THEN
            EXECUTE format('ALTER TABLE perm.grants DROP CONSTRAINT %I', c.conname);
        END IF;
    END LOOP;
END
$$;

ALTER TABLE perm.grants
    ADD CONSTRAINT grants_origin_values
    CHECK (origin IN ('explicit', 'workspace_assignment', 'workspace_assignment_group'));

-- An explicit grant names no workspace; both assignment shapes
-- (`workspace_assignment` and `workspace_assignment_group`) name theirs.
ALTER TABLE perm.grants
    ADD CONSTRAINT grants_assignment_names_workspace
    CHECK (
        (origin IN ('workspace_assignment', 'workspace_assignment_group'))
        = (origin_workspace_id IS NOT NULL)
    );

-- One group-assignment grant per (catalog, workspace): the assignment is the
-- identity, exactly as `grants_one_per_assignment` is for the old origin.
CREATE UNIQUE INDEX grants_one_per_assignment_group
    ON perm.grants (catalog_id, origin_workspace_id)
    WHERE origin = 'workspace_assignment_group';
