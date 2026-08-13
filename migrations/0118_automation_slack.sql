-- Per-workspace Slack, mirroring `automation.smtp_config` (0111).
--
-- The bot token lived on EVERY slack recipient, so a workspace posting to
-- three channels pasted the same credential three times, rotated it in three
-- places, and had three vault rows to leak. A Slack app is a property of the
-- workspace, exactly like its SMTP relay; a recipient should only have to name
-- a channel.
--
-- THE TOKEN IS NOT A COLUMN — it goes through `egret-secrets` and only its id
-- is stored here, for the reasons 0111 spells out for the SMTP password.
CREATE TABLE automation.slack_config (
  workspace_id    uuid PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  -- RESTRICT, not SET NULL: a config row whose token vanished is a workspace
  -- that looks configured and silently cannot deliver. Deleting the secret has
  -- to go through the config's own delete.
  token_secret_id uuid NOT NULL REFERENCES secrets(id) ON DELETE RESTRICT,
  -- What the editor offers when a schedule names no channel. Optional: a
  -- workspace may prefer to be explicit every time.
  default_channel text,
  updated_by      uuid REFERENCES users(id) ON DELETE SET NULL,
  updated_at      timestamptz NOT NULL DEFAULT now(),

  CONSTRAINT slack_default_channel_shape CHECK (
    default_channel IS NULL OR default_channel <> ''
  )
);

-- Backfill: adopt a token that is already working rather than making every
-- existing schedule stop until someone visits the settings page.
--
-- The most recently updated schedule's token wins where a workspace has
-- several. That is a guess, which is why the runner keeps reading a
-- recipient's own `token_id` FIRST and falls back to this one — a recipient
-- whose token differed keeps delivering with it, unchanged, and only new
-- recipients rely on the workspace token. Deliveries do not break; the sprawl
-- stops growing.
INSERT INTO automation.slack_config (workspace_id, token_secret_id)
SELECT DISTINCT ON (s.workspace_id)
       s.workspace_id,
       (r.config->>'token_id')::uuid
  FROM automation.schedule_recipients r
  JOIN automation.schedules s ON s.id = r.schedule_id
 WHERE r.kind = 'slack'
   AND r.config ? 'token_id'
   AND (r.config->>'token_id')::uuid IN (SELECT id FROM secrets)
 ORDER BY s.workspace_id, s.updated_at DESC
ON CONFLICT (workspace_id) DO NOTHING;
