-- 0144_engine_catalogs_drop_kind_check.sql
-- Catalogs are connector-first (v2): the stored `kind` IS the Trino connector
-- name (mysql, iceberg, clickhouse, ...), validated in `add_catalog`. The 0057
-- column CHECK still limited it to the v1 enum, so every newer connector
-- failed at INSERT with a 500. Postgres named the inline column constraint
-- `<table>_<column>_check`; 0062's SET SCHEMA kept the name.
ALTER TABLE engine_mgmt.engine_catalogs
    DROP CONSTRAINT IF EXISTS engine_catalogs_kind_check;
