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

import { useTestDatabase } from "../databases";
import { getDatabasesListQueryKey } from "../generated/databases/databases";

beforeEach(() => {
  apiFetch.mockReset();
});

describe("useTestDatabase", () => {
  it("refreshes the connection list, because the server stores the test's outcome as the status (KI-011)", async () => {
    // Arrange — the Status column and the counts kept the old status until a reload.
    apiFetch.mockImplementation(async () => json(200, { ok: true, latency_ms: 4, detail: "SELECT 1 ok" }));
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidate = vi.spyOn(qc, "invalidateQueries");
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useTestDatabase("w"), { wrapper });

    // Act
    await act(() => result.current.mutateAsync("db-1"));

    // Assert
    expect(apiFetch.mock.calls[0][0]).toBe("/ws/w/databases/db-1/test");
    expect(invalidate.mock.calls.map((c) => c[0]?.queryKey)).toContainEqual(getDatabasesListQueryKey("w"));
  });
});
