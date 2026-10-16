-- View governance B3 (Task 9): keep a view's column tags in step with its base
-- tables' classifications.
--
-- 1. Provenance. A view column's tag row is written by egret from the view's
--    lineage (the commit hold, `egret_catalog_engine::view_hold`, and the B3
--    retag, `egret_perm_compile::view_tags`) or by an admin by hand
--    (`egret_bi::CatalogTagRepo::assign_catalog`). The two share the primary
--    key, so nothing told them apart, and a retag that removes the tags a
--    lineage no longer derives would have removed an admin's tag as well.
--    `from_lineage` marks the rows the lineage derived: only those are ever
--    removed by a retag, a view drop or the sweep. An admin's assignment of a
--    coordinate that is already derived turns it into a manual one, so the
--    admin's intent survives the next retag. Existing rows are manual.
ALTER TABLE catalog_tag_assignments
    ADD COLUMN from_lineage boolean NOT NULL DEFAULT false;

-- 2. The view's columns, beside its lineage. The lineage names base tables
--    only, and a classification change re-tags EVERY column of each dependent
--    view (spec R3), in the transaction of the base write, where no catalog
--    metadata can be loaded. The hold and the sweep know the view's columns
--    whenever they record a lineage, so they record them here. NULL means
--    "not known", never "no column": a view always has at least one column.
ALTER TABLE view_lineage_state
    ADD COLUMN columns text[],
    ADD CONSTRAINT view_lineage_state_columns_not_empty
        CHECK (columns IS NULL OR cardinality(columns) > 0);
