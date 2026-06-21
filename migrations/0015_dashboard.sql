CREATE TABLE dashboards (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name         text NOT NULL,
    description  text NOT NULL DEFAULT '',
    created_by   uuid REFERENCES users(id),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT dashboard_name_uniq UNIQUE (workspace_id, name)
);
CREATE INDEX dashboards_workspace_idx ON dashboards (workspace_id);

-- One placed card on a dashboard's grid. chart_id is the saved chart it renders
-- (nullable so Phase B text/heading cards can omit it). Grid units mirror Metabase:
-- a 24-column grid (Metabase's GRID_WIDTH), row height in grid rows; (col,row) is the top-left cell.
CREATE TABLE dashboard_cards (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dashboard_id uuid NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
    chart_id     uuid REFERENCES charts(id) ON DELETE CASCADE,
    card_type    text NOT NULL DEFAULT 'chart',
    grid_col     int  NOT NULL DEFAULT 0,
    grid_row     int  NOT NULL DEFAULT 0,
    size_x       int  NOT NULL DEFAULT 6,
    size_y       int  NOT NULL DEFAULT 4,
    options      jsonb NOT NULL DEFAULT '{}',
    ordinal      int  NOT NULL DEFAULT 0
);
CREATE INDEX dashboard_cards_dashboard_idx ON dashboard_cards (dashboard_id);

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'dashboard:read'   FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'dashboard:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
