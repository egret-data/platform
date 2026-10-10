/**
 * Data Apps API — a thin adapter over the orval-generated client (`./generated/apps/apps`), in the
 * same shape as `notebooks.ts`: hooks unwrap the generated `{ status, data, headers }` envelope and
 * writes invalidate the generated query keys.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  appsBookmarkAdd,
  appsBookmarkRemove,
  appsCreate,
  appsDeleteApp,
  appsDuplicate,
  appsPublish,
  appsRestore,
  appsTicket,
  appsTrash,
  appsUpdate,
  appsVersionRestore,
  getAppsGetOneQueryKey,
  getAppsListQueryKey,
  getAppsVersionsQueryKey,
  useAppsCapabilities,
  useAppsGetOne,
  useAppsList,
  useAppsVersions,
} from "./generated/apps/apps";
import type { AppCapabilities, AppDetail, AppSummary, AppTicketMode, AppVersionSummary } from "./generated/model";

export type { AppCapabilities, AppDetail, AppSummary, AppVersionSummary };

/** The live list, or the trash bin with `{ trashed: true }` — two server queries, not a filter. */
export function useApps(ws: string, opts?: { trashed?: boolean }) {
  const trashed = !!opts?.trashed;
  return useAppsList<AppSummary[], ApiError>(ws, trashed ? { trashed: true } : undefined, {
    query: { select: (r) => r.data as AppSummary[] },
  });
}

/**
 * The list page's writes, in the shape of `useDashboardMutations`.
 *
 * Every one invalidates `getAppsListQueryKey(ws)` with NO params: that key is a prefix of the
 * trashed list's (`[url]` vs `[url, { trashed: true }]`), so a trash or restore refreshes the
 * view it left and the view it arrived in with one call.
 */
export function useAppMutations(ws: string) {
  const qc = useQueryClient();
  const invalidate = (id?: string) => {
    qc.invalidateQueries({ queryKey: getAppsListQueryKey(ws) });
    if (id) qc.invalidateQueries({ queryKey: getAppsGetOneQueryKey(ws, id) });
  };

  /** Rename / re-describe. A key left out is left unchanged by the server (`PATCH`). */
  const update = useMutation<AppDetail, ApiError, { id: string; name?: string; description?: string }>({
    mutationFn: async ({ id, ...b }) => (await appsUpdate(ws, id, b)).data as AppDetail,
    onSuccess: (d) => invalidate(d.id),
  });
  const trash = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await appsTrash(ws, id); },
    onSuccess: () => invalidate(),
  });
  const restore = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await appsRestore(ws, id); },
    onSuccess: () => invalidate(),
  });
  /** Permanent delete. The server refuses (409) unless the app is already in the trash. */
  const remove = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await appsDeleteApp(ws, id); },
    onSuccess: () => invalidate(),
  });
  /** Copies the source's DRAFT into a new app the caller owns, and returns that new app. */
  const duplicate = useMutation<AppDetail, ApiError, string>({
    mutationFn: async (id) => (await appsDuplicate(ws, id)).data as AppDetail,
    onSuccess: () => invalidate(),
  });
  const addBookmark = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await appsBookmarkAdd(ws, id); },
    onSuccess: () => invalidate(),
  });
  const removeBookmark = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await appsBookmarkRemove(ws, id); },
    onSuccess: () => invalidate(),
  });
  return { update, trash, restore, remove, duplicate, addBookmark, removeBookmark };
}

/**
 * What the caller may do with apps in this workspace that no row can answer: create one, and
 * create / rename / delete an app folder (the backend gates both on workspace `app:write`).
 * Per-app permissions are on each row (`can_write`, `can_manage`).
 */
export function useAppCapabilities(ws: string) {
  return useAppsCapabilities<AppCapabilities, ApiError>(ws, {
    query: { select: (r) => r.data as AppCapabilities },
  });
}

/** One app; `refetchMs` keeps the draft revision shown next to Publish honest while an author edits. */
export function useApp(ws: string, id: string, opts?: { refetchMs?: number }) {
  return useAppsGetOne<AppDetail, ApiError>(ws, id, {
    query: {
      select: (r) => r.data as AppDetail,
      refetchInterval: opts?.refetchMs,
    },
  });
}

export function useCreateApp(ws: string) {
  const qc = useQueryClient();
  return useMutation<AppDetail, ApiError, { name: string; description?: string }>({
    mutationFn: async (b) => (await appsCreate(ws, b)).data as AppDetail,
    onSuccess: () => qc.invalidateQueries({ queryKey: getAppsListQueryKey(ws) }),
  });
}

/** Publish the draft as a new version. `expectedRevision` makes it "publish rev N or fail" (409). */
export function usePublishApp(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<AppVersionSummary, ApiError, { expectedRevision?: number }>({
    mutationFn: async ({ expectedRevision }) =>
      (await appsPublish(ws, id, expectedRevision == null ? null : { expected_revision: expectedRevision }))
        .data as AppVersionSummary,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: getAppsGetOneQueryKey(ws, id) });
      qc.invalidateQueries({ queryKey: getAppsListQueryKey(ws) });
    },
  });
}

/** An app's published versions, newest first (the backend orders them; see `AppStore::versions`). */
export function useAppVersions(ws: string, id: string) {
  return useAppsVersions<AppVersionSummary[], ApiError>(ws, id, {
    query: { select: (r) => r.data as AppVersionSummary[] },
  });
}

/** Restoring a past version publishes a NEW version from its source — history stays linear, no
 * pointer moves. Invalidates both the version list and the app (its `latest_version_no` changes). */
export function useRestoreVersion(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<AppVersionSummary, ApiError, { versionId: string }>({
    mutationFn: async ({ versionId }) => (await appsVersionRestore(ws, id, versionId)).data as AppVersionSummary,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: getAppsVersionsQueryKey(ws, id) });
      qc.invalidateQueries({ queryKey: getAppsGetOneQueryKey(ws, id) });
      qc.invalidateQueries({ queryKey: getAppsListQueryKey(ws) });
    },
  });
}

/**
 * A one-time URL for the apps-origin iframe (spec §6.1). It expires in seconds and is spent by the
 * first request that follows it: never cache it, never prefetch it, never reuse it.
 */
export async function requestAppTicket(
  ws: string,
  id: string,
  mode: AppTicketMode,
  versionId?: string,
): Promise<string> {
  const r = await appsTicket(ws, id, { mode, version_id: versionId ?? null });
  return (r.data as { url: string }).url;
}
