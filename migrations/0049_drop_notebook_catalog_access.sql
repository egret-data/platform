-- Notebook catalog-access (warehouse-based) is removed with the embedded catalog.
-- The notebook now uses query engines; engine-access (notebook_engine_access) stays.
DROP TABLE IF EXISTS notebook_catalog_access CASCADE;
