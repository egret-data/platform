import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  getDataApiGetEndpointQueryKey,
  getDataApiListEndpointVersionsQueryKey,
  getDataApiListEndpointsQueryKey,
  getDataApiListServiceAccountsQueryKey,
  useDataApiBookmarkEndpoint,
  useDataApiCreateEndpoint,
  useDataApiCreateServiceAccount,
  useDataApiDeleteEndpoint,
  useDataApiDeleteServiceAccount,
  useDataApiDisableEndpoint,
  useDataApiEnableEndpoint,
  useDataApiPublishEndpoint,
  useDataApiRollbackEndpoint,
  useDataApiUnbookmarkEndpoint,
  useDataApiUpdateEndpoint,
} from "./generated/data-api/data-api";

/**
 * Data API mutations, with their cache invalidation attached.
 *
 * # Why this module exists
 *
 * The orval-generated hooks in `generated/data-api/data-api.ts` carry no
 * `onSuccess` — there is not one `invalidate` in the generated file — while
 * `queryClient.ts` sets `staleTime: 30000`. A mutation wired straight to a
 * generated hook therefore toasts "Endpoint disabled" over a list that keeps
 * showing **Live** for the next 30 seconds, and a just-created row simply is
 * not there when the author navigates back. That is not a bug in one call
 * site: it is the default every call site inherits.
 *
 * So invalidation lives HERE, next to the mutation, exactly the way the twenty
 * other hand-written `src/api/*.ts` modules do it (`alerts.ts`'s
 * `useScheduleMutations` is the closest sibling). A call site imports the
 * wrapper instead of the generated hook and gets the behaviour for free; the
 * next person to add a Data API mutation adds it here and inherits it too,
 * rather than rediscovering the problem from a stale badge.
 *
 * # Shape
 *
 * Each wrapper is the *generated* hook with an `onSuccess` injected, not a
 * hand-rolled `useMutation` over the raw request function (which is what
 * `alerts.ts` does). Two reasons:
 *
 *  - the mutation variables stay exactly what the generated hook declares
 *    (`{workspace, id, data}`), so the response union — and the `res.status`
 *    narrowing every call site does on it — is unchanged; and
 *  - the workspace is read from those **variables**, not from a hook argument,
 *    so the key invalidated is by construction the workspace the write went
 *    to. There is no second source of truth to drift.
 *
 * # What is NOT here
 *
 * `issue`/`revoke client` stay local to `Clients.tsx`. Their list key is
 * `getDataApiListClientsQueryKey(ws, serviceAccountId)` and `revoke`'s
 * variables carry the CLIENT id, not the owning account's — so the key cannot
 * be derived from the variables the way every mutation below derives it, and
 * a wrapper would have to take the account id as an argument, re-introducing
 * the second source of truth this module avoids. `ClientsCell` already owns
 * that invalidation correctly against the account it is rendered for.
 */

/** Invalidates the endpoint list for one workspace. */
function useInvalidateEndpointList() {
  const qc = useQueryClient();
  return useCallback(
    (workspace: string) => {
      void qc.invalidateQueries({ queryKey: getDataApiListEndpointsQueryKey(workspace) });
    },
    [qc],
  );
}

/** Invalidates one endpoint's own record plus the list it appears in. */
function useInvalidateEndpoint() {
  const qc = useQueryClient();
  return useCallback(
    (workspace: string, id: string) => {
      void qc.invalidateQueries({ queryKey: getDataApiGetEndpointQueryKey(workspace, id) });
      void qc.invalidateQueries({ queryKey: getDataApiListEndpointsQueryKey(workspace) });
    },
    [qc],
  );
}

/** Invalidates the service-account list for one workspace. */
function useInvalidateServiceAccounts() {
  const qc = useQueryClient();
  return useCallback(
    (workspace: string) => {
      void qc.invalidateQueries({ queryKey: getDataApiListServiceAccountsQueryKey(workspace) });
    },
    [qc],
  );
}

/** Create — the new row has to be in the list the author navigates back to. */
export function useCreateEndpoint() {
  const invalidateList = useInvalidateEndpointList();
  return useDataApiCreateEndpoint({
    mutation: { onSuccess: (_res, { workspace }) => invalidateList(workspace) },
  });
}

