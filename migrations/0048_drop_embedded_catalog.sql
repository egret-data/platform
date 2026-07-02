-- Control-plane Polaris pivot: remove the embedded-lakekeeper catalog storage.
-- Greenfield — no migration of catalog data.
DROP TABLE IF EXISTS catalog_tokens CASCADE;
DROP TABLE IF EXISTS workspace_warehouses CASCADE;
DROP SCHEMA IF EXISTS lakekeeper CASCADE;
