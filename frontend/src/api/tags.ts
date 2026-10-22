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
import { type QueryClient, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { apiFetch, apiJson } from "./client";
import {
  getControlPlaneGovernanceTagHandwrittenQueryKey,
  getControlPlaneGovernanceTagUsageQueryKey,
} from "./generated/control-plane-governance/control-plane-governance";
import { getControlPlaneTagsListTagsQueryKey } from "./generated/control-plane-tags/control-plane-tags";
import { invalidateRuleEnforcement } from "@/routes/governance/ruleEnforcementCache";

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
  /**
   * Clearance group UUIDs required to open objects carrying this tag — a
   * principal is cleared if they hold a `required_roles` role OR belong to
   * one of these groups. Empty = not group-gated.
   */
  required_groups: string[];
  /**
   * Tables of egret catalogs carrying the tag on the table itself (#4b, spec
   * §4.4). Sent by `GET /control-plane/tags` (zero in a create or update
   * answer); absent from the workspace object-tag routes, which list BI tags.
   * Hand-written here because the generated `TagDto` is egret-bi's schema.
   */
  catalog_tables?: number;
  /** Tagged columns of egret catalogs; sent and absent as `catalog_tables`. */
  catalog_columns?: number;
}

export const tagsQueryKey = ["tags"] as const;

/**
 * CL16: what a write to tag `tagId` (or to where it is assigned) changes: the
 * one list key (`["tags"]`, its counts and clearance) and the tag's usage
 * (route C, Where used and the delete dialog).
 */
export function invalidateTag(queryClient: QueryClient, tagId: string) {
  void queryClient.invalidateQueries({ queryKey: tagsQueryKey });
  void queryClient.invalidateQueries({ queryKey: getControlPlaneGovernanceTagUsageQueryKey(tagId) });
}

/** Route A's keys, `/api/control-plane/governance/objects/<catalog>/handwritten` with any params. */
const OBJECT_HANDWRITTEN = /^\/api\/control-plane\/governance\/objects\/[^/]+\/handwritten$/;

/**
 * A Data tag assignment, a rename or a delete changes which hand-written tag
 * policies reach an object (route A matches them through the object's tags,
 * by name), on any catalog's pages: every route A answer is refetched.
 */
export function invalidateObjectHandwritten(queryClient: QueryClient) {
  void queryClient.invalidateQueries({
    predicate: (q) => typeof q.queryKey[0] === "string" && OBJECT_HANDWRITTEN.test(q.queryKey[0]),
  });
}

/** Route `GET /governance/tags/{catalog_id}` (an object's tags), any catalog and params. */
const OBJECT_TAGS = /^\/api\/control-plane\/governance\/tags\/[^/]+$/;

/**
 * What attaching tag `tagId` to, or removing it from, a table or column of
 * egret's warehouse `warehouseId` changes (`PUT`/`DELETE …/resource-tags`):
 * every object's tags (the Data header, the Overview's columns, the column
 * panel and the rules that select by classification read them), the
 * catalogue under both keys it is cached by (the generated `GET /tags` key
 * and `tagsQueryKey`: counts), the tag's Where used (CL16), the hand-written
 * sections (route A matches tag policies through the object's tags),
 * Control Plane's Browse tags for that warehouse, and every stored rule's
 * enforcement and effect (what a classification rule reaches).
 */
export function invalidateTagAssignment(queryClient: QueryClient, tagId: string, warehouseId: string) {
  void queryClient.invalidateQueries({
    predicate: (q) => typeof q.queryKey[0] === "string" && OBJECT_TAGS.test(q.queryKey[0]),
  });
  void queryClient.invalidateQueries({ queryKey: getControlPlaneTagsListTagsQueryKey() });
  invalidateTag(queryClient, tagId);
  invalidateObjectHandwritten(queryClient);
  void queryClient.invalidateQueries({ queryKey: ["control-plane", "warehouses", warehouseId, "resource-tags"] });
  // What a classification rule reaches, so whether it is enforced (`NotEnforcedBadge`).
  invalidateRuleEnforcement(queryClient);
}

/**
 * A rename or a delete changes what every chip says: each object's tags (Data's
 * `GET /governance/tags/{catalog}`, any catalog, and a BI object's tags) and the
 * catalogue under its generated key (`AddTagDialog`), besides `["tags"]`.
 */
function invalidateTagChips(queryClient: QueryClient) {
  void queryClient.invalidateQueries({
    predicate: (q) =>
      (typeof q.queryKey[0] === "string" && OBJECT_TAGS.test(q.queryKey[0])) ||
      (q.queryKey[0] === "ws" && q.queryKey[2] === "object-tags"),
  });
  void queryClient.invalidateQueries({ queryKey: getControlPlaneTagsListTagsQueryKey() });
}

