-- A report may now deliver several objects in one message.
--
-- `chart_id`/`dashboard_id` held exactly one target, pinned by
-- `schedule_report_needs_target`. That is why a weekly pack covering three
-- dashboards had to be three schedules and three separate emails. Superset
-- solves the same problem with `ALERT_REPORT_TABS` — several dashboard tabs,
-- one multi-page PDF.
--
-- The columns are KEPT rather than dropped: they are the alert's optional
-- attachment (0114-era behaviour) and the single-target fast path, and every
-- existing row already reads correctly through them. What changes is that a
-- report with rows in `schedule_targets` uses those instead.

CREATE TABLE automation.schedule_targets (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  schedule_id uuid NOT NULL REFERENCES automation.schedules(id) ON DELETE CASCADE,
  kind        text NOT NULL,
  object_id   uuid NOT NULL,
  -- Delivery order, and therefore page order in the PDF. A report that
  -- reordered itself between runs would make two copies impossible to compare.
  ordinal     integer NOT NULL DEFAULT 0,

  CONSTRAINT schedule_target_kind_valid CHECK (kind IN ('chart', 'dashboard')),
  CONSTRAINT schedule_target_unique UNIQUE (schedule_id, kind, object_id)
);

CREATE INDEX schedule_targets_schedule_idx
  ON automation.schedule_targets (schedule_id, ordinal);

-- Every existing report becomes a one-target report, so the new read path is
-- correct for old rows from the first tick after deploy — no dual-source
-- window where some reports read the column and others the table.
INSERT INTO automation.schedule_targets (schedule_id, kind, object_id, ordinal)
SELECT id, 'chart', chart_id, 0 FROM automation.schedules
 WHERE type = 'report' AND chart_id IS NOT NULL
UNION ALL
SELECT id, 'dashboard', dashboard_id, 0 FROM automation.schedules
 WHERE type = 'report' AND dashboard_id IS NOT NULL;

-- The single-target CHECKs have to go: they are what made a list impossible.
-- The shape is now enforced in Rust (`to_new`), which can say WHICH field is
-- wrong — and, unlike a CHECK, can count rows in another table.
ALTER TABLE automation.schedules
  DROP CONSTRAINT IF EXISTS schedule_report_needs_target,
  DROP CONSTRAINT IF EXISTS schedule_report_format_matches_target;

-- What survives from 0112: a dashboard still cannot be sent as csv/text,
-- because a dashboard has no single table. Now expressed against the target
-- table so it covers every target, not only the one in the column.
ALTER TABLE automation.schedules
  ADD CONSTRAINT schedule_report_has_format CHECK (
    type <> 'report' OR report_format IS NOT NULL
  );
