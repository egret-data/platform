-- User hard-delete preserves content: attribution FKs → ON DELETE SET NULL.
-- notebooks.owner_user_id (NOT NULL) is reassigned in UserRepo::hard_delete, not here.

ALTER TABLE analytics.charts
  DROP CONSTRAINT charts_created_by_fkey,
  ADD CONSTRAINT charts_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE analytics.dashboards
  DROP CONSTRAINT dashboards_created_by_fkey,
  ADD CONSTRAINT dashboards_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE analytics.datasets
  DROP CONSTRAINT datasets_created_by_fkey,
  ADD CONSTRAINT datasets_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE analytics.folders
  DROP CONSTRAINT folders_created_by_fkey,
  ADD CONSTRAINT folders_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE analytics.saved_queries
  DROP CONSTRAINT saved_queries_created_by_fkey,
  ADD CONSTRAINT saved_queries_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE engine_mgmt.engine_workspaces
  DROP CONSTRAINT engine_workspaces_assigned_by_fkey,
  ADD CONSTRAINT engine_workspaces_assigned_by_fkey FOREIGN KEY (assigned_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE public.access_requests
  DROP CONSTRAINT access_requests_decided_by_fkey,
  ADD CONSTRAINT access_requests_decided_by_fkey FOREIGN KEY (decided_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE public.catalog_tag_assignments
  DROP CONSTRAINT catalog_tag_assignments_assigned_by_fkey,
  ADD CONSTRAINT catalog_tag_assignments_assigned_by_fkey FOREIGN KEY (assigned_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE public.classification_policies
  DROP CONSTRAINT classification_policies_created_by_fkey,
  ADD CONSTRAINT classification_policies_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE public.tag_assignments
  DROP CONSTRAINT tag_assignments_assigned_by_fkey,
  ADD CONSTRAINT tag_assignments_assigned_by_fkey FOREIGN KEY (assigned_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE public.tags
  DROP CONSTRAINT tags_created_by_fkey,
  ADD CONSTRAINT tags_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE public.workspaces
  DROP CONSTRAINT workspaces_created_by_fkey,
  ADD CONSTRAINT workspaces_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE query.databases
  DROP CONSTRAINT databases_created_by_fkey,
  ADD CONSTRAINT databases_created_by_fkey FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE query.queries
  DROP CONSTRAINT queries_user_id_fkey,
  ADD CONSTRAINT queries_user_id_fkey FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL;
