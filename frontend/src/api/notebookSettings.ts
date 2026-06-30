import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, apiJson } from "./client";

export interface EngineOption {
  id: string;
  name: string;
  engine: string; // "trino" | "starrocks"
  enabled: boolean;
}

export interface CatalogOption {
  id: string; // warehouse id
  name: string;
  enabled: boolean;
}

export interface NotebookSettings {
  engines: EngineOption[];
  catalogs: CatalogOption[];
}

export interface SaveNotebookSettingsInput {
  engine_ids: string[];
  catalog_ids: string[];
}

export const notebookSettingsKeys = {
  all: (ws: string) => ["notebook-settings", ws] as const,
};

export function useNotebookSettings(ws: string) {
  return useQuery<NotebookSettings, ApiError>({
    queryKey: notebookSettingsKeys.all(ws),
    queryFn: () => apiJson<NotebookSettings>(`/ws/${ws}/notebook-settings`),
    retry: false,
  });
}

export function useSaveNotebookSettings(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<NotebookSettings, ApiError, SaveNotebookSettingsInput>({
    mutationFn: (body) =>
      apiJson<NotebookSettings>(`/ws/${ws}/notebook-settings`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: notebookSettingsKeys.all(ws) });
    },
  });
}
