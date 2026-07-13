/**
 * Classification tags API hooks.
 *
 * Two surfaces:
 *   - Catalog (control-plane): the org-wide tag definitions
 *     (`GET/POST /control-plane/tags`, `PATCH/DELETE /control-plane/tags/{id}`).
 *   - Object (workspace): assigning/unassigning a tag on a specific object
 *     (`GET/PUT/DELETE /ws/{ws}/objects/{kind}/{id}/tags[/{tag_id}]`), used by
 *     a later task's TagEditor.
 *
 * These call `apiJson` directly (not the orval client), so paths are passed
 * WITHOUT the `/api` prefix — mirroring `useWorkspaceRoleCatalog` in
 * `./workspaceAccess.ts`.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { apiJson } from "./client";

export interface Tag {
  id: string;
  key: string;
  name: string;
  description: string;
  color: string;
  assignment_count: number;
  created_at: string;
  /**
   * Clearance role names required to open objects carrying this tag,
   * enforced in the Cedar PDP. Empty = not gated (no restriction beyond the
   * object's own access grants).
   */
  required_roles: string[];
}

export const tagsQueryKey = ["tags"] as const;

export function objectTagsQueryKey(ws: string, kind: string, id: string) {
  return ["ws", ws, "object-tags", kind, id] as const;
}

// ── Catalog (control-plane) ─────────────────────────────────────────────────

export function useTags() {
  return useQuery<Tag[]>({
    queryKey: tagsQueryKey,
    queryFn: () => apiJson<Tag[]>("/control-plane/tags"),
    retry: false,
  });
}

export function useCreateTag() {
  const queryClient = useQueryClient();
  return useMutation<Tag, Error, { key: string; name: string; description?: string; color?: string }>({
    mutationFn: (input) =>
      apiJson<Tag>("/control-plane/tags", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
      }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: tagsQueryKey }),
  });
}

export function useUpdateTag() {
  const queryClient = useQueryClient();
  return useMutation<
    Tag,
    Error,
    { id: string; name?: string; description?: string; color?: string }
  >({
    mutationFn: ({ id, ...input }) =>
      apiJson<Tag>(`/control-plane/tags/${id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
      }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: tagsQueryKey }),
  });
}

export function useDeleteTag() {
  const queryClient = useQueryClient();
  return useMutation<void, Error, string>({
    mutationFn: async (id) => {
      await apiJson<void>(`/control-plane/tags/${id}`, { method: "DELETE" });
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: tagsQueryKey }),
  });
}

/**
 * Sets the clearance policy for a tag: objects carrying it become openable
 * only by principals holding one of `required_roles` (Cedar PDP-enforced).
 * Passing an empty array clears the gate — same effect as
 * `useClearTagAccessPolicy`, kept separate for callers that always PUT.
 */
export function useSetTagAccessPolicy() {
  const queryClient = useQueryClient();
  return useMutation<void, Error, { id: string; required_roles: string[] }>({
    mutationFn: async ({ id, required_roles }) => {
      await apiJson<void>(`/control-plane/tags/${id}/access-policy`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ required_roles }),
      });
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: tagsQueryKey }),
  });
}

/** Clears a tag's clearance policy — objects carrying it are no longer gated. */
export function useClearTagAccessPolicy() {
  const queryClient = useQueryClient();
  return useMutation<void, Error, string>({
    mutationFn: async (id) => {
      await apiJson<void>(`/control-plane/tags/${id}/access-policy`, { method: "DELETE" });
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: tagsQueryKey }),
  });
}

// ── Object (workspace) ──────────────────────────────────────────────────────

export function useObjectTags(ws: string, kind: string | undefined, id: string | undefined) {
  return useQuery<Tag[]>({
    queryKey: objectTagsQueryKey(ws, kind ?? "", id ?? ""),
    queryFn: () => apiJson<Tag[]>(`/ws/${ws}/objects/${kind}/${id}/tags`),
    enabled: Boolean(kind) && Boolean(id),
    retry: false,
  });
}

export function useAssignTag(ws: string, kind: string, id: string) {
  const queryClient = useQueryClient();
  return useMutation<{ ok: true }, Error, string>({
    mutationFn: (tagId) =>
      apiJson<{ ok: true }>(`/ws/${ws}/objects/${kind}/${id}/tags/${tagId}`, {
        method: "PUT",
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: objectTagsQueryKey(ws, kind, id) });
      queryClient.invalidateQueries({ queryKey: tagsQueryKey });
    },
  });
}

export function useUnassignTag(ws: string, kind: string, id: string) {
  const queryClient = useQueryClient();
  return useMutation<{ ok: true }, Error, string>({
    mutationFn: (tagId) =>
      apiJson<{ ok: true }>(`/ws/${ws}/objects/${kind}/${id}/tags/${tagId}`, {
        method: "DELETE",
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: objectTagsQueryKey(ws, kind, id) });
      queryClient.invalidateQueries({ queryKey: tagsQueryKey });
    },
  });
}
