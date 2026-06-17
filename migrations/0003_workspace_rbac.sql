CREATE TABLE workspaces (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slug       text NOT NULL UNIQUE,
    name       text NOT NULL,
    status     text NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
    settings   jsonb NOT NULL DEFAULT '{}',
    created_by uuid REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE workspace_members (
    workspace_id   uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    principal_type text NOT NULL CHECK (principal_type IN ('user','group')),
    principal_id   uuid NOT NULL,
    role           text NOT NULL,
    added_by       uuid REFERENCES users(id),
    added_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, principal_type, principal_id)
);

CREATE TABLE roles (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    scope       text NOT NULL CHECK (scope IN ('platform','workspace')),
    name        text NOT NULL,
    description text NOT NULL DEFAULT '',
    UNIQUE (scope, name)
);

CREATE TABLE role_permissions (
    role_id    uuid NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    permission text NOT NULL,
    PRIMARY KEY (role_id, permission)
);

CREATE TABLE platform_role_assignments (
    principal_type text NOT NULL CHECK (principal_type IN ('user','group')),
    principal_id   uuid NOT NULL,
    role_id        uuid NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    PRIMARY KEY (principal_type, principal_id, role_id)
);

-- Seed roles
INSERT INTO roles (scope, name, description) VALUES
    ('platform','platform_admin','Full platform control'),
    ('platform','data_steward','Manage catalog, policy, databases'),
    ('workspace','ws_admin','Manage workspace members and resources'),
    ('workspace','ws_editor','Create and edit workspace content'),
    ('workspace','ws_viewer','View workspace content');

-- Seed permissions (catalog grows in later phases)
INSERT INTO role_permissions (role_id, permission)
SELECT id, perm FROM roles, unnest(ARRAY[
    'workspace:create','workspace:read','workspace:update','workspace:archive',
    'user:read','user:assign_role'
]) AS perm
WHERE scope='platform' AND name='platform_admin';

INSERT INTO role_permissions (role_id, permission)
SELECT id, perm FROM roles, unnest(ARRAY['workspace:read','user:read']) AS perm
WHERE scope='platform' AND name='data_steward';

INSERT INTO role_permissions (role_id, permission)
SELECT id, 'workspace:manage_members' FROM roles
WHERE scope='workspace' AND name='ws_admin';
