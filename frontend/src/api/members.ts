/**
 * Workspace member API hooks (AX1).
 *
 * Thin react-query wrappers over the workspace-members endpoints:
 *   GET    /api/ws/{ws}/members                 — enriched member list
 *   POST   /api/ws/{ws}/members                 — add OR change role (replace)
 *   DELETE /api/ws/{ws}/members/{ptype}/{pid}   — remove a member
 *   GET    /api/ws/{ws}/invites                 — emails pre-registered for SSO
 *   DELETE /api/ws/{ws}/invites/{id}            — withdraw a pre-registration
 *
 * These call `apiJson`/`apiFetch` directly (not the orval client), so paths are
 * passed WITHOUT the `/api` prefix — `apiFetch` prepends it — and mutating
 * requests set `content-type` + a JSON body exactly like `useLogin` in
 * `./hooks`. Both mutation endpoints reply `200 { "ok": true }`, so `apiJson`
 * parses cleanly.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { apiJson } from "./client";
import { useWorkspaceRoleCatalog } from "./workspaceAccess";

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

/**
 * What adding a member did. `invited`: with SSO on, an email nobody has
 * signed in with yet is pre-registered instead, and the membership binds at
 * that user's first sign-in (KI-185).
 */
export interface AddMemberResult {
  ok: boolean;
  invited: boolean;
}

/** Add a member OR change an existing member's role (backend replaces). */
export function useAddMember(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { principal_type: string; principal_id: string; role: string }) =>
      apiJson<AddMemberResult>(`/ws/${ws}/members`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(b),
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: membersKey(ws) });
      void qc.invalidateQueries({ queryKey: invitesKey(ws) });
    },
  });
}

/** An email pre-registered for its first SSO sign-in. */
export interface WorkspaceInvite {
  id: string;
  email: string;
  role: string;
  invited_by_name: string | null;
  created_at: string;
}

export const invitesKey = (ws: string) => ["ws", ws, "invites"] as const;

export function useWorkspaceInvites(ws: string) {
  return useQuery<WorkspaceInvite[]>({
    queryKey: invitesKey(ws),
    queryFn: () => apiJson<WorkspaceInvite[]>(`/ws/${ws}/invites`),
    retry: false,
  });
}

export function useDeleteInvite(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiJson(`/ws/${ws}/invites/${id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: invitesKey(ws) }),
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

/**
 * Workspace roles offered when assigning a member: the workspace's own role
 * catalog, the list its Roles & permissions page shows (KI-019, KI-127). Not
 * the Control Plane role list, which needs `role:read` and carries
 * `data_api_invoker`, a role granted per endpoint rather than as a membership.
 */
export function useWorkspaceRoles(ws: string) {
  return useWorkspaceRoleCatalog(ws);
}
