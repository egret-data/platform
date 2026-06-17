import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export interface Me { user_id: string; email: string; platform_roles: string[]; }
export interface WorkspaceSummary { id: string; slug: string; name: string; }

export function useMe() {
  return useQuery<Me | null>({
    queryKey: ["me"],
    queryFn: async () => {
      try { return await apiJson<Me>("/auth/me"); }
      catch { return null; } // UnauthorizedError → unauthenticated
    },
  });
}

export function useAuthConfig() {
  return useQuery({ queryKey: ["auth-config"], queryFn: () => apiJson<{ oidc_enabled: boolean }>("/auth/config") });
}

export function useMyWorkspaces(enabled: boolean) {
  return useQuery<WorkspaceSummary[]>({
    queryKey: ["my-workspaces"],
    queryFn: () => apiJson<WorkspaceSummary[]>("/me/workspaces"),
    enabled,
  });
}

export function useLogin() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { email: string; password: string }) =>
      apiFetch("/auth/login", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["me"] }),
  });
}

export function useLogout() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => apiFetch("/auth/logout", { method: "POST" }),
    onSuccess: () => qc.setQueryData(["me"], null),
  });
}
