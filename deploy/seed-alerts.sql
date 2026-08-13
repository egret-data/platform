-- Seed Alerts & Reports demo data for a FRONTEND check.
--
-- Gives one workspace a spread of schedules wide enough to exercise every branch
-- the Alerts UI can render: both types, every run state including the ones a
-- happy-path run never produces (`working`, `noop`, `grace`), paused rows, rows
-- that have never run, all four report formats, and both destination kinds —
-- plus a run history so the run-log drawer has something to page through.
--
-- Idempotent: every row it owns has an id derived from the workspace id, so a
-- re-run replaces its own rows and touches nothing else. Run it as often as you
-- like; it never accumulates.
--
-- Usage:  just seed-alerts            (workspace `demo`)
--         just seed-alerts eng        (any slug)
--         psql "$DATABASE_URL" -v ws=demo -f deploy/seed-alerts.sql

\set ON_ERROR_STOP on
\if :{?ws}
\else
  \set ws demo
\endif
-- Where the seeded SMTP row points. `localhost` suits the docker-compose
-- stack; a cluster needs the service name, because localhost there is the
-- egret pod itself.
\if :{?smtp_host}
\else
  \set smtp_host localhost
\endif
SELECT set_config('seed.ws', :'ws', false);
SELECT set_config('seed.smtp_host', :'smtp_host', false);

DO $seed$
DECLARE
  ws_slug  text := current_setting('seed.ws');
  smtp_host text := current_setting('seed.smtp_host');
  ws       uuid;
  owner_id uuid;
  db       uuid;
  ds       uuid;
  ch       uuid;
  dash     uuid;
  -- Every id this script owns is derived from the workspace, so two workspaces
  -- can both be seeded and a re-run overwrites only its own rows.
  made     text := '';
