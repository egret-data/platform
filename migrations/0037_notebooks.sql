-- Notebook C1: per-workspace notebook documents + ordered cells, + RBAC perms.
CREATE TABLE notebooks (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    owner_user_id uuid NOT NULL REFERENCES users(id),
    name          text NOT NULL DEFAULT 'Untitled',
    description   text NOT NULL DEFAULT '',
    is_public     boolean NOT NULL DEFAULT false,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_notebooks_ws_owner ON notebooks(workspace_id, owner_user_id);

CREATE TABLE notebook_cells (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    notebook_id  uuid NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    cell_type    text NOT NULL DEFAULT 'code',
    source       text NOT NULL DEFAULT '',
    cell_order   integer NOT NULL DEFAULT 0,
    execution_count        integer,
    last_output            jsonb,
    last_execution_time_ms integer,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_notebook_cells_notebook ON notebook_cells(notebook_id, cell_order);

-- RBAC: notebook:read for all workspace roles; notebook:write for admin + editor.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'notebook:read' FROM roles
WHERE scope = 'workspace' AND name IN ('ws_admin','ws_editor','ws_viewer')
ON CONFLICT (role_id, permission) DO NOTHING;

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'notebook:write' FROM roles
WHERE scope = 'workspace' AND name IN ('ws_admin','ws_editor')
ON CONFLICT (role_id, permission) DO NOTHING;
