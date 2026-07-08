-- E2c: the Policy/Ranger seam owns its tables. Move the authz_* tables from
-- `public` into their own `policy` schema (mirrors E1b's 0062 for engine_mgmt).
-- SET SCHEMA preserves data, indexes, constraints, and FKs.
CREATE SCHEMA IF NOT EXISTS policy;

ALTER TABLE authz_service_defs SET SCHEMA policy;
ALTER TABLE authz_services     SET SCHEMA policy;
ALTER TABLE authz_policies     SET SCHEMA policy;

-- policy_audit (migration 0007) was never read or written by any code — drop the
-- dead table. Effort B introduces a real audit_log in a dedicated `audit` schema.
DROP TABLE IF EXISTS policy_audit;

-- Cross-seam FK remains (Postgres allows cross-schema FKs within one database);
-- SET SCHEMA moved the referenced table, so the constraint now points into policy.
-- Dropped only at a real service split.
--   engine_mgmt.engines.authz_service_id -> policy.authz_services  -- SEAM-CROSSING (engine-mgmt seam)
-- In-seam FKs unchanged (moved with their tables):
--   policy.authz_services.type       -> policy.authz_service_defs(name)
--   policy.authz_policies.service_id -> policy.authz_services(id)
