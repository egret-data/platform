-- Alerts & Reports (P1): the data model, with nothing that runs it yet.
--
-- One table holds both types, as in Superset: alerts and reports share all of
-- their scheduling and ~80% of their columns, and splitting them would mean two
-- of every scheduler, claim query and run log.
CREATE SCHEMA IF NOT EXISTS automation;

CREATE TABLE automation.schedules (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  type          text NOT NULL,
  name          text NOT NULL,
  description   text NOT NULL DEFAULT '',
  active        boolean NOT NULL DEFAULT true,

  -- When. `crontab` is stored as written so the UI can show the user's own
  -- expression back to them; `next_run_at` is the scheduler's claim key (§5.1)
  -- and is derived from crontab + timezone on every write and after every run.
  crontab       text NOT NULL,
  timezone      text NOT NULL DEFAULT 'UTC',
  next_run_at   timestamptz,

  -- Alert only.
  database_id      uuid REFERENCES query.databases(id) ON DELETE SET NULL,
  sql              text,
  validator_type   text,
  validator_config jsonb NOT NULL DEFAULT '{}'::jsonb,
  last_eval_at     timestamptz,
  last_state       text,
  last_value       double precision,
  last_value_row   jsonb,

  -- Report only.
  chart_id      uuid REFERENCES analytics.charts(id)     ON DELETE CASCADE,
  dashboard_id  uuid REFERENCES analytics.dashboards(id) ON DELETE CASCADE,
  report_format text,
  custom_width  integer,
  custom_height integer,

  -- Shared knobs. The defaults are Superset's, because they are the numbers
  -- users coming from Superset expect to find already set.
  grace_period_s     integer NOT NULL DEFAULT 14400,
  working_timeout_s  integer NOT NULL DEFAULT 3600,
  log_retention_days integer NOT NULL DEFAULT 90,
  email_subject      text,

  created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  updated_by  uuid REFERENCES users(id) ON DELETE SET NULL,
  updated_at  timestamptz NOT NULL DEFAULT now(),

  CONSTRAINT schedule_name_uniq UNIQUE (workspace_id, type, name),

  -- The vocabulary is closed at the database, not only in Rust. A row whose
  -- `type` or `last_state` is a typo is not a slightly-wrong row: it is a row
  -- the scheduler's WHERE clause will never match again, failing silently and
  -- forever. Same reasoning for every enum below.
  CONSTRAINT schedule_type_valid CHECK (type IN ('alert', 'report')),
  CONSTRAINT schedule_validator_valid
    CHECK (validator_type IS NULL OR validator_type IN ('not_null', 'operator')),
  CONSTRAINT schedule_format_valid
    CHECK (report_format IS NULL OR report_format IN ('png', 'pdf', 'csv', 'text')),
  CONSTRAINT schedule_last_state_valid
    CHECK (last_state IS NULL
           OR last_state IN ('success', 'working', 'error', 'noop', 'grace')),

  -- The shape rules from §4, enforced where they cannot be forgotten. An alert
  -- with no SQL and a report with no target are both rows that can only ever
  -- error at run time; rejecting them at write time is what makes the run log
  -- mean something.
  CONSTRAINT schedule_alert_needs_query CHECK (
    type <> 'alert'
    OR (database_id IS NOT NULL AND sql IS NOT NULL AND validator_type IS NOT NULL)
  ),
  CONSTRAINT schedule_report_needs_target CHECK (
    type <> 'report'
    -- Exactly one target, never both, never neither.
    OR (report_format IS NOT NULL AND (chart_id IS NOT NULL) <> (dashboard_id IS NOT NULL))
  ),
  CONSTRAINT schedule_positive_timers CHECK (
    grace_period_s >= 0 AND working_timeout_s > 0 AND log_retention_days > 0
  )
);

-- The scheduler's claim query is `WHERE active AND next_run_at <= now()`, so
-- this index is the one that keeps a tick cheap as schedules accumulate.
CREATE INDEX schedules_due_idx
  ON automation.schedules (next_run_at)
  WHERE active;
CREATE INDEX schedules_ws_idx ON automation.schedules (workspace_id, type, name);

-- Owners. Plural from the start: a schedule whose only owner is deactivated
-- stops running (§5.2), and a shared on-call alert must survive one person
-- leaving. The query runs as an owner, so this table is a privilege grant —
-- hence the CASCADE on the user, not SET NULL.
CREATE TABLE automation.schedule_owners (
  schedule_id uuid NOT NULL REFERENCES automation.schedules(id) ON DELETE CASCADE,
  user_id     uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY (schedule_id, user_id)
);

CREATE TABLE automation.schedule_recipients (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  schedule_id uuid NOT NULL REFERENCES automation.schedules(id) ON DELETE CASCADE,
  kind        text NOT NULL,
  -- Shape depends on `kind`: {"addresses": [...]} for email,
  -- {"url": "...", "secret_id": "..."} for webhook. Kept as JSONB because P3
  -- and P5 add fields to it; the shape is validated in Rust on the way in.
  config      jsonb NOT NULL,
  CONSTRAINT recipient_kind_valid CHECK (kind IN ('email', 'webhook'))
);
CREATE INDEX schedule_recipients_schedule_idx
  ON automation.schedule_recipients (schedule_id);

CREATE TABLE automation.schedule_runs (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  schedule_id  uuid NOT NULL REFERENCES automation.schedules(id) ON DELETE CASCADE,
  state        text NOT NULL,
  started_at   timestamptz NOT NULL DEFAULT now(),
  ended_at     timestamptz,
  value        double precision,
  error        text,
  notification_sent_at timestamptz,
  CONSTRAINT run_state_valid
    CHECK (state IN ('success', 'working', 'error', 'noop', 'grace'))
);
-- The run log is always read newest-first for one schedule (the drawer in §7).
CREATE INDEX schedule_runs_schedule_idx
  ON automation.schedule_runs (schedule_id, started_at DESC);

-- Managing schedules is an editor act, following chart:manage (0014). Reading
-- them is implied by workspace membership plus the `alerts` module entitlement.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'alert:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
