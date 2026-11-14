-- SP3d: the per-EP migration-complete verdict, recomputed on each compile.
-- Nullable: NULL means "not computed since this column was added".
ALTER TABLE perm.compile_runs ADD COLUMN migration_complete boolean;
COMMENT ON COLUMN perm.compile_runs.migration_complete IS
  'SP3d: true when the EP has no un-migrated operator policy, role/workspace principal, or gated clearance. Recomputed each compile.';
