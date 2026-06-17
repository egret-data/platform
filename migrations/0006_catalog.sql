CREATE TABLE catalog_namespaces (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    levels     text[] NOT NULL,
    properties jsonb  NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (levels)
);

CREATE TABLE catalog_tables (
    id                         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    namespace_id               uuid NOT NULL REFERENCES catalog_namespaces(id) ON DELETE RESTRICT,
    name                       text NOT NULL,
    table_uuid                 uuid NOT NULL,
    metadata                   jsonb NOT NULL,
    metadata_location          text NOT NULL,
    previous_metadata_location text,
    location                   text NOT NULL,
    current_snapshot_id        bigint,
    current_version            int  NOT NULL DEFAULT 0,
    created_at                 timestamptz NOT NULL DEFAULT now(),
    updated_at                 timestamptz NOT NULL DEFAULT now(),
    UNIQUE (namespace_id, name)
);

CREATE TABLE catalog_table_metadata_log (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    table_id          uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    metadata_location text NOT NULL,
    created_at        timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE catalog_views (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    namespace_id      uuid NOT NULL REFERENCES catalog_namespaces(id) ON DELETE RESTRICT,
    name              text NOT NULL,
    metadata          jsonb NOT NULL,
    metadata_location text NOT NULL,
    location          text NOT NULL,
    current_version   int  NOT NULL DEFAULT 0,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    UNIQUE (namespace_id, name)
);

-- forward-compat; NOT enforced this phase (tenancy deferred)
CREATE TABLE workspace_catalog_grant (
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    namespace_id uuid NOT NULL REFERENCES catalog_namespaces(id) ON DELETE CASCADE,
    granted_by   uuid REFERENCES users(id),
    granted_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, namespace_id)
);
