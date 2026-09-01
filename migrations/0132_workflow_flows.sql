-- Workflow W1: flow drafts and their immutable published versions.
--
-- TWO tables, and the split is the whole feature, for the same reason recorded
-- in 0121 for dataapi.endpoints: if execution read the draft, editing a flow
-- would change the behaviour of tonight's scheduled run, and "versioning" would
-- be decoration.
--
-- `namespace` is Kestra's, kept because every Kestra flow has one and rejecting
-- it would break paste-compatibility at its most visible point. Uniqueness is
-- (workspace_id, namespace, flow_id) -- Kestra's (tenant, namespace, id) with
-- the workspace in the tenant's place. `folder_id` remains the UI's organising
-- axis, as it is for dataapi.endpoints, and is deliberately NOT derived from
-- namespace: folders get renamed and re-parented, a namespace is identity.
CREATE SCHEMA IF NOT EXISTS workflow;

CREATE TABLE workflow.flows (
  id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id    uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  folder_id       uuid REFERENCES analytics.folders(id) ON DELETE SET NULL,
  namespace       text NOT NULL,
  flow_id         text NOT NULL,          -- the YAML's `id`; `id` is taken by the surrogate key
  source          text NOT NULL,          -- the YAML the author wrote, byte for byte
  plan            jsonb NOT NULL,         -- the parsed, normalised model
  state           text NOT NULL DEFAULT 'draft' CHECK (state IN ('draft','live','disabled')),
  live_version_id uuid,
  created_by      uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at      timestamptz NOT NULL DEFAULT now(),
  updated_at      timestamptz NOT NULL DEFAULT now(),
  deleted_at      timestamptz,
  UNIQUE (workspace_id, namespace, flow_id)
);

CREATE TABLE workflow.flow_versions (
  id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  flow_id     uuid NOT NULL REFERENCES workflow.flows(id) ON DELETE CASCADE,
  version     integer NOT NULL,
  snapshot    jsonb NOT NULL,   -- source + normalised plan + RESOLVED references, self-contained
  note        text NOT NULL DEFAULT '',
  created_by  uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at  timestamptz NOT NULL DEFAULT now(),
  UNIQUE (flow_id, version)
);

ALTER TABLE workflow.flows
  ADD CONSTRAINT flows_live_version_fk
  FOREIGN KEY (live_version_id) REFERENCES workflow.flow_versions(id) ON DELETE SET NULL;

CREATE INDEX workflow_flows_ws_idx ON workflow.flows (workspace_id);
CREATE INDEX workflow_flow_versions_flow_idx
  ON workflow.flow_versions (flow_id, version DESC);

-- Managing flows is an editor act, following alert:manage (0110) and
-- chart:manage (0014).
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'workflow:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
