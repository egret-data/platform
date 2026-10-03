/**
 * Data Apps API — a thin adapter over the orval-generated client (`./generated/apps/apps`), in the
 * same shape as `notebooks.ts`: hooks unwrap the generated `{ status, data, headers }` envelope and
 * writes invalidate the generated query keys.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  appsCreate,
  appsPublish,
  appsTicket,
  getAppsGetOneQueryKey,
  getAppsListQueryKey,
  useAppsGetOne,
  useAppsList,
} from "./generated/apps/apps";
import type { AppDetail, AppSummary, AppTicketMode, AppVersionSummary } from "./generated/model";

export type { AppDetail, AppSummary, AppVersionSummary };

export function useApps(ws: string) {
  return useAppsList<AppSummary[], ApiError>(ws, undefined, {
    query: { select: (r) => r.data as AppSummary[] },
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
