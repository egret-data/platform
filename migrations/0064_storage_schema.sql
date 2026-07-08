-- E3a: the Catalog seam sheds the (unrelated) global storage-connection registry.
-- Move storage_connections from `public` into its own `storage` schema, owned by
-- egret-storage (mirrors E1b's 0062 for engine_mgmt and E2c's 0063 for policy).
-- SET SCHEMA preserves data, indexes, constraints, and FKs.
CREATE SCHEMA IF NOT EXISTS storage;

ALTER TABLE storage_connections SET SCHEMA storage;

-- Cross-seam FKs remain (Postgres allows cross-schema FKs within one database);
-- they are dropped only at a real service split.
--   warehouses.storage_conn_id                -> storage.storage_connections  -- SEAM-CROSSING (catalog seam)
--   storage.storage_connections.secret_ref    -> workspace_secrets            -- SEAM-CROSSING (secrets seam)
