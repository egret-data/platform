-- Encrypted credentials (AES-256-GCM): one row per stored password/token.
CREATE TABLE secrets (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind       text NOT NULL,                       -- 'db_password' | 'db_token'
    nonce      bytea NOT NULL,
    ciphertext bytea NOT NULL,
    key_id     text NOT NULL DEFAULT 'cfg',         -- which master key sealed it (rotation reserve)
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Registered query engines (databases).
CREATE TABLE databases (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text NOT NULL UNIQUE,
    engine     text NOT NULL CHECK (engine IN ('trino','starrocks')),
    host       text NOT NULL,
    port       int  NOT NULL,
    options    jsonb NOT NULL DEFAULT '{}',         -- user, http_scheme, catalog/database, ssl…
    secret_id  uuid REFERENCES secrets(id) ON DELETE SET NULL,
    status     text NOT NULL DEFAULT 'unknown',     -- unknown | ok | error (last test)
    created_by uuid REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- Grant a database to a workspace (global register → grant model).
CREATE TABLE workspace_database_grant (
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    database_id  uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    granted_by   uuid REFERENCES users(id),
    granted_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, database_id)
);

-- RBAC: platform_admin + data_steward may manage/read databases.
INSERT INTO role_permissions (role_id, permission)
SELECT id, perm FROM roles, unnest(ARRAY['database:manage','database:read']) AS perm
WHERE scope = 'platform' AND name IN ('platform_admin','data_steward');
