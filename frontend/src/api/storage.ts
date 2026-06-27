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

export interface StorageObject {
  key: string;
  size: number;
  last_modified: string | null;
}

export interface ObjectListing {
  prefix: string;
  folders: string[];
  objects: StorageObject[];
}

export const storageKeys = {
  conns: (ws: string) => ["storage", ws, "conns"] as const,
  objects: (ws: string, connId: string, prefix: string) =>
    ["storage", ws, "conns", connId, "objects", prefix] as const,
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

// ── Object browser ───────────────────────────────────────────────────────────

export function useStorageObjects(ws: string, connId: string, prefix: string) {
  return useQuery<ObjectListing, ApiError>({
    queryKey: storageKeys.objects(ws, connId, prefix),
    queryFn: () =>
      apiJson<ObjectListing>(
        `/ws/${ws}/storages/${connId}/objects?prefix=${encodeURIComponent(prefix)}`,
      ),
    enabled: !!connId,
    // 403 (non-admin) is an expected outcome — surface it immediately.
    retry: false,
  });
}

export interface UploadObjectArgs {
  key: string;
  bytes: ArrayBuffer | Uint8Array;
}

export function useUploadObject(ws: string, connId: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, UploadObjectArgs>({
    mutationFn: async ({ key, bytes }) => {
      await apiFetch(
        `/ws/${ws}/storages/${connId}/objects?key=${encodeURIComponent(key)}`,
        {
          method: "POST",
          body: bytes as BodyInit,
          headers: { "Content-Type": "application/octet-stream" },
        },
      );
    },
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["storage", ws, "conns", connId, "objects"],
      });
    },
  });
}

export interface DeleteObjectArgs {
  key: string;
  recursive?: boolean;
}

export function useDeleteObject(ws: string, connId: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, DeleteObjectArgs>({
    mutationFn: async ({ key, recursive }) => {
      await apiFetch(
        `/ws/${ws}/storages/${connId}/objects?key=${encodeURIComponent(
          key,
        )}&recursive=${!!recursive}`,
        { method: "DELETE" },
      );
    },
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["storage", ws, "conns", connId, "objects"],
      });
    },
  });
}

export function useCreateFolder(ws: string, connId: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (path) => {
      await apiFetch(
        `/ws/${ws}/storages/${connId}/objects/folder?path=${encodeURIComponent(path)}`,
        { method: "POST" },
      );
    },
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: ["storage", ws, "conns", connId, "objects"],
      });
    },
  });
}

export interface MoveObjectArgs {
  from: string;
  to: string;
}

export function useMoveObject(ws: string, connId: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, MoveObjectArgs>({
    mutationFn: (args) =>
      apiJson<void>(`/ws/${ws}/storages/${connId}/objects/move`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(args),
      }),
    onSuccess: () => {
      // Broad invalidation so both the source and destination prefixes refetch.
      queryClient.invalidateQueries({
        queryKey: ["storage", ws, "conns", connId, "objects"],
      });
    },
  });
}

/** Fetch an object's bytes and trigger a browser download. */
export async function downloadObject(
  ws: string,
  connId: string,
  key: string,
): Promise<void> {
  const res = await apiFetch(
    `/ws/${ws}/storages/${connId}/objects/content?key=${encodeURIComponent(key)}`,
  );
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = key.split("/").pop() ?? key;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}
