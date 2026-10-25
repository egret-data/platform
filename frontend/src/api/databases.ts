/**
 * Engine (database) connections API — MIGRATED to the orval-generated client
 * (E0·P4c reference module).
 *
 * This file is now a thin adapter over `./generated/databases/databases`:
 *   - Types are re-exported under the names the app already uses
 *     (`DatabaseConn` = `DatabaseDetailDto`, `DatabaseConnInput` = `DatabaseInput`).
 *   - Hooks keep their old names + signatures (`useDatabases(ws)`,
 *     `useCreateDatabase(ws)`, …) so call sites and tests don't churn.
 *
 * PATTERN FOR P4d
 * ---------------
 * The generated mutation hooks bake path params into the mutation *variables*
 * (`{ workspace, id, data }`), whereas this codebase currys `ws` (and `id`) at
 * hook-construction and passes only the payload as the variable. To preserve
 * that ergonomics across ~144 call sites we wrap:
 *   - READS  → the generated query hook (`useDatabasesList`) with a `select`
 *              that unwraps orval's `{ data, status, headers }` envelope.
 *   - WRITES → a plain `useMutation` over the generated fetch fn
 *              (`databasesCreate`, …), unwrapping `.data`, and invalidating the
 *              generated query key via `getDatabasesListQueryKey(ws)`.
 * Errors: the custom mutator (see `mutator.ts`) delegates to `apiFetch`, which
 * throws `ApiError`/`UnauthorizedError`; we type `TError = ApiError` so call
 * sites keep reading `.status` off the error exactly as before.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  databasesCreate,
  databasesRemove,
  databasesTestSaved,
  databasesUpdate,
  getDatabasesListQueryKey,
  useDatabasesEngineDetail,
  useDatabasesList,
} from "./generated/databases/databases";
import type {
  ConnOptions,
  DatabaseDetailDto,
  DatabaseInput,
  DatabaseListDto,
  Engine,
  TestOutcome,
  WsEngineCatalogDto,
  WsEngineDetailDto,
} from "./generated/model";

// Re-export generated types under the names the app already imports.
export type { ConnOptions, Engine, TestOutcome, WsEngineCatalogDto, WsEngineDetailDto };
export type DatabaseConn = DatabaseDetailDto;
export type DatabaseConnInput = DatabaseInput;
/**
 * One row of `GET /ws/{ws}/databases`. A workspace admin gets the full
 * connection (`DatabaseConn`); a member who may only run queries or read
 * datasets gets the redacted summary (`DatabaseListDto`: id, name, engine,
 * delegated — no host, port, options or secret).
 */
export type DatabaseListEntry = DatabaseConn | DatabaseListDto;

/** True for a row that carries the admin-only connection detail. */
export function isFullConnection(d: DatabaseListEntry): d is DatabaseConn {
  return "host" in d;
}

/** Back-compat query-key helper; delegates to the generated key factory. */
export const databaseKeys = {
  all: (ws: string) => getDatabasesListQueryKey(ws),
};

export function useDatabases(ws: string) {
  return useDatabasesList<DatabaseListEntry[], ApiError>(ws, {
    query: {
      // 403 (no query:run or dataset:read) is an expected outcome — surface it
      // immediately instead of retrying before the notice can render.
      retry: false,
      // Unwrap orval's `{ data, status, headers }` envelope to the list body.
      select: (r) => r.data as DatabaseListEntry[],
    },
  });
}

/**
 * Read-only view of an egret-provisioned engine assigned to the workspace:
 * its catalogs, each resolved to the storage/secret NAME it maps to (P2 B2,
 * `GET /ws/{ws}/engines/{engine_id}`). Used by the workspace Engines page to
 * expand a provisioned card. Unwraps orval's `{ data, status }` envelope.
 */
export function useWsEngineView(ws: string, engineId: string) {
  return useDatabasesEngineDetail<WsEngineDetailDto | undefined, ApiError>(ws, engineId, {
    query: {
      retry: false,
      select: (r) => (r.status === 200 ? (r.data as WsEngineDetailDto) : undefined),
    },
  });
}

export function useCreateDatabase(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<DatabaseConn, ApiError, DatabaseConnInput>({
    mutationFn: async (req) => (await databasesCreate(ws, req)).data as DatabaseConn,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: getDatabasesListQueryKey(ws) });
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
    mutationFn: async ({ id, input }) =>
      (await databasesUpdate(ws, id, input)).data as DatabaseConn,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: getDatabasesListQueryKey(ws) });
    },
  });
}

export function useDeleteDatabase(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await databasesRemove(ws, id);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: getDatabasesListQueryKey(ws) });
    },
  });
}

/**
 * The server stores a saved connection's test outcome as its `status`, so the
 * list is read again after every test: the Status column and the counts follow
 * the result (KI-011).
 */
export function useTestDatabase(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<TestOutcome, ApiError, string>({
    mutationFn: async (id) => (await databasesTestSaved(ws, id)).data as TestOutcome,
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: getDatabasesListQueryKey(ws) });
    },
  });
}
