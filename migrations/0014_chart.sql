CREATE TABLE charts (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    dataset_id   uuid NOT NULL REFERENCES datasets(id)   ON DELETE CASCADE,
    name         text NOT NULL,
    description  text NOT NULL DEFAULT '',
    viz_type     text NOT NULL,
    params       jsonb NOT NULL,
    created_by   uuid REFERENCES users(id),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT chart_name_uniq UNIQUE (workspace_id, name)
);
CREATE INDEX charts_workspace_idx ON charts (workspace_id);
CREATE INDEX charts_dataset_idx   ON charts (dataset_id);

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'chart:read'   FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'chart:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
