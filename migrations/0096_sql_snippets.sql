-- Reusable SQL snippets for SQL Lab (workspace-scoped content, like saved
-- queries). Users insert a snippet's body into the editor.
CREATE TABLE analytics.sql_snippets (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name         text NOT NULL,
  body         text NOT NULL,
  description  text NOT NULL DEFAULT '',
  created_by   uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sql_snippets_ws_idx ON analytics.sql_snippets (workspace_id, updated_at DESC);
