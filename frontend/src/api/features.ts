import { useQueryClient } from "@tanstack/react-query";
import {
  getAdminWorkspacesWsFeaturesQueryKey,
  useAdminWorkspacesSetWsFeatures,
  useAdminWorkspacesWsFeatures,
} from "@/api/generated/admin-workspaces/admin-workspaces";

/**
 * Per-workspace module entitlements (`sql_lab` / `datasets` / `charts` /
 * `dashboards` / `apps`) — deliberately distinct from RBAC.
 *
 * RBAC (`ws_viewer`/`ws_editor`/`ws_admin`) decides what a *user* inside a
 * workspace may do. A feature switch decides what the workspace *is*: it
 * applies to EVERYONE in it, platform admins included, with no bypass — the
 * backend's `WsContext` extractor enforces this on every request, not just
 * the UI. A feature has no "read-only" state; read/write depth is still
 * RBAC's job.
 */
export function useWorkspaceFeaturesList(id: string) {
  const q = useAdminWorkspacesWsFeatures(id, { query: { retry: false } });
  const features = q.data?.status === 200 ? q.data.data : undefined;
  return { ...q, features };
}

/**
 * Full-replacement save of a workspace's feature switches. On success,
 * invalidates both this workspace's own feature list AND `["my-workspaces"]`:
 * the workspace-side Sidebar/route guard read `/api/me/workspaces`, not this
 * admin endpoint, so without the second invalidation a platform admin who is
 * also a member of the workspace would keep seeing a module they just
 * switched off until that cache happened to go stale on its own.
 */
export function useSetWorkspaceFeatures(id: string) {
  const qc = useQueryClient();
  return useAdminWorkspacesSetWsFeatures({
    mutation: {
      onSuccess: () => {
        qc.invalidateQueries({ queryKey: getAdminWorkspacesWsFeaturesQueryKey(id) });
        qc.invalidateQueries({ queryKey: ["my-workspaces"] });
      },
    },
  });
}
