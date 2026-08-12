/**
 * Folders API — MIGRATED to the orval-generated client (E0·P4d-1).
 *
 * Thin adapter over `./generated/folders/folders` (list/create/rename/delete),
 * plus `useSetFolder`, which is generic across entity kinds and therefore
 * dispatches to each domain's generated `*SetFolder` fetch fn and invalidates
 * that domain's list key. Charts/dashboards/datasets are migrated to generated
 * keys; notebooks is still on its hand-written key (migrated in a later P4d
 * batch), so its branch keeps the legacy `["notebooks", ws]` invalidation.
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
import { notebooksSetFolder } from "./generated/notebooks/notebooks";
import { sqllabSqSetFolder } from "./generated/sqllab/sqllab";

export type EntityKind =
  | "charts"
  | "dashboards"
  | "datasets"
  | "notebooks"
  | "saved_queries"
  | "alerts";
export type Folder = FolderDto;

/** Invalidation key for each entity's list; generated for migrated domains,
 *  legacy for notebooks (not yet on the generated client). */
const listKeyOf: Record<EntityKind, (ws: string) => readonly unknown[]> = {
  charts: (ws) => getChartsChListQueryKey(ws),
  dashboards: (ws) => getDashboardsDbListQueryKey(ws),
  datasets: (ws) => getDatasetsDsListQueryKey(ws),
  notebooks: (ws) => ["notebooks", ws],
  saved_queries: (ws) => ["sqllab", ws, "saved"],
  // Prefix key: both list tabs (`{type: alert|report}`) invalidate together.
  alerts: (ws) => getAlertsListQueryKey(ws),
};

/** `PATCH /ws/{ws}/{kind}/{id}/folder` via each domain's generated fetch fn. */
const setFolderOf: Record<EntityKind, (ws: string, id: string, folderId: string | null) => Promise<unknown>> = {
  charts: (ws, id, folder_id) => chartsChSetFolder(ws, id, { folder_id }),
  dashboards: (ws, id, folder_id) => dashboardsDbSetFolder(ws, id, { folder_id }),
  datasets: (ws, id, folder_id) => datasetsDsSetFolder(ws, id, { folder_id }),
  notebooks: (ws, id, folder_id) => notebooksSetFolder(ws, id, { folder_id }),
  saved_queries: (ws, id, folder_id) => sqllabSqSetFolder(ws, id, { folder_id }),
  alerts: (ws, id, folder_id) => alertsSetFolder(ws, id, { folder_id }),
};

export const folderKeys = { list: (ws: string, kind: EntityKind) => getFoldersFoldersListQueryKey(ws, kind) };

export function useFolders(ws: string, kind: EntityKind) {
  return useFoldersFoldersList<Folder[], ApiError>(ws, kind, {
    query: { select: (r) => r.data as Folder[] },
  });
}

export function useFolderMutations(ws: string, kind: EntityKind) {
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
