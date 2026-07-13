/**
 * Workspace access API hooks (role catalog + object-share rollup).
 *
 * Thin react-query wrappers over the workspace-access endpoints:
 *   GET /api/ws/{ws}/roles  — role catalog (role + parent + permissions)
 *   GET /api/ws/{ws}/shares — object-grant rollup across shared objects
 *
 * These call `apiJson` directly (not the orval client), so paths are passed
 * WITHOUT the `/api` prefix — `apiJson`/`apiFetch` prepend it — mirroring
 * `useWorkspaceMembers` in `./members.ts`.
 */
import { useQuery } from "@tanstack/react-query";

import { apiJson } from "./client";

export interface WsPerm {
  key: string;
  description: string;
  group: string;
}

export interface WsRole {
  id: string;
  name: string;
  description: string;
  parent_role_id: string | null;
  permissions: WsPerm[];
}

export interface ObjectGrantRollup {
  kind: string;
  object_id: string;
  object_name: string | null;
  principal_type: "user" | "group";
  principal_id: string;
  principal_name: string | null;
  role: string;
  effect: string;
  expires_at: string | null;
}

export function useWorkspaceRoleCatalog(ws: string) {
  return useQuery<WsRole[]>({
    queryKey: ["ws", ws, "role-catalog"],
    queryFn: () => apiJson<WsRole[]>(`/ws/${ws}/roles`),
    retry: false,
  });
}

export function useWorkspaceShares(ws: string) {
  return useQuery<ObjectGrantRollup[]>({
    queryKey: ["ws", ws, "shares"],
    queryFn: () => apiJson<ObjectGrantRollup[]>(`/ws/${ws}/shares`),
    retry: false,
  });
}
