-- Per-workspace SMTP (P3). One row per workspace: mail is sent as the
-- workspace's own identity, not the platform's, so two tenants never share a
-- from-address or a relay's reputation.
--
-- THE PASSWORD IS NOT A COLUMN. It goes through `egret-secrets` as an encrypted
-- vault row and only its id is stored here. A password in a plain column is the
-- shape that leaks through a `SELECT *` in a support session, a pg_dump handed
-- to a contractor, or a log line that echoed a row.
CREATE TABLE automation.smtp_config (
  workspace_id       uuid PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  host               text NOT NULL,
  port               integer NOT NULL,
  username           text,
  from_address       text NOT NULL,
  from_name          text,
  tls_mode           text NOT NULL DEFAULT 'starttls',
  password_secret_id uuid REFERENCES secrets(id) ON DELETE SET NULL,
  updated_by         uuid REFERENCES users(id) ON DELETE SET NULL,
  updated_at         timestamptz NOT NULL DEFAULT now(),

  -- Closed at the database, like every other enum in this schema: a tls_mode
  -- typo would otherwise be a config that silently falls back to whatever the
  -- Rust match arm treats as its default — which for TLS is the one mistake
  -- you cannot see from the outside.
  CONSTRAINT smtp_tls_mode_valid CHECK (tls_mode IN ('starttls', 'implicit', 'none')),
  CONSTRAINT smtp_port_valid CHECK (port > 0 AND port <= 65535),
  -- A username with no password (or the reverse) is a half-configured relay
  -- that fails at send time; catching it here makes the editor say so.
  CONSTRAINT smtp_auth_is_complete CHECK (
    (username IS NULL AND password_secret_id IS NULL)
    OR (username IS NOT NULL AND password_secret_id IS NOT NULL)
  )
);
