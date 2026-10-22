# B2: Datasets and metrics integrity

Package B2 of the Known-issues fix programme. Rows: KI-096, KI-098, KI-099 (S2); KI-100, KI-101,
KI-102, KI-103, KI-105, KI-110, KI-111, KI-158, KI-166 (S3). Decision D8 (settled): deleting a
dataset that charts use is refused with the list of those charts; "delete with charts" exists
only as a typed confirmation; the same rule applies to a used metric.

## Backend (`egret-bi`)

- **In use is a 409 with the charts.** A new `BiError::InUse` answers
  `{error, code: "in_use", charts: [{id, name, viz_type, trashed}]}`.
- **KI-098.** `DELETE /datasets/{id}` refuses (409) while any chart, live or in the trash, is
  built on the dataset (they would be deleted with it by `ON DELETE CASCADE`). The request
  succeeds with `?confirm=<dataset name>`, which deletes the charts too. No charts: unchanged.
- **KI-099.** Renaming a metric rewrites, in the same transaction, every chart on the dataset
  that names it: `spec.metrics`, `spec.orderby[].field`, and every string value and object key
  equal to the old name under `settings` (and under a dashboard card's `options.vizOverride` for
  those charts). The rewrite is refused (409) when the old name is also a column name of the
  dataset, because a settings string could then mean either. Deleting a used metric is a 409
  unless `?confirm=<metric name>`.
- **KI-100.** A metric expression is probed twice: as before (type), then as
  `SELECT count(*), <expr> FROM <source>`. Only an aggregate (or a constant) survives the second
  probe; failing it is a 400 "metric expression must be an aggregate…".
- **KI-101.** `POST /datasets/{id}/columns` rejects an empty or whitespace-only name (400).
- **KI-102.** Refresh keeps `is_temporal` for a column that still exists with the same engine
  type (a changed type takes the engine's answer). The answer adds `added_columns` and
  `removed_columns`.
- **KI-110.** `POST /datasets/{id}/duplicate {name}` copies the dataset (either kind) with every
  column setting and metric (certification is not copied). It needs `dataset:read` on the source
  and `dataset:manage` with the database granted, as create does.
- **KI-111 / KI-166.** The usage answer carries `viz_type`; the dataset detail carries
  `deleted_at`.

## Frontend

- **KI-096.** Decision: the switches are enforced in the pickers, not in the backend (a backend
  refusal would break saved charts). Dimension off: the column is not offered by the chart
  builder's field pickers (raw-records table and object detail keep every column, as Superset
  does). Filter off: the column is not offered when mapping a dashboard filter to a card (the
  chart builder has no filter picker; filters come from drill and dashboards). A column the
  chart or the mapping already uses stays offered, so an existing chart keeps rendering and
  showing its pick.
- **KI-098 / KI-099 / KI-105.** Delete dialogs list the charts that use the dataset or metric
  and their count; with charts, the destructive path needs the typed name. Delete on a
  calculated column and on a metric asks first (shared `ConfirmDeleteDialog`).
- **KI-102.** The Refresh toast names the added and removed columns.
- **KI-103.** Display names become the default `column_title` at render time (never saved) in
  the chart page and in Preview, so table headers show them.
- **KI-110.** Duplicate is offered for both kinds and asks for the new name.
- **KI-111.** Trashing a dataset with charts asks first ("N charts use this dataset and will
  keep working until the dataset is deleted"); a chart whose dataset is in the trash shows a
  notice.
- **KI-158 / KI-166.** "1 column", "1 metric"; the Usage tab uses each chart type's own icon.
