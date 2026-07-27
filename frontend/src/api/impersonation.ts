/**
 * Start-impersonation hooks (Task 11).
 *
 * Thin `useMutation` wrappers over the orval-generated start endpoints:
 *   POST /api/admin/impersonate         — platform_admin → any non-platform_admin user
 *   POST /api/ws/{workspace}/impersonate — ws_admin (or platform_admin) → a member of that workspace
 *
 * Both generated functions go through `customInstance` → `apiFetch` (see
 * `./mutator.ts`), which throws on any non-2xx response: `UnauthorizedError` on
 * 401, `ApiError` otherwise. So a successful `await` here always means the
 * overlay was created; every failure — including the `409 oidc_step_up`
 * step-up redirect — arrives as a thrown error, not a returned `{status: 409}`
 * envelope, regardless of what the generated response union's `TError` claims
 * (the ws endpoint's spec doesn't type a 409 body at all, but the backend does
 * send one — see `workspaceMembersWsImpersonateStart`'s doc comment).
 *
 * `ApiError` only structurally guarantees `status`/`message`/`code` from the
 * body's `error`/`code` fields; it does NOT know about `authorize_url`. It also
 * carries the full parsed JSON body (`.body`, added for this task) precisely so
 * a caller can reach fields a generic error type doesn't model — that's how the
 * `OidcStepUp` redirect below finds its `authorize_url`.
 */
import { useMutation, useQueryClient, type UseMutationResult } from "@tanstack/react-query";

import { ApiError } from "./client";
import { authImpersonateStart } from "./generated/auth/auth";
import type { StartImpersonation, StartWsImpersonation } from "./generated/model";
import { workspaceMembersWsImpersonateStart } from "./generated/workspace-members/workspace-members";

/** Redirects the browser to the OIDC re-auth URL carried by a `409 oidc_step_up`
 * error body. The backend's OIDC callback completes the impersonation and
 * redirects back — there is nothing further for the SPA to do on this branch. */
function redirectOnStepUp(err: unknown): void {
  if (!(err instanceof ApiError) || err.status !== 409 || err.code !== "oidc_step_up") return;
  const authorizeUrl = (err.body as { authorize_url?: unknown } | undefined)?.authorize_url;
  if (typeof authorizeUrl === "string" && authorizeUrl) {
    window.location.href = authorizeUrl;
  }
}

/** Platform-scoped impersonation start: `POST /api/admin/impersonate`. */
export function useStartPlatformImpersonation(): UseMutationResult<void, ApiError, StartImpersonation> {
  const qc = useQueryClient();
  return useMutation<void, ApiError, StartImpersonation>({
    mutationFn: async (body) => {
      await authImpersonateStart(body);
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["me"] });
      qc.invalidateQueries({ queryKey: ["my-workspaces"] });
    },
    onError: redirectOnStepUp,
  });
}

/** Workspace-scoped impersonation start: `POST /api/ws/{workspace}/impersonate`. */
export function useStartWsImpersonation(workspace: string): UseMutationResult<void, ApiError, StartWsImpersonation> {
  const qc = useQueryClient();
  return useMutation<void, ApiError, StartWsImpersonation>({
    mutationFn: async (body) => {
      await workspaceMembersWsImpersonateStart(workspace, body);
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["me"] });
      qc.invalidateQueries({ queryKey: ["my-workspaces"] });
    },
    onError: redirectOnStepUp,
  });
}
