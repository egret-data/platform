-- Z2: Data API endpoint alerts. Widens `automation.schedules` with a third
-- `type`, `dataapi_alert`, that watches `dataapi.access_log` instead of
-- running SQL. See spec 2026-08-22-dataapi-alerts-design.md §5.

-- Widen the type vocabulary. This is the only existing CHECK that changes:
-- `schedule_alert_needs_query` and `schedule_report_has_format` are both
-- written `type <> 'x' OR (...)`, so a new type excludes itself from both and
-- neither needs to be touched.
ALTER TABLE automation.schedules DROP CONSTRAINT schedule_type_valid;
ALTER TABLE automation.schedules ADD CONSTRAINT schedule_type_valid
  CHECK (type IN ('alert', 'report', 'dataapi_alert'));

ALTER TABLE automation.schedules
  ADD COLUMN dataapi_endpoint_id uuid REFERENCES dataapi.endpoints(id) ON DELETE CASCADE,
  ADD COLUMN dataapi_metric      text,
  ADD COLUMN dataapi_error_code  text,
  ADD COLUMN dataapi_window_s    integer;

ALTER TABLE automation.schedules ADD CONSTRAINT schedule_dataapi_metric_valid
  CHECK (dataapi_metric IS NULL OR dataapi_metric IN
    ('error_rate', 'error_count', 'p50_duration_ms', 'p95_duration_ms',
     'call_count', 'error_code_count'));

-- error_code if and only if the metric is error_code_count. Two
-- one-directional clauses rather than one equality, so NULL semantics stay
-- obvious.
ALTER TABLE automation.schedules ADD CONSTRAINT schedule_dataapi_error_code_needs_metric
  CHECK (dataapi_error_code IS NULL OR dataapi_metric = 'error_code_count');
ALTER TABLE automation.schedules ADD CONSTRAINT schedule_dataapi_metric_needs_error_code
  CHECK (dataapi_metric <> 'error_code_count' OR dataapi_error_code IS NOT NULL);

-- The shape of a dataapi_alert. The load-bearing half is
-- `database_id IS NULL AND sql IS NULL`: it makes "a row both evaluators
-- claim as theirs" impossible to write.
--
-- `dataapi_window_s <= 2592000` (30 days, the widest of the four presets
-- `ActivityRange` offers): review fix-wave finding 2. `> 0` alone left this
-- unbounded, and unlike the Activity page's own range picker — which
-- DISABLES a preset that reaches past `access_log` retention (spec §4.7's
-- `retention_days`, `activity.rs`) rather than let someone watch the data
-- thin out at the old end and read it as a real drop in traffic — a
-- dataapi_alert is never rendered for a human to notice that. It is a
-- threshold decision made unattended every tick. The bound here is a fixed
-- ceiling, not the live retention setting: `egret-automation` does not
-- depend on `egret-dataapi`'s runtime config (see this file's own dispatch
-- doc comment in `runner.rs`), so a CHECK cannot see it either — Postgres
-- CHECK constraints cannot query another table. `ScheduleEditor.tsx`'s help
-- text says plainly that the window may still be truncated by retention.
ALTER TABLE automation.schedules ADD CONSTRAINT schedule_dataapi_needs_source
  CHECK (type <> 'dataapi_alert' OR (
    dataapi_metric IS NOT NULL
    AND dataapi_window_s IS NOT NULL
    AND dataapi_window_s > 0 AND dataapi_window_s <= 2592000
    AND validator_type = 'operator'
    AND database_id IS NULL AND sql IS NULL
    AND chart_id IS NULL AND dashboard_id IS NULL));

-- And the converse: an engine alert or a report may not carry the Data API
-- columns.
ALTER TABLE automation.schedules ADD CONSTRAINT schedule_alert_no_dataapi_columns
  CHECK (type = 'dataapi_alert' OR (
    dataapi_metric IS NULL AND dataapi_endpoint_id IS NULL
    AND dataapi_window_s IS NULL AND dataapi_error_code IS NULL));

-- The evaluator's read: one workspace, one (or every) endpoint, a time
-- window, newest first.
CREATE INDEX access_log_ws_ep_ts_idx
  ON dataapi.access_log (workspace_id, endpoint_id, ts DESC);
