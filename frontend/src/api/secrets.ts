/**
 * Named secrets API — MIGRATED to the orval-generated client (E0·P4d-3).
 *
 * Thin adapter over `./generated/control-plane-secrets/control-plane-secrets`
 * (tag `control-plane-secrets`), following the P4c/P4d pattern: hooks keep their
 * old names/signatures so call sites (`SecretPicker`) and the module-mocking
 * tests don't churn. Reads wrap the generated query hook (unwrapping orval's
 * `{ status, data, headers }` envelope via `select`); writes run `useMutation`
 * over the generated fetch fns and invalidate the GENERATED query key.
 *
 * NOTE — endpoint reconcile: the hand-written module targeted `/ws/{ws}/secrets`,
 * but the backend exposes these secrets ONLY at `/api/control-plane/secrets`
 * (global named secrets with a NULL `workspace_id`; there is no workspace-scoped
 * secrets HTTP route). The generated client hits the real control-plane path, so
 * the `ws` argument is now vestigial — retained on the hook signatures purely to
 * avoid churning the one caller. See the P4d-3 report.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  controlPlaneSecretsCreate,
  controlPlaneSecretsRemove,
  controlPlaneSecretsUpdate,
  getControlPlaneSecretsListQueryKey,
  useControlPlaneSecretsList,
} from "./generated/control-plane-secrets/control-plane-secrets";
import type {
  CreateSecretInput,
  SecretDto,
  UpdateSecretInput,
} from "./generated/model";

// Re-export generated types under the names the app already imports.
export type WorkspaceSecret = SecretDto;
export type SecretInput = CreateSecretInput;
export type SecretUpdate = UpdateSecretInput;

/** Back-compat query-key helper; delegates to the generated key factory. */
export const secretKeys = {
  // `ws` is vestigial (secrets are global control-plane rows); ignored.
  all: (_ws: string) => getControlPlaneSecretsListQueryKey(),
};

export function useSecrets(_ws: string) {
  return useControlPlaneSecretsList<WorkspaceSecret[], ApiError>({
    query: {
      // 403 (non-admin) is an expected outcome — surface it immediately instead
      // of retrying 3× before the workspace-admin notice can render.
      retry: false,
      select: (r) => r.data as WorkspaceSecret[],
    },
  });
}

export function useCreateSecret(_ws: string) {
  const queryClient = useQueryClient();
  return useMutation<WorkspaceSecret, ApiError, SecretInput>({
    mutationFn: async (req) => (await controlPlaneSecretsCreate(req)).data as WorkspaceSecret,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: getControlPlaneSecretsListQueryKey() });
    },
  });
}

export interface UpdateSecretArgs {
  id: string;
  input: SecretUpdate;
}

export function useUpdateSecret(_ws: string) {
  const queryClient = useQueryClient();
  return useMutation<WorkspaceSecret, ApiError, UpdateSecretArgs>({
    mutationFn: async ({ id, input }) =>
      (await controlPlaneSecretsUpdate(id, input)).data as WorkspaceSecret,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: getControlPlaneSecretsListQueryKey() });
    },
  });
}

export function useDeleteSecret(_ws: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await controlPlaneSecretsRemove(id);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: getControlPlaneSecretsListQueryKey() });
    },
  });
}
