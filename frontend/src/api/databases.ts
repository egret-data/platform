import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, apiFetch, apiJson } from "./client";

export type Engine = "trino" | "starrocks";

export interface ConnOptions {
  user?: string;
  http_scheme?: string;
  catalog?: string;
  database?: string;
}

export interface DatabaseConn {
  id: string;
  name: string;
  engine: Engine;
  host: string;
  port: number;
  options: ConnOptions;
  has_secret: boolean;
  status: string;
}

export interface DatabaseConnInput {
  name: string;
  engine: Engine;
  host: string;
  port: number;
  options: ConnOptions;
  // Reference an existing named secret, or create-and-reference a new one.
  // Both omitted on update keeps the current secret.
  secret_id?: string;
  new_secret?: { name: string; value: string };
}

export interface TestOutcome {
  ok: boolean;
  message?: string;
}

export const databaseKeys = {
  all: (ws: string) => ["databases", ws] as const,
};

export function useDatabases(ws: string) {
  return useQuery<DatabaseConn[], ApiError>({
    queryKey: databaseKeys.all(ws),
    queryFn: () => apiJson<DatabaseConn[]>(`/ws/${ws}/databases`),
    // 403 (non-admin) is an expected outcome — surface it immediately instead
    // of retrying 3× before the workspace-admin notice can render.
    retry: false,
  });
}

export function useCreateDatabase(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<DatabaseConn, ApiError, DatabaseConnInput>({
    mutationFn: (req) =>
      apiJson<DatabaseConn>(`/ws/${ws}/databases`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(req),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: databaseKeys.all(ws) });
    },
  });
}

export interface UpdateDatabaseArgs {
  id: string;
  input: DatabaseConnInput;
}

export function useUpdateDatabase(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<DatabaseConn, ApiError, UpdateDatabaseArgs>({
    mutationFn: ({ id, input }) =>
      apiJson<DatabaseConn>(`/ws/${ws}/databases/${id}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: databaseKeys.all(ws) });
    },
  });
}

export function useDeleteDatabase(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await apiFetch(`/ws/${ws}/databases/${id}`, { method: "DELETE" });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: databaseKeys.all(ws) });
    },
  });
}

export function useTestDatabase(ws: string) {
  return useMutation<TestOutcome, ApiError, string>({
    mutationFn: (id) =>
      apiJson<TestOutcome>(`/ws/${ws}/databases/${id}/test`, {
        method: "POST",
      }),
  });
}
