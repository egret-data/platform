import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, apiFetch, apiJson } from "./client";

export interface StorageConn {
  id: string;
  name: string;
  purpose: string | null;
  provider: string;
  endpoint: string | null;
  region: string;
  bucket: string;
  key_prefix: string | null;
  access_key: string;
  path_style: boolean;
  created_at: string;
  has_secret: boolean;
}

export interface StorageConnInput {
  name: string;
  purpose?: string | null;
  provider: string;
  endpoint?: string | null;
  region: string;
  bucket: string;
  key_prefix?: string | null;
  access_key: string;
  secret_key?: string | null;
  path_style: boolean;
}

export const storageKeys = {
  conns: (ws: string) => ["storage", ws, "conns"] as const,
};

export function useStorageConns(ws: string) {
  return useQuery<StorageConn[], ApiError>({
    queryKey: storageKeys.conns(ws),
    queryFn: () => apiJson<StorageConn[]>(`/ws/${ws}/storages`),
    // 403 (non-admin) is an expected outcome — surface it immediately instead
    // of retrying 3× before the workspace-admin notice can render.
    retry: false,
  });
}

export function useCreateStorageConn(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<StorageConn, ApiError, StorageConnInput>({
    mutationFn: (req) =>
      apiJson<StorageConn>(`/ws/${ws}/storages`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(req),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: storageKeys.conns(ws) });
    },
  });
}

export interface UpdateStorageConnArgs {
  id: string;
  input: StorageConnInput;
}

export function useUpdateStorageConn(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<StorageConn, ApiError, UpdateStorageConnArgs>({
    mutationFn: ({ id, input }) =>
      apiJson<StorageConn>(`/ws/${ws}/storages/${id}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: storageKeys.conns(ws) });
    },
  });
}

export function useDeleteStorageConn(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await apiFetch(`/ws/${ws}/storages/${id}`, { method: "DELETE" });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: storageKeys.conns(ws) });
    },
  });
}
