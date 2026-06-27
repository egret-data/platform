-- Drop legacy egret-native catalog metadata tables.
-- Lakekeeper is now the catalog engine; its data lives in the `lakekeeper` Postgres schema.
-- Drop children before parents (CASCADE also handles any FK stragglers).
--
-- KEEP: catalog_tokens  (engine front-auth + mint CLI)
-- KEEP: workspace_warehouse  (engine mapping, migration 0025)
-- KEEP: role_permissions / roles  (RBAC, migration 0024 added catalog:read/catalog:write)

DROP TABLE IF EXISTS catalog_table_partition_statistics CASCADE;
DROP TABLE IF EXISTS catalog_table_statistics CASCADE;
DROP TABLE IF EXISTS catalog_table_refs CASCADE;
DROP TABLE IF EXISTS catalog_table_metadata_log CASCADE;
DROP TABLE IF EXISTS catalog_table_snapshot_log CASCADE;
DROP TABLE IF EXISTS catalog_table_snapshots CASCADE;
DROP TABLE IF EXISTS catalog_table_properties CASCADE;
DROP TABLE IF EXISTS catalog_table_sort_orders CASCADE;
DROP TABLE IF EXISTS catalog_table_partition_specs CASCADE;
DROP TABLE IF EXISTS catalog_table_schemas CASCADE;
DROP TABLE IF EXISTS catalog_tables CASCADE;
DROP TABLE IF EXISTS catalog_view_properties CASCADE;
DROP TABLE IF EXISTS catalog_view_version_log CASCADE;
DROP TABLE IF EXISTS catalog_view_representations CASCADE;
DROP TABLE IF EXISTS catalog_view_versions CASCADE;
DROP TABLE IF EXISTS catalog_view_schemas CASCADE;
DROP TABLE IF EXISTS catalog_views CASCADE;
DROP TABLE IF EXISTS catalog_namespaces CASCADE;
DROP TABLE IF EXISTS catalogs CASCADE;
-- KEEP: catalog_tokens, workspace_warehouse, role_permissions, roles