/** Delete — the deleted row has to leave the list it was deleted from. */
export function useDeleteEndpoint() {
  const invalidateList = useInvalidateEndpointList();
  return useDataApiDeleteEndpoint({
    mutation: { onSuccess: (_res, { workspace }) => invalidateList(workspace) },
  });
}

/** Disable — moves `state` to `disabled`, which the list renders as a badge. */
export function useDisableEndpoint() {
  const invalidateEndpoint = useInvalidateEndpoint();
  return useDataApiDisableEndpoint({
    mutation: { onSuccess: (_res, { workspace, id }) => invalidateEndpoint(workspace, id) },
  });
}

/** Enable — moves `state` back to `live`, same badge. */
export function useEnableEndpoint() {
  const invalidateEndpoint = useInvalidateEndpoint();
  return useDataApiEnableEndpoint({
    mutation: { onSuccess: (_res, { workspace, id }) => invalidateEndpoint(workspace, id) },
  });
}

/** Save — a rename shows in the detail header AND in the list's Name column. */
export function useUpdateEndpoint() {
  const invalidateEndpoint = useInvalidateEndpoint();
  return useDataApiUpdateEndpoint({
    mutation: { onSuccess: (_res, { workspace, id }) => invalidateEndpoint(workspace, id) },
  });
}

/**
 * Bookmark / unbookmark.
 *
 * The star is rendered from `bookmarked` on the LIST row
 * (`DataApiEndpointListItemDto`), which is the only response that carries the
 * flag — `GET /endpoints/{id}` does not — so the list is the one query that
 * has to be refetched, and invalidating the endpoint RECORD here would refetch
 * something that cannot answer the question the star asks.
 *
 * Note these are also the only two Data API mutations whose variables are
 * `{workspace, id}` with no `data`; the workspace still comes off the
 * variables, so a star clicked in one workspace can never invalidate another's
 * list.
 */
export function useBookmarkEndpoint() {
  const invalidateList = useInvalidateEndpointList();
  return useDataApiBookmarkEndpoint({
    mutation: { onSuccess: (_res, { workspace }) => invalidateList(workspace) },
  });
}

export function useUnbookmarkEndpoint() {
  const invalidateList = useInvalidateEndpointList();
  return useDataApiUnbookmarkEndpoint({
    mutation: { onSuccess: (_res, { workspace }) => invalidateList(workspace) },
  });
}

/**
 * Publish/rollback both move `state` and `live_version_id` server-side, so
 * they invalidate the endpoint record and the list badge as well as the
 * version history the Versions section renders.
 */
function useInvalidateVersions() {
  const qc = useQueryClient();
  const invalidateEndpoint = useInvalidateEndpoint();
  return useCallback(
    (workspace: string, id: string) => {
      invalidateEndpoint(workspace, id);
      void qc.invalidateQueries({ queryKey: getDataApiListEndpointVersionsQueryKey(workspace, id) });
    },
    [qc, invalidateEndpoint],
  );
}

export function usePublishEndpoint() {
  const invalidate = useInvalidateVersions();
  return useDataApiPublishEndpoint({
    mutation: { onSuccess: (_res, { workspace, id }) => invalidate(workspace, id) },
  });
}

export function useRollbackEndpoint() {
  const invalidate = useInvalidateVersions();
  return useDataApiRollbackEndpoint({
    mutation: { onSuccess: (_res, { workspace, id }) => invalidate(workspace, id) },
  });
}

/**
 * Create/delete a service account. The list key is shared by five surfaces —
 * the Clients table, Try-it's "Run as" picker, the Access checklist, the
 * OpenAPI download dialog and the Response preview — so all five are stale
 * together, and all five recover together from this one invalidation.
 */
export function useCreateServiceAccount() {
  const invalidate = useInvalidateServiceAccounts();
  return useDataApiCreateServiceAccount({
    mutation: { onSuccess: (_res, { workspace }) => invalidate(workspace) },
  });
}

export function useDeleteServiceAccount() {
  const invalidate = useInvalidateServiceAccounts();
  return useDataApiDeleteServiceAccount({
    mutation: { onSuccess: (_res, { workspace }) => invalidate(workspace) },
  });
}
