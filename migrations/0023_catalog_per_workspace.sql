-- Spec A: per-workspace, normalized Iceberg catalog.
-- egret is pre-release and catalog tenancy was "deferred" — drop & recreate (no backfill).

DROP TABLE IF EXISTS workspace_catalog_grant CASCADE;
DROP TABLE IF EXISTS catalog_table_metadata_log CASCADE;
DROP TABLE IF EXISTS catalog_views CASCADE;
DROP TABLE IF EXISTS catalog_tables CASCADE;
DROP TABLE IF EXISTS catalog_namespaces CASCADE;

-- One managed Iceberg catalog per workspace
CREATE TABLE catalogs (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id       uuid NOT NULL UNIQUE REFERENCES workspaces(id) ON DELETE CASCADE,
    name               text NOT NULL,
    warehouse_location text NOT NULL,
    properties         jsonb NOT NULL DEFAULT '{}',
    created_at         timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE catalog_namespaces (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    catalog_id uuid NOT NULL REFERENCES catalogs(id) ON DELETE CASCADE,
    levels     text[] NOT NULL,
    properties jsonb  NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (catalog_id, levels)
);

CREATE TABLE catalog_tables (
    id                         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    namespace_id               uuid NOT NULL REFERENCES catalog_namespaces(id) ON DELETE RESTRICT,
    name                       text NOT NULL,
    table_uuid                 uuid NOT NULL,
    location                   text NOT NULL,
    format_version             smallint NOT NULL,
    last_column_id             int NOT NULL,
    last_sequence_number       bigint NOT NULL,
    last_updated_ms            bigint NOT NULL,
    last_partition_id          int NOT NULL,
    current_schema_id          int NOT NULL,
    default_spec_id            int NOT NULL,
    default_sort_order_id      int NOT NULL,
    current_snapshot_id        bigint,
    metadata_location          text NOT NULL,
    previous_metadata_location text,
    current_version            int NOT NULL DEFAULT 0,
    created_at                 timestamptz NOT NULL DEFAULT now(),
    updated_at                 timestamptz NOT NULL DEFAULT now(),
    UNIQUE (namespace_id, name),
    UNIQUE (table_uuid)
);

CREATE TABLE catalog_table_schemas (
    table_id  uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    schema_id int  NOT NULL,
    schema    jsonb NOT NULL,
    PRIMARY KEY (table_id, schema_id)
);
CREATE TABLE catalog_table_partition_specs (
    table_id uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    spec_id  int  NOT NULL,
    spec     jsonb NOT NULL,
    PRIMARY KEY (table_id, spec_id)
);
CREATE TABLE catalog_table_sort_orders (
    table_id      uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    sort_order_id int  NOT NULL,
    sort_order    jsonb NOT NULL,
    PRIMARY KEY (table_id, sort_order_id)
);
CREATE TABLE catalog_table_properties (
    table_id uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    key      text NOT NULL,
    value    text NOT NULL,
    PRIMARY KEY (table_id, key)
);
CREATE TABLE catalog_table_snapshots (
    table_id           uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    snapshot_id        bigint NOT NULL,
    parent_snapshot_id bigint,
    sequence_number    bigint NOT NULL,
    manifest_list      text NOT NULL,
    summary            jsonb NOT NULL,
    schema_id          int,
    timestamp_ms       bigint NOT NULL,
    PRIMARY KEY (table_id, snapshot_id)
);
CREATE TABLE catalog_table_snapshot_log (
    table_id     uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    seq          bigserial,
    snapshot_id  bigint NOT NULL,
    timestamp_ms bigint NOT NULL,
    PRIMARY KEY (table_id, seq)
);
CREATE TABLE catalog_table_metadata_log (
    table_id      uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    seq           bigserial,
    timestamp_ms  bigint NOT NULL,
    metadata_file text NOT NULL,
    PRIMARY KEY (table_id, seq)
);
CREATE TABLE catalog_table_refs (
    table_id    uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    ref_name    text NOT NULL,
    snapshot_id bigint NOT NULL,
    type        text NOT NULL,
    retention   jsonb NOT NULL,
    PRIMARY KEY (table_id, ref_name),
    FOREIGN KEY (table_id, snapshot_id) REFERENCES catalog_table_snapshots(table_id, snapshot_id)
);
CREATE TABLE catalog_table_statistics (
    table_id                  uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    snapshot_id               bigint NOT NULL,
    statistics_path           text NOT NULL,
    file_size_in_bytes        bigint NOT NULL,
    file_footer_size_in_bytes bigint NOT NULL,
    key_metadata              text,
    blob_metadata             jsonb NOT NULL,
    PRIMARY KEY (table_id, snapshot_id)
);
CREATE TABLE catalog_table_partition_statistics (
    table_id           uuid NOT NULL REFERENCES catalog_tables(id) ON DELETE CASCADE,
    snapshot_id        bigint NOT NULL,
    statistics_path    text NOT NULL,
    file_size_in_bytes bigint NOT NULL,
    PRIMARY KEY (table_id, snapshot_id)
);

CREATE TABLE catalog_views (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    namespace_id       uuid NOT NULL REFERENCES catalog_namespaces(id) ON DELETE RESTRICT,
    name               text NOT NULL,
    view_uuid          uuid NOT NULL,
    location           text NOT NULL,
    format_version     smallint NOT NULL,
    current_version_id int NOT NULL,           -- Iceberg current-version-id
    metadata_location  text NOT NULL,
    current_version    int NOT NULL DEFAULT 0, -- egret CAS optimistic-lock counter
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    UNIQUE (namespace_id, name),
    UNIQUE (view_uuid)
);
CREATE TABLE catalog_view_schemas (
    view_id   uuid NOT NULL REFERENCES catalog_views(id) ON DELETE CASCADE,
    schema_id int  NOT NULL,
    schema    jsonb NOT NULL,
    PRIMARY KEY (view_id, schema_id)
);
CREATE TABLE catalog_view_versions (
    view_id      uuid NOT NULL REFERENCES catalog_views(id) ON DELETE CASCADE,
    version_id   int  NOT NULL,
    schema_id    int  NOT NULL,
    timestamp_ms bigint NOT NULL,
    summary      jsonb NOT NULL,
    default_catalog   text,
    default_namespace jsonb,
    PRIMARY KEY (view_id, version_id),
    FOREIGN KEY (view_id, schema_id) REFERENCES catalog_view_schemas(view_id, schema_id)
);
CREATE TABLE catalog_view_representations (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    view_id    uuid NOT NULL REFERENCES catalog_views(id) ON DELETE CASCADE,
    version_id int  NOT NULL,
    type       text NOT NULL DEFAULT 'sql',
    sql        text NOT NULL,
    dialect    text NOT NULL,
    FOREIGN KEY (view_id, version_id) REFERENCES catalog_view_versions(view_id, version_id) ON DELETE CASCADE
);
CREATE TABLE catalog_view_version_log (
    view_id      uuid NOT NULL REFERENCES catalog_views(id) ON DELETE CASCADE,
    seq          bigserial,
    version_id   int NOT NULL,
    timestamp_ms bigint NOT NULL,
    PRIMARY KEY (view_id, seq)
);
CREATE TABLE catalog_view_properties (
    view_id uuid NOT NULL REFERENCES catalog_views(id) ON DELETE CASCADE,
    key     text NOT NULL,
    value   text NOT NULL,
    PRIMARY KEY (view_id, key)
);

-- Engine/kernel access tokens, scoped per workspace catalog (used in a later plan)
CREATE TABLE catalog_tokens (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    token_hash   text NOT NULL,
    label        text NOT NULL DEFAULT '',
    created_at   timestamptz NOT NULL DEFAULT now(),
    revoked_at   timestamptz,
    UNIQUE (token_hash)
);

CREATE INDEX catalog_view_reprs_vid_ver_idx ON catalog_view_representations (view_id, version_id);
CREATE INDEX catalog_tokens_workspace_idx ON catalog_tokens (workspace_id);
