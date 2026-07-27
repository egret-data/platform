-- Richer semantic layer (Metabase/Superset parity):
--  - dataset_columns.semantic_type: a semantic role (category/country/currency/
--    email/url/image/…) that drives smart rendering & defaults. '' = none.
--  - certification metadata: who certified and why, on metrics and on the
--    dataset itself (Superset certifies datasets + metrics + columns).
ALTER TABLE analytics.dataset_columns
  ADD COLUMN semantic_type text NOT NULL DEFAULT '';

ALTER TABLE analytics.dataset_metrics
  ADD COLUMN certified_by          text NOT NULL DEFAULT '',
  ADD COLUMN certification_details text NOT NULL DEFAULT '';

ALTER TABLE analytics.datasets
  ADD COLUMN certified             boolean NOT NULL DEFAULT false,
  ADD COLUMN certified_by          text    NOT NULL DEFAULT '',
  ADD COLUMN certification_details text    NOT NULL DEFAULT '';
