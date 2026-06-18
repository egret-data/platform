CREATE TABLE queries (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  database_id   uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
  user_id       uuid REFERENCES users(id),
  sql           text NOT NULL,
  status        text NOT NULL DEFAULT 'queued'
                  CHECK (status IN ('queued','running','finished','failed','cancelled')),
  columns_jsonb jsonb,
  preview_jsonb jsonb,
  row_count     bigint,
  bytes_scanned bigint,
  truncated     boolean NOT NULL DEFAULT false,
  result_ref    text,
  error         text,
  submitted_at  timestamptz NOT NULL DEFAULT now(),
  started_at    timestamptz,
  finished_at   timestamptz
);
CREATE INDEX queries_ws_user_idx ON queries (workspace_id, user_id, submitted_at DESC);

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'query:run' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
