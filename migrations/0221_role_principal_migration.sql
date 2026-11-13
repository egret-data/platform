-- 0221_role_principal_migration.sql
-- Reversible record of the role-principal -> `role:<name> holders` group rewrite
-- (SP3c-1 Task 4). Separate from perm.migrated_from (that table is policy_id-keyed
-- and records the hand-written-policy migration, which excludes clearance/roles).
-- One row per migration run; the undo re-hashes each rewritten row against
-- `row_hashes` under `FOR UPDATE` on this row and refuses if any diverges.
CREATE TABLE perm.role_principal_migration (
    id           bigserial PRIMARY KEY,
    roles        text[]      NOT NULL,          -- roles that were rewritten
    rewrites     jsonb       NOT NULL,          -- [{table, id, mode, role, group_name, prev_kind, prev_ref}]
    row_hashes   text[]      NOT NULL,          -- canonical hash per rewritten row, for the undo guard
    migrated_by  text        NOT NULL,
    migrated_at  timestamptz NOT NULL DEFAULT now()
);
