-- E1b: the engine-provisioning seam owns its tables. Move engines +
-- engine_catalogs from `public` into their own `engine_mgmt` schema.
-- SET SCHEMA preserves data, indexes, constraints, and FKs.
CREATE SCHEMA IF NOT EXISTS engine_mgmt;

ALTER TABLE engines SET SCHEMA engine_mgmt;
ALTER TABLE engine_catalogs SET SCHEMA engine_mgmt;

-- Cross-seam FKs remain (Postgres allows cross-schema FKs within one database);
-- they are dropped only at a real service split.
--   engine_mgmt.engines.warehouse_id      -> public.warehouses      -- SEAM-CROSSING (catalog seam)
--   engine_mgmt.engines.authz_service_id  -> public.authz_services  -- SEAM-CROSSING (policy seam)
-- In-seam FK unchanged:
--   engine_mgmt.engine_catalogs.engine_id -> engine_mgmt.engines
