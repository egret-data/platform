import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, apiFetch, apiJson } from "./client";

export interface WorkspaceSecret {
  id: string;
  name: string;
  description?: string | null;
  created_at: string;
  // Number of connections referencing this secret. 0 = safe to delete.
  used_by: number;
}

export interface SecretInput {
  name: string;
  description?: string | null;
  // Write-only — never returned by the API.
  value: string;
}

export interface SecretUpdate {
  name?: string;
  description?: string | null;
  // Write-only. Omit/blank to keep the existing value; a value rotates it.
  value?: string;
}

export const secretKeys = {
  all: (ws: string) => ["secrets", ws] as const,
};

export function useSecrets(ws: string) {
  return useQuery<WorkspaceSecret[], ApiError>({
    queryKey: secretKeys.all(ws),
    queryFn: () => apiJson<WorkspaceSecret[]>(`/ws/${ws}/secrets`),
    // 403 (non-admin) is an expected outcome — surface it immediately instead
    // of retrying 3× before the workspace-admin notice can render.
    retry: false,
  });
}

export function useCreateSecret(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<WorkspaceSecret, ApiError, SecretInput>({
    mutationFn: (req) =>
      apiJson<WorkspaceSecret>(`/ws/${ws}/secrets`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(req),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: secretKeys.all(ws) });
    },
  });
}

export interface UpdateSecretArgs {
  id: string;
  input: SecretUpdate;
}

export function useUpdateSecret(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<WorkspaceSecret, ApiError, UpdateSecretArgs>({
    mutationFn: ({ id, input }) =>
      apiJson<WorkspaceSecret>(`/ws/${ws}/secrets/${id}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: secretKeys.all(ws) });
    },
  });
}

export function useDeleteSecret(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await apiFetch(`/ws/${ws}/secrets/${id}`, { method: "DELETE" });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: secretKeys.all(ws) });
    },
  });
}
