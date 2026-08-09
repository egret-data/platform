-- Certification for dashboards, mirroring what datasets and metrics already carry (0098).
--
-- WHY THE SAME SHAPE. `analytics.datasets` has carried `certified` / `certified_by` /
-- `certification_details` since 0098, and `DatasetRepo` reads and writes them. A dashboard that
-- expressed the same idea with different column names, or as a tag, would make "is this trusted"
-- two questions instead of one. This is deliberately the boring choice.
--
-- ON BACKFILL. These defaults DO write to every existing row, and that is correct here: an
-- existing dashboard is genuinely not certified, so `false` / `''` is the true value rather than
-- a stamp. Contrast 0106's `created_at`, which had to be added bare and defaulted separately
-- precisely because `now()` would have asserted something untrue about rows that predate it.
--
-- SCHEMA. The table is `analytics.dashboards` — 0067 moved it out of `public`.
ALTER TABLE analytics.dashboards
  ADD COLUMN certified             boolean NOT NULL DEFAULT false,
  ADD COLUMN certified_by          text    NOT NULL DEFAULT '',
  ADD COLUMN certification_details text    NOT NULL DEFAULT '';

-- WHO MAY CERTIFY, and why it is not `dashboard:manage`.
--
-- Certification is a TRUST signal. If everyone who can edit a dashboard can also mark it
-- trusted, the mark says only "its author liked it" and readers learn nothing. Metabase gates
-- its equivalent on admin; egret already has the right vocabulary for this without reaching for
-- admin — `data_steward` holds the platform governance perms (policy:* in 0059, tag:* in 0080),
-- and certifying content is the same kind of act.
--
-- NOTE an inconsistency this makes visible rather than creates: DATASET certification rides on
-- `dataset:manage` (routes.rs ds_update), so a dataset's editor can self-certify. That predates
-- this migration and is worth resolving, but silently copying it here would have spread it.
INSERT INTO role_permissions (role_id, permission)
SELECT r.id, 'dashboard:certify'
FROM roles r
WHERE r.scope = 'platform' AND r.name = 'data_steward'
ON CONFLICT (role_id, permission) DO NOTHING;
