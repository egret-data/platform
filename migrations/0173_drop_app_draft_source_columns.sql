-- Data Apps: `source` and `layout_json` moved to the shared object store
-- (`[storage]`, `apps.storage.prefix`) in Parts S1-S3 of
-- docs/en/plans/2026-10-05-data-apps-storage-s3.md. This is IRREVERSIBLE: any
-- row whose content was never backfilled (Part S2's `egret
-- backfill-apps-storage`, since removed by this same change) loses that
-- content for good. Confirmed before this migration was written to run:
-- every environment with real Data Apps content has had the backfill run
-- against it, and this migration ships in the same change that removes the
-- backfill tool itself -- there is no later chance to run it.
ALTER TABLE analytics.app_drafts
  DROP COLUMN source,
  DROP COLUMN layout_json;

ALTER TABLE analytics.app_versions
  DROP COLUMN source,
  DROP COLUMN layout_json;