BEGIN
  SELECT id INTO ws FROM workspaces WHERE slug = ws_slug;
  IF ws IS NULL THEN
    RAISE EXCEPTION 'workspace "%" not found — pass an existing slug (just seed-alerts <slug>)', ws_slug;
  END IF;

  -- Owner of every seeded schedule. The alert query runs AS AN OWNER, so this
  -- has to be a real, active user: prefer someone already granted on this
  -- workspace, else the break-glass admin.
  SELECT u.id INTO owner_id
    FROM access_grants g JOIN users u ON u.id = g.principal_id
   WHERE g.scope_type = 'workspace' AND g.scope_id = ws
     AND g.principal_type = 'user' AND u.status = 'active'
   LIMIT 1;
  IF owner_id IS NULL THEN
    SELECT id INTO owner_id FROM users
     WHERE email = 'admin@egret.local' AND status = 'active';
  END IF;
  IF owner_id IS NULL THEN
    RAISE EXCEPTION 'no active user to own the seeded schedules';
  END IF;

  -- The Alerts module is a GATED workspace entitlement. Without this row the
  -- nav item does not render at all and the whole seed is invisible.
  INSERT INTO workspace_features (workspace_id, feature, enabled, updated_by)
  VALUES (ws, 'alerts', true, owner_id)
  ON CONFLICT (workspace_id, feature) DO UPDATE SET enabled = true;

  ------------------------------------------------------------------ targets --
  -- An alert needs a database and a report needs a chart or a dashboard, both
  -- as NOT NULL foreign keys. Reuse whatever the workspace already has so the
  -- seed points at real, runnable objects; only fabricate placeholders when the
  -- workspace is empty. A placeholder is enough to drive the UI and will fail
  -- honestly at run time, which is the correct outcome for a fake connection.
  SELECT id INTO db FROM query.databases WHERE workspace_id = ws
   ORDER BY created_at DESC LIMIT 1;
  IF db IS NULL THEN
    db := md5(ws::text || ':seed-alerts:database')::uuid;
    INSERT INTO query.databases (id, workspace_id, name, engine, host, port, status, created_by)
    VALUES (db, ws, 'Seed Trino (placeholder)', 'trino', 'localhost', 8081, 'unknown', owner_id)
    ON CONFLICT (id) DO NOTHING;
    made := made || ' database';
  END IF;

  SELECT id INTO ds FROM analytics.datasets
   WHERE workspace_id = ws AND deleted_at IS NULL ORDER BY updated_at DESC LIMIT 1;
  IF ds IS NULL THEN
    ds := md5(ws::text || ':seed-alerts:dataset')::uuid;
    INSERT INTO analytics.datasets (id, workspace_id, database_id, name, description, sql, created_by)
    VALUES (ds, ws, db, 'Seed orders', 'Placeholder dataset created by seed-alerts.sql',
            'SELECT 1 AS n, ''a'' AS label', owner_id)
    ON CONFLICT (id) DO NOTHING;
    made := made || ' dataset';
  END IF;

  SELECT id INTO ch FROM analytics.charts
   WHERE workspace_id = ws AND deleted_at IS NULL ORDER BY updated_at DESC LIMIT 1;
  IF ch IS NULL THEN
    ch := md5(ws::text || ':seed-alerts:chart')::uuid;
    -- params v2: the query at `spec`, a flat namespaced `settings` map.
    INSERT INTO analytics.charts (id, workspace_id, dataset_id, name, description, viz_type, params, created_by)
    VALUES (ch, ws, ds, 'Seed orders by label', 'Placeholder chart created by seed-alerts.sql', 'bar',
            jsonb_build_object(
              'version', 2,
              'spec', jsonb_build_object(
                'metrics', jsonb_build_array('n'), 'dimensions', jsonb_build_array('label'),
                'columns', '[]'::jsonb, 'filters', '[]'::jsonb, 'orderby', '[]'::jsonb,
                'row_limit', 1000, 'rollup_groups', '[]'::jsonb),
              'settings', jsonb_build_object('cartesian.xAxis', 'label')),
            owner_id)
    ON CONFLICT (id) DO NOTHING;
    made := made || ' chart';
  END IF;

  SELECT id INTO dash FROM analytics.dashboards
   WHERE workspace_id = ws AND deleted_at IS NULL ORDER BY updated_at DESC LIMIT 1;
  IF dash IS NULL THEN
    dash := md5(ws::text || ':seed-alerts:dashboard')::uuid;
    INSERT INTO analytics.dashboards (id, workspace_id, name, description, created_by, updated_by)
    VALUES (dash, ws, 'Seed overview', 'Placeholder dashboard created by seed-alerts.sql', owner_id, owner_id)
    ON CONFLICT (id) DO NOTHING;
    INSERT INTO analytics.dashboard_cards (dashboard_id, chart_id, grid_col, grid_row, size_x, size_y, ordinal)
    VALUES (dash, ch, 0, 0, 12, 6, 0);
    made := made || ' dashboard';
  END IF;

  ---------------------------------------------------------------- schedules --
  -- Clear this script's own previous run. The CASCADE on schedule_id takes the
  -- owners, recipients and runs with it, so there is no partial state to merge.
  DELETE FROM automation.schedules
   WHERE workspace_id = ws
     AND id IN (SELECT md5(ws::text || ':seed-alerts:s' || i)::uuid FROM generate_series(1, 10) i);

  -- Ten schedules. `next_run_at` is hours out on purpose: a running ticker would
  -- otherwise claim them within one tick and overwrite the state spread below.
  INSERT INTO automation.schedules
    (id, workspace_id, type, name, description, active, crontab, timezone, next_run_at,
     database_id, sql, validator_type, validator_config,
     last_eval_at, last_state, last_value, chart_id, dashboard_id, report_format,
     grace_period_s, email_subject, created_by, updated_by)
  VALUES
    -- 1. The everyday case: fired, notified, has history.
    (md5(ws::text || ':seed-alerts:s1')::uuid, ws, 'alert',
     'Orders below target', 'Fires when yesterday''s order count drops under 500.',
     true, '0 9 * * *', 'Asia/Ho_Chi_Minh', now() + interval '9 hours',
     db, 'SELECT count(*) AS orders FROM orders WHERE created_at >= current_date - 1',
     'operator', '{"op": "<", "threshold": 500}'::jsonb,
     now() - interval '3 hours', 'success', 412, NULL, NULL, NULL,
     14400, 'Order volume below target', owner_id, owner_id),

    -- 2. Broken query — the run log carries the engine's own words.
    (md5(ws::text || ':seed-alerts:s2')::uuid, ws, 'alert',
     'Checkout error rate', 'Alerts when the checkout 5xx rate goes above 2%.',
     true, '*/30 * * * *', 'UTC', now() + interval '4 hours',
     db, 'SELECT error_rate FROM checkout_health', 'operator',
     '{"op": ">", "threshold": 0.02}'::jsonb,
     now() - interval '25 minutes', 'error', NULL, NULL, NULL, NULL,
     14400, NULL, owner_id, owner_id),

    -- 3. Fired recently, so the next firing is suppressed by the grace period.
    (md5(ws::text || ':seed-alerts:s3')::uuid, ws, 'alert',
     'Warehouse disk pressure', 'Suppressed by its grace period after the last firing.',
     true, '0 * * * *', 'UTC', now() + interval '2 hours',
     db, 'SELECT pct_used FROM warehouse_disk', 'operator',
     '{"op": ">=", "threshold": 85}'::jsonb,
     now() - interval '20 minutes', 'grace', 91.4, NULL, NULL, NULL,
     14400, NULL, owner_id, owner_id),

    -- 4. Ran, condition not met, nobody notified.
    (md5(ws::text || ':seed-alerts:s4')::uuid, ws, 'alert',
     'Nightly ETL row drift', 'Ran clean — the condition was not met.',
     true, '30 2 * * *', 'UTC', now() + interval '14 hours',
     db, 'SELECT abs(today_rows - yesterday_rows) AS drift FROM etl_rowcounts',
     'operator', '{"op": "not between", "low": 0, "high": 1000}'::jsonb,
     now() - interval '6 hours', 'noop', 240, NULL, NULL, NULL,
     14400, NULL, owner_id, owner_id),

    -- 5. Mid-run. Reaped into `error` once working_timeout_s elapses.
    (md5(ws::text || ':seed-alerts:s5')::uuid, ws, 'alert',
     'Revenue anomaly sweep', 'Currently evaluating.',
     true, '15 */6 * * *', 'UTC', now() + interval '5 hours',
     db, 'SELECT revenue FROM daily_revenue ORDER BY d DESC LIMIT 1',
     'not_null', '{}'::jsonb,
     now() - interval '2 minutes', 'working', NULL, NULL, NULL, NULL,
     14400, NULL, owner_id, owner_id),

    -- 6. Paused and never run: the "Never run" badge with no history at all.
    (md5(ws::text || ':seed-alerts:s6')::uuid, ws, 'alert',
     'Fraud score spike (paused)', 'Paused before it ever ran.',
     false, '0 */2 * * *', 'UTC', NULL,
     db, 'SELECT max(score) AS s FROM fraud_scores WHERE ts > now() - interval ''2'' hour',
     'operator', '{"op": ">", "threshold": 0.9}'::jsonb,
     NULL, NULL, NULL, NULL, NULL, NULL,
     14400, NULL, owner_id, owner_id),

    -- 7. PNG of a dashboard — the renderer path.
    (md5(ws::text || ':seed-alerts:s7')::uuid, ws, 'report',
     'Monday morning overview', 'Dashboard PNG, emailed at the start of the week.',
     true, '0 8 * * 1', 'Asia/Ho_Chi_Minh', now() + interval '20 hours',
     NULL, NULL, NULL, '{}'::jsonb,
     now() - interval '2 days', 'success', NULL, NULL, dash, 'png',
     14400, 'Weekly overview', owner_id, owner_id),

    -- 8. PDF of the same dashboard, paused, last attempt failed.
    (md5(ws::text || ':seed-alerts:s8')::uuid, ws, 'report',
     'Board pack (paused)', 'Dashboard PDF. Paused after the renderer timed out.',
     false, '0 7 1 * *', 'UTC', NULL,
     NULL, NULL, NULL, '{}'::jsonb,
     now() - interval '9 days', 'error', NULL, NULL, dash, 'pdf',
     14400, NULL, owner_id, owner_id),

    -- 9. CSV of a chart. A dashboard CANNOT take csv/text — the target is a chart.
    (md5(ws::text || ':seed-alerts:s9')::uuid, ws, 'report',
     'Daily orders extract', 'Chart CSV for the ops mailing list.',
     true, '0 6 * * *', 'Asia/Ho_Chi_Minh', now() + interval '18 hours',
     NULL, NULL, NULL, '{}'::jsonb,
     now() - interval '18 hours', 'success', NULL, ch, NULL, 'csv',
     14400, 'Daily orders', owner_id, owner_id),

    -- 10. Text of a chart, never run.
    (md5(ws::text || ':seed-alerts:s10')::uuid, ws, 'report',
     'Orders digest (text)', 'Plain-text chart digest. Created, not yet due.',
     true, '0 12 * * 5', 'UTC', now() + interval '3 days',
     NULL, NULL, NULL, '{}'::jsonb,
     NULL, NULL, NULL, ch, NULL, 'text',
     14400, NULL, owner_id, owner_id);

  ----------------------------------------------------------------- owners ----
  INSERT INTO automation.schedule_owners (schedule_id, user_id)
  SELECT s.id, owner_id FROM automation.schedules s
   WHERE s.workspace_id = ws
     AND s.id IN (SELECT md5(ws::text || ':seed-alerts:s' || i)::uuid FROM generate_series(1, 10) i);

  ------------------------------------------------------------- recipients ----
  -- Email everywhere; two of them also POST to a webhook, so both destination
  -- kinds render in the list and the editor.
  INSERT INTO automation.schedule_recipients (schedule_id, kind, config)
  SELECT md5(ws::text || ':seed-alerts:s' || i)::uuid, 'email',
         jsonb_build_object('addresses', CASE
           WHEN i % 3 = 0 THEN jsonb_build_array('data-team@example.com', 'oncall@example.com')
           ELSE jsonb_build_array('data-team@example.com') END)
    FROM generate_series(1, 10) i;

  INSERT INTO automation.schedule_recipients (schedule_id, kind, config)
  SELECT md5(ws::text || ':seed-alerts:s' || i)::uuid, 'webhook',
         jsonb_build_object('url', 'https://hooks.example.com/egret/' || i)
    FROM (VALUES (1), (3)) AS t(i);

  -------------------------------------------------------------- run history --
  -- Schedule 1 gets a real backlog to page through; the rest get the one or two
  -- runs their last_state implies, so badge and log never disagree.
  INSERT INTO automation.schedule_runs
    (schedule_id, state, started_at, ended_at, value, error, notification_sent_at)
  SELECT md5(ws::text || ':seed-alerts:s1')::uuid,
         CASE WHEN d % 5 = 0 THEN 'noop' ELSE 'success' END,
         now() - (d || ' days')::interval,
         now() - (d || ' days')::interval + interval '4 seconds',
         480 + (d * 7) % 120,
         NULL,
         CASE WHEN d % 5 = 0 THEN NULL
              ELSE now() - (d || ' days')::interval + interval '5 seconds' END
    FROM generate_series(1, 14) d;

  INSERT INTO automation.schedule_runs
    (schedule_id, state, started_at, ended_at, value, error, notification_sent_at)
  VALUES
    (md5(ws::text || ':seed-alerts:s1')::uuid, 'success',
     now() - interval '3 hours', now() - interval '3 hours' + interval '4 seconds',
     412, NULL, now() - interval '3 hours' + interval '5 seconds'),

    (md5(ws::text || ':seed-alerts:s2')::uuid, 'error',
     now() - interval '25 minutes', now() - interval '25 minutes' + interval '2 seconds',
     NULL, 'line 1:22: Table ''checkout_health'' does not exist', NULL),
    (md5(ws::text || ':seed-alerts:s2')::uuid, 'error',
     now() - interval '55 minutes', now() - interval '55 minutes' + interval '2 seconds',
     NULL, 'line 1:22: Table ''checkout_health'' does not exist', NULL),

    (md5(ws::text || ':seed-alerts:s3')::uuid, 'grace',
     now() - interval '20 minutes', now() - interval '20 minutes' + interval '1 second',
     91.4, NULL, NULL),
    (md5(ws::text || ':seed-alerts:s3')::uuid, 'success',
     now() - interval '80 minutes', now() - interval '80 minutes' + interval '3 seconds',
     88.2, NULL, now() - interval '80 minutes' + interval '4 seconds'),

    (md5(ws::text || ':seed-alerts:s4')::uuid, 'noop',
     now() - interval '6 hours', now() - interval '6 hours' + interval '3 seconds',
     240, NULL, NULL),

    (md5(ws::text || ':seed-alerts:s5')::uuid, 'working',
     now() - interval '2 minutes', NULL, NULL, NULL, NULL),

    (md5(ws::text || ':seed-alerts:s7')::uuid, 'success',
     now() - interval '2 days', now() - interval '2 days' + interval '11 seconds',
     NULL, NULL, now() - interval '2 days' + interval '12 seconds'),

    (md5(ws::text || ':seed-alerts:s8')::uuid, 'error',
     now() - interval '9 days', now() - interval '9 days' + interval '30 seconds',
     NULL, 'render timed out after 30000ms', NULL),

    (md5(ws::text || ':seed-alerts:s9')::uuid, 'success',
     now() - interval '18 hours', now() - interval '18 hours' + interval '6 seconds',
     NULL, NULL, now() - interval '18 hours' + interval '7 seconds');

  ------------------------------------------------------------------- SMTP ----
  -- Points at mailpit. The HOST is a parameter because the answer differs by
  -- stack: `localhost` for `deploy/docker-compose.dev.yml`, the service name
  -- inside a cluster. Hardcoding `localhost` meant every kind run had someone
  -- editing this row by hand, which is not a seed.
  -- No username, so no vault row is needed and `smtp_auth_is_complete` holds.
  INSERT INTO automation.smtp_config
    (workspace_id, host, port, username, from_address, from_name, tls_mode, password_secret_id, updated_by)
  VALUES (ws, smtp_host, 1025, NULL, 'alerts@egret.local', 'egret alerts', 'none', NULL, owner_id)
  ON CONFLICT (workspace_id) DO UPDATE
     SET host = EXCLUDED.host, port = EXCLUDED.port, from_address = EXCLUDED.from_address,
         from_name = EXCLUDED.from_name, tls_mode = EXCLUDED.tls_mode, updated_at = now();

  RAISE NOTICE 'seeded 10 schedules into workspace "%"%', ws_slug,
    CASE WHEN made = '' THEN ' (reused its existing database/chart/dashboard)'
         ELSE ' (created placeholder' || made || ')' END;
END
$seed$;

-- What the UI should now show.
SELECT s.type, s.name, s.active, coalesce(s.last_state, 'never run') AS state,
       s.report_format, count(r.id) AS runs
  FROM automation.schedules s
  LEFT JOIN automation.schedule_runs r ON r.schedule_id = s.id
 WHERE s.workspace_id = (SELECT id FROM workspaces WHERE slug = current_setting('seed.ws'))
 GROUP BY s.id, s.type, s.name, s.active, s.last_state, s.report_format
 ORDER BY s.type, s.name;
