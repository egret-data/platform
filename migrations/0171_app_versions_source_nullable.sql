-- Data Apps S3 cutover, Part S3/Task 9: `AppStore` stops writing
-- `source`/`layout_json` on `analytics.app_versions` (object storage is now
-- the only writer of version content; Part S2's backfill guarantees every
-- version already has its content in the object store too). `layout_json` is
-- already nullable; `source` is `NOT NULL` from its original migration
-- (0163), so inserting a version row without it would otherwise fail.
--
-- This does not drop the columns -- that is Part S4's own migration, kept
-- separate and irreversible on purpose. This just stops requiring a value
-- clients no longer provide, so the stale columns read back NULL for every
-- version from here on instead of being fabricated.
ALTER TABLE analytics.app_versions
  ALTER COLUMN source DROP NOT NULL;
