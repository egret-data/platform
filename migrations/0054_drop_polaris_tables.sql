-- Embedded-catalog pivot: the Polaris integration is superseded by the embedded
-- lakekeeper + egret RBAC store. Drop the Polaris-specific tables.
DROP TABLE IF EXISTS workspace_catalog_assignments CASCADE;
DROP TABLE IF EXISTS polaris_connection CASCADE;
