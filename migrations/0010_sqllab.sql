CREATE TABLE saved_queries (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  database_id  uuid REFERENCES databases(id) ON DELETE SET NULL,
  name         text NOT NULL,
  sql          text NOT NULL,
  created_by   uuid REFERENCES users(id),
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX saved_queries_ws_idx ON saved_queries (workspace_id, updated_at DESC);
