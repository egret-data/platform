-- 0147_catalog_registry.sql
-- SP3: a catalog is a platform resource (spec docs/en/specs/2026-09-19-catalog-registry-design.md).
-- `registry.catalogs` holds one connection definition per catalog; engines attach
-- catalogs, workspaces are assigned catalogs. Data is filled by the Rust backfill
-- at boot (egret-catalog-registry::backfill), not here: naming, dedupe, typed
-- parsing and secret extraction need the typed parser and the secret vault.
CREATE SCHEMA IF NOT EXISTS registry;

CREATE TABLE registry.catalogs (
    id                    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    -- The catalog's SQL name on every engine (D5). Immutable.
    name                  text NOT NULL UNIQUE CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$'),
    kind                  text NOT NULL CHECK (kind IN
                              ('iceberg', 'hive', 'delta_lake', 'hudi', 'jdbc',
                               'elasticsearch', 'tpch', 'tpcds', 'generic')),
    -- Typed per kind, validated by egret_catalog_registry::CatalogDef on every
    -- write. Never a plaintext secret: only secret ids and storage ids.
    config                jsonb NOT NULL,
    -- {"trino": {k: v}, "starrocks": {k: v}}, rendered after the typed keys.
    advanced              jsonb NOT NULL DEFAULT '{}'::jsonb,
    description           text,
    -- Mirrors config.metastore.warehouse_id of an egret catalog: one catalog per
    -- warehouse, and the catalog goes when its warehouse goes.
    warehouse_id          uuid REFERENCES catalog.warehouses(id) ON DELETE CASCADE,
    -- Mirrors config.storage_connection_id.
    storage_connection_id uuid REFERENCES storage.storage_connections(id),
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT catalogs_warehouse_mirrors_config CHECK (
        (warehouse_id IS NULL AND NOT (kind = 'iceberg' AND config->'metastore'->>'type' = 'egret'))
        OR (warehouse_id IS NOT NULL AND kind = 'iceberg'
            AND config->'metastore'->>'type' = 'egret'
            AND config->'metastore'->>'warehouse_id' = warehouse_id::text)),
    CONSTRAINT catalogs_storage_mirrors_config CHECK (
        storage_connection_id IS NOT DISTINCT FROM (config->>'storage_connection_id')::uuid)
);
CREATE UNIQUE INDEX catalogs_warehouse_id_key ON registry.catalogs (warehouse_id)
    WHERE warehouse_id IS NOT NULL;
CREATE INDEX catalogs_storage_connection_idx ON registry.catalogs (storage_connection_id);

-- NO ACTION (end of statement), not RESTRICT, on both catalog_id FKs: deleting a
-- workspace cascades to its warehouses and their catalogs in the same statement
-- that removes the workspace's own assignment rows; RESTRICT would fire first.
-- The API still refuses a catalog delete with 409 while anything references it.
CREATE TABLE registry.engine_catalog_attachments (
    engine_id            uuid NOT NULL REFERENCES engine_mgmt.engines(id) ON DELETE CASCADE,
    catalog_id           uuid NOT NULL REFERENCES registry.catalogs(id),
    -- egret catalogs only: the Catalog Client egret minted for this engine…
    catalog_client_id    uuid,
    -- …and the named global secret holding its "client_id:client_secret".
    credential_secret_id uuid,
    -- false: the client belongs to someone else and is never revoked by egret.
    minted_client        boolean NOT NULL DEFAULT true,
    apply_status         text NOT NULL DEFAULT 'applied' CHECK (apply_status IN ('applied', 'failed')),
    apply_error          text,
    applied_at           timestamptz,
    created_at           timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (engine_id, catalog_id),
    CONSTRAINT attachments_client_has_credential
        CHECK (catalog_client_id IS NULL OR credential_secret_id IS NOT NULL)
);
CREATE INDEX engine_catalog_attachments_catalog_idx
    ON registry.engine_catalog_attachments (catalog_id);
CREATE INDEX engine_catalog_attachments_credential_idx
    ON registry.engine_catalog_attachments (credential_secret_id);

CREATE TABLE registry.catalog_workspaces (
    catalog_id   uuid NOT NULL REFERENCES registry.catalogs(id),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    assigned_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (catalog_id, workspace_id)
);
CREATE INDEX catalog_workspaces_workspace_idx ON registry.catalog_workspaces (workspace_id);

-- Every secret a catalog's config references (written with the catalog). Counted
-- by WorkspaceSecretStore::used_by, so a referenced secret cannot be deleted.
CREATE TABLE registry.catalog_secret_refs (
    catalog_id uuid NOT NULL REFERENCES registry.catalogs(id) ON DELETE CASCADE,
    secret_id  uuid NOT NULL,
    PRIMARY KEY (catalog_id, secret_id)
);
CREATE INDEX catalog_secret_refs_secret_idx ON registry.catalog_secret_refs (secret_id);

-- Which engine_mgmt.engine_catalogs rows the backfill has migrated. A row here is
-- never migrated again, so a detach or catalog delete an admin makes later is not
-- undone at the next boot. No FK to engine_catalogs: SP3c drops that table.
CREATE TABLE registry.legacy_engine_catalogs (
    engine_catalog_id uuid PRIMARY KEY,
    engine_id         uuid NOT NULL,
    catalog_id        uuid REFERENCES registry.catalogs(id) ON DELETE SET NULL,
    migrated_at       timestamptz NOT NULL DEFAULT now()
);

-- What the backfill did, for the log and for the Catalogs page (shown until
-- acknowledged). Codes: warehouse_catalog_created, primary_named_egret,
-- engine_catalog_typed, engine_catalog_generic, engine_catalog_shared,
-- engine_catalog_suffixed, engine_catalog_renamed, duplicate_attachment_skipped,
-- secret_extracted, saved_sql_references_catalog, backfill_failed.
CREATE TABLE registry.migration_report (
    id              bigserial PRIMARY KEY,
    run_id          uuid NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    severity        text NOT NULL CHECK (severity IN ('info', 'warning')),
    code            text NOT NULL,
    catalog_id      uuid REFERENCES registry.catalogs(id) ON DELETE SET NULL,
    engine_id       uuid,
    warehouse_id    uuid,
    subject         text NOT NULL,
    message         text NOT NULL,
    detail          jsonb NOT NULL DEFAULT '{}'::jsonb,
    acknowledged_at timestamptz
);
CREATE INDEX migration_report_unacknowledged_idx
    ON registry.migration_report (id) WHERE acknowledged_at IS NULL;
