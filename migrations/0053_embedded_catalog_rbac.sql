-- Embedded-catalog RBAC (egret-owned): warehouses per workspace/domain, roles
-- keyed by consumer-type, grants, and per-consumer Catalog Clients.
CREATE TABLE warehouses (
    id                      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name                    text NOT NULL,
    workspace_id            uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    storage_conn_id         uuid NOT NULL REFERENCES storage_connections(id),
    lakekeeper_warehouse_id text,
    created_at              timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);

CREATE TABLE catalog_roles (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    warehouse_id  uuid NOT NULL REFERENCES warehouses(id) ON DELETE CASCADE,
    name          text NOT NULL,
    consumer_type text NOT NULL CHECK (consumer_type IN ('engine','notebook','external')),
    UNIQUE (warehouse_id, name)
);

CREATE TABLE catalog_grants (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    catalog_role_id uuid NOT NULL REFERENCES catalog_roles(id) ON DELETE CASCADE,
    securable_type  text NOT NULL CHECK (securable_type IN ('catalog','namespace','table')),
    namespace       text[],
    table_name      text,
    privilege       text NOT NULL
);

CREATE TABLE catalog_clients (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name               text NOT NULL,
    consumer_type      text NOT NULL CHECK (consumer_type IN ('engine','notebook','external')),
    workspace_id       uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    warehouse_id       uuid NOT NULL REFERENCES warehouses(id) ON DELETE CASCADE,
    catalog_role_id    uuid NOT NULL REFERENCES catalog_roles(id),
    client_id          text NOT NULL UNIQUE,
    client_secret_hash text NOT NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    last_used_at       timestamptz
);
CREATE INDEX catalog_clients_client_id_idx ON catalog_clients(client_id);
CREATE INDEX catalog_clients_warehouse_idx ON catalog_clients(warehouse_id);
