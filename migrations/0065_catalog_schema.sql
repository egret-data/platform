-- E3c: the Catalog seam owns its tables. Move warehouses + catalog_roles +
-- catalog_grants + catalog_clients (embedded-catalog RBAC) and oidc_signing_keys
-- (the mini machine-IdP, part of the Catalog seam cluster) from `public` into
-- their own `catalog` schema (mirrors E1b's 0062 for engine_mgmt, E2c's 0063 for
-- policy, E3a's 0064 for storage). SET SCHEMA preserves data, indexes,
-- constraints, and FKs.
CREATE SCHEMA IF NOT EXISTS catalog;

ALTER TABLE warehouses        SET SCHEMA catalog;
ALTER TABLE catalog_roles     SET SCHEMA catalog;
ALTER TABLE catalog_grants    SET SCHEMA catalog;
ALTER TABLE catalog_clients   SET SCHEMA catalog;
ALTER TABLE oidc_signing_keys SET SCHEMA catalog;

-- Cross-seam FKs remain (Postgres allows cross-schema FKs within one database);
-- they are dropped only at a real service split.
--   catalog.warehouses.workspace_id      -> public.workspaces              -- SEAM-CROSSING (iam/workspace)
--   catalog.warehouses.storage_conn_id   -> storage.storage_connections    -- SEAM-CROSSING (storage seam)
--   catalog.catalog_clients.workspace_id -> public.workspaces              -- SEAM-CROSSING (iam/workspace)
-- In-seam FKs unchanged (moved with their tables):
--   catalog.catalog_roles.warehouse_id      -> catalog.warehouses
--   catalog.catalog_grants.catalog_role_id  -> catalog.catalog_roles
--   catalog.catalog_clients.warehouse_id    -> catalog.warehouses
--   catalog.catalog_clients.catalog_role_id -> catalog.catalog_roles
-- oidc_signing_keys has no FKs.
