import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export interface Me {
  user_id: string;
  email: string;
  platform_roles: string[];
  permissions: string[];
  /** Present + non-null while an admin is acting as another user (see
   * `ImpersonationBanner`); absent/null otherwise. */
  impersonating?: {
    target: { user_id: string; email: string };
    impersonator: { user_id: string; email: string };
    workspace_scope: string | null;
    expires_at: string;
  } | null;
}
/** `features` = the module keys (`sql_lab`/`datasets`/`charts`/`dashboards`/
 * `notebooks`) this workspace currently has switched on — drives both the
 * Sidebar's per-item visibility and `RequireFeature`'s route gate. */
export interface WorkspaceSummary { id: string; slug: string; name: string; features: string[]; }

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

/** Ends the current impersonation session, returning the admin to their own
 * account. Used by `ImpersonationBanner`'s Exit action. */
export function useExitImpersonation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => apiFetch("/auth/impersonation/exit", { method: "POST" }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["me"] });
      qc.invalidateQueries({ queryKey: ["my-workspaces"] });
    },
  });
}
