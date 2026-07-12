/**
 * Workspace member API hooks (AX1).
 *
 * Thin react-query wrappers over the workspace-members endpoints:
 *   GET    /api/ws/{ws}/members                 — enriched member list
 *   POST   /api/ws/{ws}/members                 — add OR change role (replace)
 *   DELETE /api/ws/{ws}/members/{ptype}/{pid}   — remove a member
 *
 * These call `apiJson`/`apiFetch` directly (not the orval client), so paths are
 * passed WITHOUT the `/api` prefix — `apiFetch` prepends it — and mutating
 * requests set `content-type` + a JSON body exactly like `useLogin` in
 * `./hooks`. Both mutation endpoints reply `200 { "ok": true }`, so `apiJson`
 * parses cleanly.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { listRoles, type AccessRole } from "./access";
import { apiJson } from "./client";

export interface WorkspaceMember {
  principal_type: "user" | "group";
  principal_id: string;
  role: string;
  principal_name: string | null;
  granted_by_name: string | null;
  added_at: string;
  is_platform_admin: boolean;
}

export const membersKey = (ws: string) => ["ws", ws, "members"] as const;

export function useWorkspaceMembers(ws: string) {
  return useQuery<WorkspaceMember[]>({
    queryKey: membersKey(ws),
    queryFn: () => apiJson<WorkspaceMember[]>(`/ws/${ws}/members`),
    retry: false,
  });
}

/** Add a member OR change an existing member's role (backend replaces). */
export function useAddMember(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { principal_type: string; principal_id: string; role: string }) =>
      apiJson(`/ws/${ws}/members`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(b),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: membersKey(ws) }),
  });
}

export function useRemoveMember(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (m: { principal_type: string; principal_id: string }) =>
      apiJson(`/ws/${ws}/members/${m.principal_type}/${m.principal_id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: membersKey(ws) }),
  });
}

/** Workspace-scoped roles offered when assigning a member (seeded + custom). */
export function useWorkspaceRoles() {
  return useQuery<AccessRole[]>({
    queryKey: ["ws", "roles", "workspace-scope"],
    queryFn: async () => (await listRoles()).filter((r) => r.scope === "workspace"),
    retry: false,
  });
}
