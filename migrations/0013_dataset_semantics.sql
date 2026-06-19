ALTER TABLE dataset_columns
  ADD COLUMN expression   text,
  ADD COLUMN verbose_name text NOT NULL DEFAULT '',
  ADD COLUMN description  text NOT NULL DEFAULT '';

CREATE TABLE dataset_metrics (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dataset_id   uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    metric_name  text NOT NULL,
    expression   text NOT NULL,
    data_type    text NOT NULL,
    type_generic text NOT NULL,
    verbose_name text NOT NULL DEFAULT '',
    description  text NOT NULL DEFAULT '',
    ordinal      int  NOT NULL,
    UNIQUE (dataset_id, metric_name)
);
CREATE INDEX dataset_metrics_dataset_idx ON dataset_metrics (dataset_id);
