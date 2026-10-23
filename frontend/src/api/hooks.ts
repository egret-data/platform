import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";
import { clearUserScopedCache } from "./queryClient";

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
/** `features` = the feature entitlement keys this workspace currently has
 * switched on (a subset of whatever the catalog defines) — drives both the
 * Sidebar's per-item visibility and `RequireFeature`'s route gate. */
export interface WorkspaceSummary {
  id: string;
  slug: string;
  name: string;
  features: string[];
  /**
   * The caller's own page-gating permissions in this workspace (the server's
   * `PAGE_PERMISSIONS`: `data_api_endpoint:read`, `service_account:read`,
   * `workflow:read`, `workspace:manage_members`), as the PDP answers them.
   * Absent from a server that predates it; read through
   * `useWorkspacePermission`, which treats absent as "unknown, show it".
   */
  permissions?: string[];
}

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
    // Clear the previous identity's cache (e.g. `["my-workspaces"]`) before
    // refetching `me` — and await the refetch, since `LoginPage` navigates
    // right after `await login.mutateAsync(...)` and must see the new
    // identity's data, not whatever an unawaited invalidate left behind.
    onSuccess: async () => {
      clearUserScopedCache(qc);
      await qc.invalidateQueries({ queryKey: ["me"] });
    },
  });
}

export function useLogout() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => apiFetch("/auth/logout", { method: "POST" }),
    onSuccess: () => {
      // No network round-trip needed to know we're unauthenticated, but the
      // previous user's other cached queries (e.g. `["my-workspaces"]`) must
      // still go — otherwise the next login in this tab can render off them.
      clearUserScopedCache(qc);
      qc.setQueryData(["me"], null);
    },
  });
}

/** Ends the current impersonation session, returning the admin to their own
 * account. Used by `ImpersonationBanner`'s Exit action. */
export function useExitImpersonation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => apiFetch("/auth/impersonation/exit", { method: "POST" }),
    onSuccess: async () => {
      clearUserScopedCache(qc);
      await qc.invalidateQueries({ queryKey: ["me"] });
    },
  });
}
