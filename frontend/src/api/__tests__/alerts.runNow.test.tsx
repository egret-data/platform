import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";

// Mock only the shared `apiFetch` seam the generated client goes through.
const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const apiFetch = vi.fn(async (_path: string, _init?: RequestInit) => new Response(null, { status: 204 }));
vi.mock("@/api/client", async (importActual) => {
  const actual = await importActual<typeof import("@/api/client")>();
  return {
    ...actual,
    apiFetch: (...a: unknown[]) => apiFetch(...(a as Parameters<typeof apiFetch>)),
  };
});

import { useScheduleMutations } from "../alerts";
import {
  getAlertsDetailQueryKey,
  getAlertsListQueryKey,
  getAlertsRunsQueryKey,
} from "../generated/alerts/alerts";

beforeEach(() => {
  apiFetch.mockReset();
});

describe("useScheduleMutations().runNow", () => {
  it("refreshes the list, the detail and the run log, because the run's outcome is the schedule's last state", async () => {
    // Arrange — KI-032: the list's Last run and the detail badge stayed "Never run" after Run now.
    apiFetch.mockImplementation(async () => json(200, { id: "r1", state: "success" }));
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidate = vi.spyOn(qc, "invalidateQueries");
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useScheduleMutations("w"), { wrapper });

    // Act
    await act(() => result.current.runNow.mutateAsync("s1"));

    // Assert
    expect(apiFetch.mock.calls[0][0]).toBe("/ws/w/alerts/s1/run");
    const keys = invalidate.mock.calls.map((c) => c[0]?.queryKey);
    expect(keys).toContainEqual(getAlertsRunsQueryKey("w", "s1"));
    expect(keys).toContainEqual(getAlertsListQueryKey("w"));
    expect(keys).toContainEqual(getAlertsDetailQueryKey("w", "s1"));
  });
});
