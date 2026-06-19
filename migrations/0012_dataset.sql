CREATE TABLE datasets (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    database_id  uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    name         text NOT NULL,
    description  text NOT NULL DEFAULT '',
    catalog      text,
    schema_name  text,
    table_name   text,
    sql          text,
    created_by   uuid REFERENCES users(id),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT dataset_one_kind CHECK ((table_name IS NOT NULL) <> (sql IS NOT NULL)),
    CONSTRAINT dataset_name_uniq UNIQUE (workspace_id, name)
);

CREATE TABLE dataset_columns (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dataset_id   uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    column_name  text NOT NULL,
    data_type    text NOT NULL,
    type_generic text NOT NULL,
    is_temporal  boolean NOT NULL DEFAULT false,
    ordinal      int NOT NULL,
    UNIQUE (dataset_id, column_name)
);
CREATE INDEX dataset_columns_dataset_idx ON dataset_columns (dataset_id);

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'dataset:read'   FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'dataset:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