/**
 * A rename or a delete of tag `tagId` changes what matches its name: route A
 * everywhere, the tag's own Hand-written tab (route D) and every chip.
 */
function invalidateTagName(queryClient: QueryClient, tagId: string) {
  invalidateObjectHandwritten(queryClient);
  invalidateTagChips(queryClient);
  void queryClient.invalidateQueries({ queryKey: getControlPlaneGovernanceTagHandwrittenQueryKey(tagId) });
}

export function objectTagsQueryKey(ws: string, kind: string, id: string) {
  return ["ws", ws, "object-tags", kind, id] as const;
}

// ── Catalog (control-plane) ─────────────────────────────────────────────────

/**
 * The catalogue (`GET /control-plane/tags`, `tag:read`). `enabled: false`
 * skips the read, for a caller that knows the viewer would be refused
 * (a 403 is not cached as data, so every mount would ask again).
 */
export function useTags(opts?: { enabled?: boolean }) {
  return useQuery<Tag[]>({
    queryKey: tagsQueryKey,
    queryFn: () => apiJson<Tag[]>("/control-plane/tags"),
    retry: false,
    enabled: opts?.enabled ?? true,
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
    onSuccess: (_data, { id }) => {
      invalidateTag(queryClient, id);
      invalidateTagName(queryClient, id);
    },
  });
}

export function useDeleteTag() {
  const queryClient = useQueryClient();
  return useMutation<void, Error, string>({
    mutationFn: async (id) => {
      // 204 with no body: read nothing (`apiJson` would fail parsing an empty body).
      await apiFetch(`/control-plane/tags/${id}`, { method: "DELETE" });
    },
    onSuccess: (_data, id) => {
      // The list changes; the deleted tag's own reads (its usage, its Manual
      // policies) are dropped, never refetched: they would only 404 (live result §7).
      void queryClient.invalidateQueries({ queryKey: tagsQueryKey });
      for (const queryKey of [getControlPlaneGovernanceTagUsageQueryKey(id), getControlPlaneGovernanceTagHandwrittenQueryKey(id)]) {
        void queryClient.cancelQueries({ queryKey });
        queryClient.removeQueries({ queryKey });
      }
      invalidateObjectHandwritten(queryClient);
      // Its chips go from every object, and the generated catalogue loses it.
      invalidateTagChips(queryClient);
    },
  });
}

/**
 * What a clearance write actually reached (SP4c review I5). `skipped` is empty
 * on the normal path; a non-empty entry means the recompile could NOT express
 * the clearance somewhere — a group-only gate, or a Trino profile with no
 * usable tag service — so the row was saved and no engine was given anything.
 * The endpoint used to answer a bare `{ ok: true }` either way.
 */
export interface AccessPolicyWritten {
  ok: boolean;
  skipped: { source_kind: string; id: string; reason: string }[];
}

/**
 * Sets the clearance policy for a tag: objects carrying it become openable
 * only by principals holding one of `required_roles` OR belonging to one of
 * `required_groups` (Cedar PDP-enforced). Passing both as empty arrays
 * clears the gate — same effect as `useClearTagAccessPolicy`, kept separate
 * for callers that always PUT.
 *
 * Resolves with the server's [`AccessPolicyWritten`]; a caller that discards it
 * is back to reporting "saved" over a clearance no engine enforces.
 */
export function useSetTagAccessPolicy() {
  const queryClient = useQueryClient();
  return useMutation<
    AccessPolicyWritten,
    Error,
    { id: string; required_roles: string[]; required_groups: string[] }
  >({
    mutationFn: ({ id, required_roles, required_groups }) =>
      apiJson<AccessPolicyWritten>(`/control-plane/tags/${id}/access-policy`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ required_roles, required_groups }),
      }),
    onSuccess: (_data, { id }) => invalidateTag(queryClient, id),
  });
}

/** Clears a tag's clearance policy — objects carrying it are no longer gated. */
export function useClearTagAccessPolicy() {
  const queryClient = useQueryClient();
  return useMutation<AccessPolicyWritten, Error, string>({
    mutationFn: (id) =>
      apiJson<AccessPolicyWritten>(`/control-plane/tags/${id}/access-policy`, { method: "DELETE" }),
    onSuccess: (_data, id) => invalidateTag(queryClient, id),
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
    onSuccess: (_data, tagId) => {
      queryClient.invalidateQueries({ queryKey: objectTagsQueryKey(ws, kind, id) });
      invalidateTag(queryClient, tagId);
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
    onSuccess: (_data, tagId) => {
      queryClient.invalidateQueries({ queryKey: objectTagsQueryKey(ws, kind, id) });
      invalidateTag(queryClient, tagId);
    },
  });
}
