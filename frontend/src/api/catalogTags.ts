/**
 * Catalog resource tags — tagging a warehouse's tables/columns from the
 * catalog browse (control-plane).
 *
 * `GET /control-plane/warehouses/{warehouse_id}/resource-tags?schema=&table=`
 * returns every tag assignment for that table AND its columns in one call;
 * `column: ""` denotes the table-level assignment. Assign/unassign are
 * `PUT`/`DELETE` on the same collection endpoint with the resource
 * coordinates + `tag_id` in the body.
 *
 * Calls `apiJson` directly (not the orval client), so paths are passed
 * WITHOUT the `/api` prefix — mirroring `./tags.ts`.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { apiJson } from "./client";
import { invalidateObjectHandwritten, invalidateTag, type Tag } from "./tags";

export interface ResourceTagAssignment {
  schema: string;
  table: string;
  column: string;
  tag: Tag;
}

export interface ResourceTagInput {
  schema: string;
  table: string;
  column?: string;
  tag_id: string;
}

export function resourceTagsQueryKey(warehouseId: string, schema: string, table: string) {
  return ["control-plane", "warehouses", warehouseId, "resource-tags", schema, table] as const;
}

export function useResourceTags(warehouseId: string, schema: string, table: string) {
  return useQuery<ResourceTagAssignment[]>({
    queryKey: resourceTagsQueryKey(warehouseId, schema, table),
    queryFn: () =>
      apiJson<ResourceTagAssignment[]>(
        `/control-plane/warehouses/${warehouseId}/resource-tags?schema=${encodeURIComponent(schema)}&table=${encodeURIComponent(table)}`,
      ),
    enabled: Boolean(warehouseId) && Boolean(schema) && Boolean(table),
    retry: false,
  });
}

export function useAssignResourceTag(warehouseId: string) {
  const queryClient = useQueryClient();
  return useMutation<void, Error, ResourceTagInput>({
    mutationFn: async (input) => {
      await apiJson<void>(`/control-plane/warehouses/${warehouseId}/resource-tags`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
      });
    },
    onSuccess: (_data, input) => {
      void queryClient.invalidateQueries({
        queryKey: resourceTagsQueryKey(warehouseId, input.schema, input.table),
      });
      // CL16: the list's Data counts and the tag's usage change too, and the
      // hand-written tag policies reaching objects (route A).
      invalidateTag(queryClient, input.tag_id);
      invalidateObjectHandwritten(queryClient);
    },
  });
}

export function useUnassignResourceTag(warehouseId: string) {
  const queryClient = useQueryClient();
  return useMutation<void, Error, ResourceTagInput>({
    mutationFn: async (input) => {
      await apiJson<void>(`/control-plane/warehouses/${warehouseId}/resource-tags`, {
        method: "DELETE",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
      });
    },
    onSuccess: (_data, input) => {
      void queryClient.invalidateQueries({
        queryKey: resourceTagsQueryKey(warehouseId, input.schema, input.table),
      });
      // CL16: the list's Data counts and the tag's usage change too, and the
      // hand-written tag policies reaching objects (route A).
      invalidateTag(queryClient, input.tag_id);
      invalidateObjectHandwritten(queryClient);
    },
  });
}
