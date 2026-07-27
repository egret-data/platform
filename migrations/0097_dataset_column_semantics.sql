-- Semantic-layer config for dataset columns and metrics (Superset/Metabase-style):
-- which columns are groupable dimensions / filterable, a display format, and
-- metric display format + certification.
ALTER TABLE analytics.dataset_columns
  ADD COLUMN groupby    boolean NOT NULL DEFAULT true,
  ADD COLUMN filterable boolean NOT NULL DEFAULT true,
  ADD COLUMN format     text    NOT NULL DEFAULT '';

ALTER TABLE analytics.dataset_metrics
  ADD COLUMN format    text    NOT NULL DEFAULT '',
  ADD COLUMN certified boolean NOT NULL DEFAULT false;
