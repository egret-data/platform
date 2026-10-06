-- Data Apps S3 cutover, Part S3/Task 10: `AppStore` stops storing draft
-- content in `analytics.app_drafts` (object storage is now the only home of
-- draft content, keyed by revision so a reader only ever sees the committed
-- revision's object). `layout_json` is already nullable (0170); `source` is
-- `NOT NULL` from its original migration (0163), so a draft row without it
-- would otherwise be refused.
--
-- From here on `create` inserts `source = NULL` and every `save_draft` sets
-- `source`/`layout_json` back to NULL, so a non-NULL `source` means exactly
-- one thing: the row's CURRENT revision was written by pre-Task-10 code and
-- the column is its committed content. `egret backfill-apps-storage` relies
-- on that to move such rows to the revision-keyed object, and skips NULL ones.
--
-- This does not drop the columns -- that is Part S4's own migration, kept
-- separate and irreversible on purpose.
ALTER TABLE analytics.app_drafts
  ALTER COLUMN source DROP NOT NULL;
