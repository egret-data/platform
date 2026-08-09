-- Dashboard attribution: WHEN a dashboard was created, and WHO last edited it.
--
-- THE GAP. analytics.dashboards has carried `created_by` and `updated_at` since 0015 and has not
-- gained an attribution column since (0016 parameters, 0017 tabs, 0018 width, 0019 soft-delete,
-- 0021 folder_id, 0105 width default). Metabase's info sidebar states four things — created when,
-- created by whom, last edited when, last edited by whom — and two of the four do not exist in
-- this schema at all. The sidebar renders what there is: the raw `created_by` UUID under "Owner",
-- and a bare `updated_at` under "Last edited".
--
-- WHY created_at IS NULLABLE AND IS NOT BACKFILLED. The obvious shape is
-- `timestamptz NOT NULL DEFAULT now()`, which is what 0021_folders.sql used — on a NEW table.
-- On an EXISTING table it is a different statement: since Postgres 11, ADD COLUMN with a DEFAULT
-- materialises that default for every pre-existing row, so every dashboard in the deployment
-- would report that it was created the moment this migration ran. That is a wrong date which is
-- indistinguishable from a right one, and no later repair can recover the true value because the
-- true value was never recorded. Backfilling from `updated_at` is the same lie with extra steps:
-- `updated_at` is rewritten by every save, so it is the creation date of nothing. NULL is the
-- honest encoding of "never recorded", and the sidebar renders the "Added" line only when the
-- value is present. New rows pick up the DEFAULT attached below, so the gap closes going forward.
--
-- updated_by is ON DELETE SET NULL, matching the treatment 0093 gave every other attribution FK:
-- hard-deleting a user preserves the content and drops only the name.
ALTER TABLE analytics.dashboards
  ADD COLUMN created_at timestamptz,
  ADD COLUMN updated_by uuid REFERENCES users(id) ON DELETE SET NULL;

-- Attached AFTER the column exists, deliberately. Folding it into the ADD COLUMN above is what
-- would backfill every existing row (see the header); this form applies to future INSERTs only.
ALTER TABLE analytics.dashboards ALTER COLUMN created_at SET DEFAULT now();
