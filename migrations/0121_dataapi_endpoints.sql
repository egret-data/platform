-- Data API P3: endpoint definitions and their immutable published versions.
--
-- TWO tables, and the split is the whole feature. `endpoints` is the draft an
-- author edits freely. `endpoint_versions` holds frozen snapshots, and the
-- serving path (P4) reads ONLY those. If serving read the draft, editing a
-- query would change behaviour under every client already calling it, and
-- "versioning" would be decoration.
CREATE TABLE dataapi.endpoints (
  id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  -- Cross-schema FK into the ANALYTICS schema (folders lives there, not public),
  -- as dataapi.service_accounts does into users: `folders` is
  -- already a shared table serving chart/dashboard/dataset, and duplicating it
  -- per schema would be worse than the ownership bend.
  folder_id     uuid REFERENCES analytics.folders(id) ON DELETE SET NULL,
  name          text NOT NULL,
  description   text NOT NULL DEFAULT '',
  -- The URL path is FREE-FORM and user-typed. It is deliberately NOT derived
  -- from folder_id: folders are renamed and re-parented freely, and a URL is a
  -- published contract with deployed clients.
  path          text NOT NULL,
  method        text NOT NULL DEFAULT 'GET' CHECK (method IN ('GET','POST')),
  database_id   uuid NOT NULL,
  sql           text NOT NULL,
  params        jsonb NOT NULL DEFAULT '[]',
  output        jsonb NOT NULL DEFAULT '{}',
  cache         jsonb NOT NULL DEFAULT '{}',
  limits        jsonb NOT NULL DEFAULT '{}',
  rate_limit    jsonb NOT NULL DEFAULT '{}',
  source_kind   text NOT NULL DEFAULT 'raw' CHECK (source_kind IN ('raw','saved_query','dataset')),
  source_ref    uuid,
  live_version_id uuid,
  state         text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft','live','disabled')),
  created_by    uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at    timestamptz NOT NULL DEFAULT now(),
  updated_at    timestamptz NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, method, path)
);
CREATE INDEX dataapi_endpoints_ws_idx ON dataapi.endpoints (workspace_id);

CREATE TABLE dataapi.endpoint_versions (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  endpoint_id uuid NOT NULL REFERENCES dataapi.endpoints(id) ON DELETE CASCADE,
  version     integer NOT NULL,
  -- The FULL definition, self-contained: sql, params, resolved output schema,
  -- database_id, cache and limits. Serving must never need to consult the
  -- draft row to answer a request.
  snapshot    jsonb NOT NULL,
  note        text NOT NULL DEFAULT '',
  created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  UNIQUE (endpoint_id, version)
);

-- Added after both tables exist, so the circular reference resolves.
ALTER TABLE dataapi.endpoints
  ADD CONSTRAINT endpoints_live_version_fk
  FOREIGN KEY (live_version_id) REFERENCES dataapi.endpoint_versions(id) ON DELETE SET NULL;
