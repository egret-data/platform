import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export type EntityKind = "charts" | "dashboards" | "datasets";
export interface Folder { id: string; name: string }

const listKeyOf: Record<EntityKind, (ws: string) => readonly unknown[]> = {
  charts: (ws) => ["charts", ws],
  dashboards: (ws) => ["dashboards", ws],
  datasets: (ws) => ["datasets", ws, "list"],
};
export const folderKeys = { list: (ws: string, kind: EntityKind) => ["folders", ws, kind] as const };

export function useFolders(ws: string, kind: EntityKind) {
  return useQuery<Folder[]>({
    queryKey: folderKeys.list(ws, kind),
    queryFn: () => apiJson<Folder[]>(`/ws/${ws}/folders/${kind}`),
  });
}

export function useFolderMutations(ws: string, kind: EntityKind) {
  const qc = useQueryClient();
  const inval = () => qc.invalidateQueries({ queryKey: folderKeys.list(ws, kind) });
  const create = useMutation({
    mutationFn: (name: string) =>
      apiJson<Folder>(`/ws/${ws}/folders/${kind}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name }) }),
    onSuccess: inval,
  });
  const rename = useMutation({
    mutationFn: (a: { id: string; name: string }) =>
      apiJson<Folder>(`/ws/${ws}/folders/${kind}/${a.id}`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ name: a.name }) }),
    onSuccess: inval,
  });
  const remove = useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/folders/${kind}/${id}`, { method: "DELETE" }),
    onSuccess: inval,
  });
  return { create, rename, remove };
}

/** Move a resource into (folderId) or out of (null) a folder; invalidates that type's list. */
export function useSetFolder(ws: string, kind: EntityKind) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (a: { id: string; folder_id: string | null }) =>
      apiJson(`/ws/${ws}/${kind}/${a.id}/folder`, { method: "PATCH", headers: { "content-type": "application/json" }, body: JSON.stringify({ folder_id: a.folder_id }) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: listKeyOf[kind](ws) }),
  });
}
