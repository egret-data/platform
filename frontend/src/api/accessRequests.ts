/**
 * Access-request API hooks (P2b).
 *
 * Thin react-query wrappers over the access-request endpoints:
 *   GET  /api/access-requests/joinable                — workspaces the caller can request to join
 *   POST /api/access-requests                          — create a request
 *   GET  /api/access-requests/mine                      — the caller's own requests
 *   POST /api/access-requests/{id}/cancel               — cancel a pending request
 *   GET  /api/ws/{ws}/access-requests?status=           — approver inbox (used by a later task)
 *   POST /api/ws/{ws}/access-requests/{id}/approve       — approve a request
 *   POST /api/ws/{ws}/access-requests/{id}/deny          — deny a request
 *
 * These call `apiJson` directly (not the orval client), so paths are passed
 * WITHOUT the `/api` prefix — `apiJson`/`apiFetch` prepend it — mirroring
 * `useWorkspaceMembers` in `./members.ts`.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { membersKey } from "./members";
import { apiJson } from "./client";

export interface JoinableWorkspace {
  id: string;
  slug: string;
  name: string;
}

export interface AccessRequest {
  id: string;
  workspace_id: string;
  workspace_name: string | null;
  requester_id: string;
  requester_name: string | null;
  requested_role: string;
  reason: string | null;
  status: "pending" | "approved" | "denied" | "cancelled";
  decided_by: string | null;
  decided_at: string | null;
  decision_note: string | null;
  expires_at: string | null;
  created_at: string;
}

const joinableKey = ["access-requests", "joinable"] as const;
const mineKey = ["access-requests", "mine"] as const;
const wsRequestsKey = (ws: string) => ["ws", ws, "access-requests"] as const;

export function useJoinable(enabled = true) {
  return useQuery<JoinableWorkspace[]>({
    queryKey: joinableKey,
    queryFn: () => apiJson<JoinableWorkspace[]>("/access-requests/joinable"),
    retry: false,
    enabled,
  });
}

export function useMyRequests(enabled = true) {
  return useQuery<AccessRequest[]>({
    queryKey: mineKey,
    queryFn: () => apiJson<AccessRequest[]>("/access-requests/mine"),
    retry: false,
    enabled,
  });
}

export function useCreateRequest() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { workspace_id: string; role?: string; reason?: string }) =>
      apiJson<AccessRequest>("/access-requests", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: mineKey });
      qc.invalidateQueries({ queryKey: joinableKey });
    },
  });
}

export function useCancelRequest() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      apiJson<{ ok: true }>(`/access-requests/${id}/cancel`, { method: "POST" }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: mineKey });
      qc.invalidateQueries({ queryKey: joinableKey });
    },
  });
}

/** Approver inbox for a workspace (used by a later task). */
export function useWorkspaceRequests(ws: string, status = "pending") {
  return useQuery<AccessRequest[]>({
    queryKey: [...wsRequestsKey(ws), status],
    queryFn: () => apiJson<AccessRequest[]>(`/ws/${ws}/access-requests?status=${status}`),
    retry: false,
  });
}

export function useApproveRequest(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: { id: string; role?: string; expires_at?: string }) =>
      apiJson<AccessRequest>(`/ws/${ws}/access-requests/${id}/approve`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: wsRequestsKey(ws) });
      qc.invalidateQueries({ queryKey: membersKey(ws) });
    },
  });
}

export function useDenyRequest(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: { id: string; note?: string }) =>
      apiJson<AccessRequest>(`/ws/${ws}/access-requests/${id}/deny`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: wsRequestsKey(ws) }),
  });
}
