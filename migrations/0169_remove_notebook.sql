-- Data Apps P6e: drop the notebook tables and their permission grants.
--
-- IRREVERSIBLE. Every notebook, cell, and kernel-runtime row in this database
-- is deleted here, along with every grant and shared-table row that named
-- "notebook". There is no corresponding rollback migration (the project's
-- append-only convention has no "down" migrations at all) and no export step
-- runs first — see the PR description for why that is believed safe today.
--
-- Order: row-level deletes against tables this feature shared with others
-- first (so no FK or CHECK stands in the way and so the CHECK tightening
-- below has nothing left to violate), then DROP TABLE for the notebook-only
-- tables, children before parents.

-- Shared RBAC grants (egret-core::rbac.rs is the only catalog; `permission`
-- is plain text here, no FK to validate against — see migration 0003).
DELETE FROM role_permissions WHERE permission IN ('notebook:read', 'notebook:write');

-- Shared, polymorphic analytics tables (0067's comment: "analytics.folders and
-- analytics.bookmarks are shared analytics tables ... written by
-- egret-notebook (entity_type='notebook' rows)"). The tables stay; only the
-- notebook-kind rows go.
DELETE FROM analytics.folders WHERE entity_type = 'notebook';
DELETE FROM analytics.bookmarks WHERE entity_type = 'notebook';

-- Shared tagging table (0080): object_type is free text, no FK/CHECK. Rows
-- could exist from `egret-bi`'s generic sharing dispatch before Part P6a
-- removed the "notebook" kind there.
DELETE FROM tag_assignments WHERE object_type = 'notebook';

-- Shared per-workspace feature entitlements (0094): feature is free text,
-- validated only by the code catalog in `egret-workspace/src/features.rs`,
-- which no longer lists "notebooks" as of this same change. A workspace that
-- had explicitly toggled it leaves a now-meaningless row otherwise.
DELETE FROM workspace_features WHERE feature = 'notebooks';

-- Shared embedded-catalog RBAC (0053): consumer_type is engine/notebook/
-- external. Clients before roles -- catalog_clients_catalog_role_id_fkey has
-- no ON DELETE action, so a client still pointing at a notebook role would
-- block that role's delete.
DELETE FROM catalog.catalog_clients WHERE consumer_type = 'notebook';
DELETE FROM catalog.catalog_roles WHERE consumer_type = 'notebook';

-- Tighten both CHECK constraints now that no row can violate the narrower
-- set: "notebook" is no longer a representable consumer_type, not just an
-- unused one.
ALTER TABLE catalog.catalog_roles
  DROP CONSTRAINT catalog_roles_consumer_type_check,
  ADD CONSTRAINT catalog_roles_consumer_type_check
    CHECK (consumer_type = ANY (ARRAY['engine'::text, 'external'::text]));

ALTER TABLE catalog.catalog_clients
  DROP CONSTRAINT catalog_clients_consumer_type_check,
  ADD CONSTRAINT catalog_clients_consumer_type_check
    CHECK (consumer_type = ANY (ARRAY['engine'::text, 'external'::text]));

-- Kernel-runtime tables (0041, 0042, 0067): the `kernel` schema was created
-- solely for `egret-kernel`'s Jupyter Kernel Gateway bookkeeping, which Part
-- P6c already deleted. `kernel.notebook_kernels` references
-- `analytics.notebooks`, so it goes before both the notebooks table and its
-- own schema.
DROP TABLE kernel.notebook_kernels;
DROP TABLE kernel.kernels;
DROP SCHEMA kernel;

-- Notebook tables proper (0037, 0039, 0091; moved into `analytics` by 0067).
-- `notebook_cells` is the only remaining child of `notebooks` at this point.
DROP TABLE analytics.notebook_cells;
DROP TABLE analytics.notebook_engine_access;
DROP TABLE analytics.notebook_warehouse_settings;
DROP TABLE analytics.notebooks;
