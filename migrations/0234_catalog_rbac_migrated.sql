-- 0234_catalog_rbac_migrated.sql
--
-- SP5b (catalog merge / one grant store): provenance for the reversible fold of
-- an embedded catalog's RBAC (`catalog.catalog_roles` / `catalog_grants`) into
-- `perm.grants`. The fold itself is done by the Rust migrator
-- (`egret_perm_compile::catalog_rbac_migrate`), not here, because it must
-- compute content hashes and attach principals; this migration changes no data.
--
-- One row per catalog (UNIQUE), so a second fold is a no-op. The created ids are
-- what an undo deletes; `source_hash` is the SHA-256 of the canonical
-- roles+grants of the warehouse at fold time, and undo refuses (409) if the live
-- RBAC no longer hashes to it. The source tables are never written.
--
-- `created_owner_ids` / `created_engine_grant_ids` are reserved for the later
-- SP5b tasks (owners, shared `engine` role); this task leaves them empty.
-- `skipped_role_names` records the roles with no bound service account, which
-- contributed no grant, so a re-run can return the same report.

CREATE TABLE perm.catalog_rbac_migrated (
    id                       bigserial   PRIMARY KEY,
    catalog_id               uuid        NOT NULL REFERENCES registry.catalogs(id) ON DELETE CASCADE,
    warehouse_id             uuid        NOT NULL,
    source_role_ids          uuid[]      NOT NULL DEFAULT '{}',
    source_grant_ids         uuid[]      NOT NULL DEFAULT '{}',
    source_hash              text        NOT NULL,
    created_grant_ids        uuid[]      NOT NULL DEFAULT '{}',
    created_owner_ids        uuid[]      NOT NULL DEFAULT '{}',
    created_engine_grant_ids uuid[]      NOT NULL DEFAULT '{}',
    skipped_role_names       text[]      NOT NULL DEFAULT '{}',
    migrated_by              text        NOT NULL,
    migrated_at              timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT catalog_rbac_migrated_once_per_catalog UNIQUE (catalog_id)
);
