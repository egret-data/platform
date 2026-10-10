/**
 * Folders API — MIGRATED to the orval-generated client (E0·P4d-1).
 *
 * Thin adapter over `./generated/folders/folders` (list/create/rename/delete),
 * plus `useSetFolder`, which is generic across entity kinds and therefore
 * dispatches to each domain's generated `*SetFolder` fetch fn and invalidates
 * that domain's list key.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import { getChartsChListQueryKey, chartsChSetFolder } from "./generated/charts/charts";
import { getDashboardsDbListQueryKey, dashboardsDbSetFolder } from "./generated/dashboards/dashboards";
import { getDatasetsDsListQueryKey, datasetsDsSetFolder } from "./generated/datasets/datasets";
import {
  foldersFoldersCreate,
  foldersFoldersDelete,
  foldersFoldersRename,
  getFoldersFoldersListQueryKey,
  useFoldersFoldersList,
} from "./generated/folders/folders";
import type { FolderDto } from "./generated/model";
import { alertsSetFolder, getAlertsListQueryKey } from "./generated/alerts/alerts";
import { appsSetFolder, getAppsListQueryKey } from "./generated/apps/apps";
import { sqllabSqSetFolder } from "./generated/sqllab/sqllab";

export type EntityKind =
  | "charts"
  | "dashboards"
  | "datasets"
  | "saved_queries"
  | "alerts"
  | "apps";

/**
 * Every `{kind}` the GENERIC folder router accepts — `egret-bi`'s `kind_perms`
 * — which is a superset of [`EntityKind`].
 *
 * `EntityKind` is narrower on purpose: it is the key of the `listKeyOf` and
 * `setFolderOf` records below, so a kind may only join it once it has BOTH a
 * list query key and a `PATCH …/{id}/folder` fetch fn. Data API endpoints have
 * folders and no such route — an endpoint moves through the generic `PATCH
 * /endpoints/{id}` with `folder_id`, owned by `useUpdateEndpoint` in
 * `dataApi.ts` — so widening `EntityKind` would force a `setFolderOf` entry
 * that nothing calls, i.e. a second, dead way to move an endpoint sitting
 * beside the real one. Listing and creating folders need neither record, so
 * they take this wider type instead.
 */
export type FolderKind = EntityKind | "data_api_endpoints";
export type Folder = FolderDto;

/** Invalidation key for each entity's list; generated for every migrated domain. */
const listKeyOf: Record<EntityKind, (ws: string) => readonly unknown[]> = {
  charts: (ws) => getChartsChListQueryKey(ws),
  dashboards: (ws) => getDashboardsDbListQueryKey(ws),
  datasets: (ws) => getDatasetsDsListQueryKey(ws),
  saved_queries: (ws) => ["sqllab", ws, "saved"],
  // Prefix key: both list tabs (`{type: alert|report}`) invalidate together.
  alerts: (ws) => getAlertsListQueryKey(ws),
  // Prefix key: the live list and the trash (`{trashed: true}`) invalidate together.
  apps: (ws) => getAppsListQueryKey(ws),
};

/** `PATCH /ws/{ws}/{kind}/{id}/folder` via each domain's generated fetch fn. */
const setFolderOf: Record<EntityKind, (ws: string, id: string, folderId: string | null) => Promise<unknown>> = {
  charts: (ws, id, folder_id) => chartsChSetFolder(ws, id, { folder_id }),
  dashboards: (ws, id, folder_id) => dashboardsDbSetFolder(ws, id, { folder_id }),
  datasets: (ws, id, folder_id) => datasetsDsSetFolder(ws, id, { folder_id }),
  saved_queries: (ws, id, folder_id) => sqllabSqSetFolder(ws, id, { folder_id }),
  alerts: (ws, id, folder_id) => alertsSetFolder(ws, id, { folder_id }),
  apps: (ws, id, folder_id) => appsSetFolder(ws, id, { folder_id }),
};

export const folderKeys = { list: (ws: string, kind: FolderKind) => getFoldersFoldersListQueryKey(ws, kind) };

export function useFolders(ws: string, kind: FolderKind) {
  return useFoldersFoldersList<Folder[], ApiError>(ws, kind, {
    query: { select: (r) => r.data as Folder[] },
  });
}

export function useFolderMutations(ws: string, kind: FolderKind) {
  const qc = useQueryClient();
  const inval = () => qc.invalidateQueries({ queryKey: getFoldersFoldersListQueryKey(ws, kind) });
  const create = useMutation<Folder, ApiError, string>({
    mutationFn: async (name) => (await foldersFoldersCreate(ws, kind, { name })).data as Folder,
    onSuccess: inval,
  });
  const rename = useMutation<Folder, ApiError, { id: string; name: string }>({
    mutationFn: async (a) => (await foldersFoldersRename(ws, kind, a.id, { name: a.name })).data as Folder,
    onSuccess: inval,
  });
  const remove = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await foldersFoldersDelete(ws, kind, id); },
    onSuccess: inval,
  });
  return { create, rename, remove };
}

/** Move a resource into (folderId) or out of (null) a folder; invalidates that type's list. */
export function useSetFolder(ws: string, kind: EntityKind) {
  const qc = useQueryClient();
  return useMutation<unknown, ApiError, { id: string; folder_id: string | null }>({
    mutationFn: (a) => setFolderOf[kind](ws, a.id, a.folder_id),
    onSuccess: () => qc.invalidateQueries({ queryKey: listKeyOf[kind](ws) }),
  });
}
