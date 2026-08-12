-- Per-render credentials for the report renderer (P4). Deliberately NOT a
-- session: one object, one use, ~60s, bound to the schedule's owner.
CREATE TABLE automation.render_tokens (
  id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  -- Only the HASH is stored, like every other bearer secret in this repo: a
  -- database leak must not yield usable credentials.
  token_hash text NOT NULL UNIQUE,
  kind       text NOT NULL,
  object_id  uuid NOT NULL,
  owner_id   uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  expires_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT render_token_kind_valid CHECK (kind IN ('chart', 'dashboard'))
);
CREATE INDEX render_tokens_expiry_idx ON automation.render_tokens (expires_at);

-- A dashboard has no single table, so a CSV or text rendering of one is either
-- an arbitrary choice of card or a zip of many. Saying so is better than either.
ALTER TABLE automation.schedules
  ADD CONSTRAINT schedule_report_format_matches_target CHECK (
    type <> 'report'
    OR dashboard_id IS NULL
    OR report_format IN ('png', 'pdf')
  );
