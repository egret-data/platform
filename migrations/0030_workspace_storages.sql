CREATE TABLE workspace_storages (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name         text NOT NULL,
  purpose      text,
  provider     text NOT NULL DEFAULT 's3',
  endpoint     text,
  region       text NOT NULL,
  bucket       text NOT NULL,
  key_prefix   text,
  access_key   text NOT NULL,
  secret_ref   uuid,
  path_style   boolean NOT NULL DEFAULT true,
  created_at   timestamptz NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, name)
);
