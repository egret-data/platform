-- Bring Alerts & Reports to parity with the other Explore lists: folders, a
-- trash bin, and per-user bookmarks.
--
-- Bookmarks need no column here — `analytics.bookmarks` is keyed by
-- (user_id, entity_type, entity_id) with no CHECK on the type, so 'alert' is
-- already storable. Folders reuse `analytics.folders` the same way, via its
-- own `entity_type`.

ALTER TABLE automation.schedules
  ADD COLUMN folder_id  uuid REFERENCES analytics.folders(id) ON DELETE SET NULL,
  ADD COLUMN deleted_at timestamptz;

-- The list page reads one workspace's live rows, or one workspace's trash.
CREATE INDEX schedules_ws_live_idx
  ON automation.schedules (workspace_id, type)
  WHERE deleted_at IS NULL;

-- `schedules_due_idx` (0110) is the scheduler's claim index. It is replaced
-- rather than added to: a trashed schedule must never be claimed, so the
-- partial predicate has to carry that condition or the index stops matching
-- the WHERE clause and the claim falls back to a sequential scan.
DROP INDEX IF EXISTS automation.schedules_due_idx;
CREATE INDEX schedules_due_idx
  ON automation.schedules (next_run_at)
  WHERE active AND deleted_at IS NULL;

-- Reading alerts was gated only by workspace membership plus the `alerts`
-- entitlement, which left no permission for the generic folder router to check
-- (`kind_perms` in egret-bi wants a read AND a manage perm per kind). Every
-- other foldered entity has one — chart:read, dataset:read, notebook:read — so
-- alerts get theirs, granted to the three seeded workspace roles. Writes stay
-- on `alert:manage`, which only editors and admins hold.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'alert:read' FROM roles
 WHERE scope = 'workspace' AND name IN ('ws_viewer', 'ws_editor', 'ws_admin')
ON CONFLICT DO NOTHING;
