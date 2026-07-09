-- E5b: the Analytics seam and the Kernel-runtime seam own their tables. Move the
-- BI/notebook analytics tables from `public` into their own `analytics` schema and
-- the kernel-runtime tables into a `kernel` schema (mirrors E1b's 0062 for
-- engine_mgmt, E2c's 0063 for policy, E3a's 0064 for storage, E3c's 0065 for
-- catalog, E4c's 0066 for query). SET SCHEMA preserves data, indexes,
-- constraints, and FKs.
CREATE SCHEMA IF NOT EXISTS analytics;
CREATE SCHEMA IF NOT EXISTS kernel;

ALTER TABLE datasets                SET SCHEMA analytics;
ALTER TABLE dataset_columns         SET SCHEMA analytics;
ALTER TABLE dataset_metrics         SET SCHEMA analytics;
ALTER TABLE charts                  SET SCHEMA analytics;
ALTER TABLE dashboards              SET SCHEMA analytics;
ALTER TABLE dashboard_cards         SET SCHEMA analytics;
ALTER TABLE dashboard_tabs          SET SCHEMA analytics;
ALTER TABLE folders                 SET SCHEMA analytics;
ALTER TABLE bookmarks               SET SCHEMA analytics;
ALTER TABLE saved_queries           SET SCHEMA analytics;
ALTER TABLE notebooks               SET SCHEMA analytics;
ALTER TABLE notebook_cells          SET SCHEMA analytics;
ALTER TABLE notebook_engine_access  SET SCHEMA analytics;

ALTER TABLE kernels                 SET SCHEMA kernel;
ALTER TABLE notebook_kernels        SET SCHEMA kernel;

-- `analytics.folders` and `analytics.bookmarks` are shared analytics tables: they
-- are OWNED by egret-bi but also written by egret-notebook (entity_type='notebook'
-- rows). Both crates schema-qualify them; they live in the analytics schema.

-- Cross-seam FKs remain (Postgres allows cross-schema FKs within one database);
-- they are dropped only at a real service split.
-- OUTBOUND (analytics/kernel seams reference other seams):
--   analytics.datasets.workspace_id               -> public.workspaces  -- SEAM-CROSSING (iam/workspace)
--   analytics.datasets.created_by                 -> public.users       -- SEAM-CROSSING (iam)
--   analytics.datasets.database_id                -> query.databases    -- SEAM-CROSSING (query)      (0012, re-noted 0066)
--   analytics.charts.workspace_id                 -> public.workspaces  -- SEAM-CROSSING (iam/workspace)
--   analytics.charts.created_by                   -> public.users       -- SEAM-CROSSING (iam)
--   analytics.dashboards.workspace_id             -> public.workspaces  -- SEAM-CROSSING (iam/workspace)
--   analytics.dashboards.created_by               -> public.users       -- SEAM-CROSSING (iam)
--   analytics.folders.workspace_id                -> public.workspaces  -- SEAM-CROSSING (iam/workspace)
--   analytics.folders.created_by                  -> public.users       -- SEAM-CROSSING (iam)
--   analytics.bookmarks.user_id                   -> public.users       -- SEAM-CROSSING (iam)
--   analytics.saved_queries.workspace_id          -> public.workspaces  -- SEAM-CROSSING (iam/workspace)
--   analytics.saved_queries.created_by            -> public.users       -- SEAM-CROSSING (iam)
--   analytics.saved_queries.database_id           -> query.databases    -- SEAM-CROSSING (query)      (0010, re-noted 0066)
--   analytics.notebooks.workspace_id              -> public.workspaces  -- SEAM-CROSSING (iam/workspace)
--   analytics.notebooks.owner_user_id             -> public.users       -- SEAM-CROSSING (iam)
--   analytics.notebook_engine_access.workspace_id -> public.workspaces  -- SEAM-CROSSING (iam/workspace)
--   analytics.notebook_engine_access.database_id  -> query.databases    -- SEAM-CROSSING (query)      (0039, re-noted 0066)
--   kernel.kernels.user_id                        -> public.users       -- SEAM-CROSSING (iam)
--   kernel.kernels.workspace_id                   -> public.workspaces  -- SEAM-CROSSING (iam/workspace)
--   kernel.notebook_kernels.user_id               -> public.users       -- SEAM-CROSSING (iam)
--   kernel.notebook_kernels.notebook_id           -> analytics.notebooks -- SEAM-CROSSING (kernel -> analytics)  (NEW on this split)
-- In-seam FKs unchanged (moved with their tables):
--   analytics.dataset_columns.dataset_id   -> analytics.datasets
--   analytics.dataset_metrics.dataset_id   -> analytics.datasets
--   analytics.charts.dataset_id            -> analytics.datasets
--   analytics.charts.folder_id             -> analytics.folders
--   analytics.datasets.folder_id           -> analytics.folders
--   analytics.dashboards.folder_id         -> analytics.folders
--   analytics.notebooks.folder_id          -> analytics.folders
--   analytics.dashboard_cards.dashboard_id -> analytics.dashboards
--   analytics.dashboard_cards.chart_id     -> analytics.charts
--   analytics.dashboard_cards.tab_id       -> analytics.dashboard_tabs
--   analytics.dashboard_tabs.dashboard_id  -> analytics.dashboards
--   analytics.notebook_cells.notebook_id   -> analytics.notebooks
