-- 0151_drop_legacy_engine_catalogs.sql
-- SP3c-2 §4 step 1. The registry (0147) replaced `engine_mgmt.engine_catalogs`,
-- and 0147's boot backfill — deleted in this release — was the only thing that
-- moved rows across. So this migration REFUSES rather than drops data it cannot
-- carry forward: an operator on a pre-SP3a database upgrades through SP3c-1 and
-- boots once, which runs the backfill, and then comes here.
DO $$
DECLARE
    unmapped bigint;
BEGIN
    SELECT count(*) INTO unmapped
      FROM engine_mgmt.engine_catalogs c
      LEFT JOIN registry.legacy_engine_catalogs m ON m.engine_catalog_id = c.id
     WHERE m.engine_catalog_id IS NULL;

    IF unmapped > 0 THEN
        RAISE EXCEPTION
            'engine_mgmt.engine_catalogs still holds % row(s) the catalog registry has not migrated. Upgrade to the SP3c-1 release first and start egret once so the boot backfill runs, then apply this release.',
            unmapped;
    END IF;
END $$;

-- `migration_report` recorded what the backfill did. Nothing writes it now: its
-- other writer, `ensure_warehouse_catalog`, took a `report_for` closure and
-- every production caller passed one that returned an empty Vec (SP3c-2 Task 2
-- deleted the parameter and the facility with it).
DROP TABLE registry.migration_report;
DROP TABLE registry.legacy_engine_catalogs;
DROP TABLE engine_mgmt.engine_catalogs;
