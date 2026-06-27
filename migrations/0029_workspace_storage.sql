CREATE TABLE workspace_storage (
  workspace_id uuid PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  provider     text NOT NULL DEFAULT 's3',
  endpoint     text,
  region       text NOT NULL,
  bucket       text NOT NULL,
  key_prefix   text,
  access_key   text NOT NULL,
  secret_ref   uuid,
  path_style   boolean NOT NULL DEFAULT true,
  created_at   timestamptz NOT NULL DEFAULT now()
);
