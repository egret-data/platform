/**
 * Notebook settings API — MIGRATED to the orval-generated client (E0·P4d-2).
 *
 * Thin adapter over `./generated/notebooks/notebooks` (the settings endpoints
 * live on the notebooks tag), following the P4c/P4d-1 pattern: hooks keep their
 * old names/signatures so call sites and tests don't churn; the read wraps the
 * generated query hook (unwrapping orval's `{ status, data, headers }` envelope
 * via `select`); the write runs `useMutation` over the generated fetch fn and
 * invalidates the GENERATED query key.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  getNotebooksGetSettingsQueryKey,
  notebooksUpdateSettings,
  useNotebooksGetSettings,
} from "./generated/notebooks/notebooks";

export interface EngineOption {
  id: string;
  name: string;
  engine: string; // "trino" | "starrocks"
  enabled: boolean;
}

export interface NotebookSettings {
  engines: EngineOption[];
}

export interface SaveNotebookSettingsInput {
  engine_ids: string[];
}

/** Back-compat query-key helper; delegates to the generated key factory. */
export const notebookSettingsKeys = {
  all: (ws: string) => getNotebooksGetSettingsQueryKey(ws),
};

export function useNotebookSettings(ws: string) {
  return useNotebooksGetSettings<NotebookSettings, ApiError>(ws, {
    query: { retry: false, select: (r) => r.data as NotebookSettings },
  });
}

export function useSaveNotebookSettings(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<NotebookSettings, ApiError, SaveNotebookSettingsInput>({
    mutationFn: async (body) =>
      (await notebooksUpdateSettings(ws, body)).data as NotebookSettings,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: getNotebooksGetSettingsQueryKey(ws) });
    },
  });
}
