-- SP3c D2: primary is a property of the RELATIONSHIP between an engine and a
-- catalog, not of the engine. `engines.warehouse_id` forced one warehouse per
-- engine; this column is what replaces it, and SP3c-2 drops the column once
-- nothing reads it.
ALTER TABLE registry.engine_catalog_attachments
    ADD COLUMN is_primary boolean NOT NULL DEFAULT false;

-- Backfill from the predicate the code used until now
-- (`catalog.warehouse_id == Some(engine.warehouse_id)`, catalog_apply.rs:844),
-- so every existing attachment keeps the primacy it already had and
-- `list_engine_attachments` returns rows in exactly the order it did before.
UPDATE registry.engine_catalog_attachments a
   SET is_primary = true
  FROM registry.catalogs c, engine_mgmt.engines e
 WHERE c.id = a.catalog_id
   AND e.id = a.engine_id
   AND c.warehouse_id = e.warehouse_id;

-- AT MOST one primary per engine. "Exactly one" is a cross-row invariant with
-- no anchor row to hang a CHECK on, so the code owns it and a route test proves
-- it; this index is what makes the failure a constraint violation rather than
-- two primaries nobody notices.
CREATE UNIQUE INDEX engine_catalog_attachments_primary_key
    ON registry.engine_catalog_attachments (engine_id)
    WHERE is_primary;
