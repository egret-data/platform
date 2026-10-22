/**
 * KI-019 / KI-127: the member role picker offers the workspace's own role
 * catalog (`GET /api/ws/{ws}/roles`), the list the Roles & permissions page
 * shows, not the Control Plane role list. The Control Plane list needs
 * `role:read`, which a workspace admin does not hold, and it carries
 * `data_api_invoker`, a role that is granted per endpoint, never as a
 * membership.
 */
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { useWorkspaceRoles } from "@/api/members";

afterEach(() => vi.unstubAllGlobals());

test("the member picker reads the workspace role catalog", async () => {
  const fetchMock = vi.fn(() =>
    Promise.resolve(
      new Response(JSON.stringify([{ id: "r1", name: "ws_viewer", description: "", parent_role_id: null, permissions: [], inherited_permissions: [] }]), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    ),
  );
  vi.stubGlobal("fetch", fetchMock);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={qc}>{children}</QueryClientProvider>
  );

  const { result } = renderHook(() => useWorkspaceRoles("demo"), { wrapper });

  await waitFor(() => expect(result.current.data?.map((r) => r.name)).toEqual(["ws_viewer"]));
  expect(fetchMock).toHaveBeenCalledTimes(1);
  expect(String((fetchMock.mock.calls[0] as unknown[])[0])).toBe("/api/ws/demo/roles");
});
