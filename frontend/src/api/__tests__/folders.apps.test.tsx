import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";

// Mock only the shared `apiFetch` seam the generated client goes through.
const apiFetch = vi.fn(async () => new Response(null, { status: 204 }));
vi.mock("@/api/client", async (importActual) => {
  const actual = await importActual<typeof import("@/api/client")>();
  return {
    ...actual,
    apiFetch: (...a: unknown[]) => apiFetch(...(a as Parameters<typeof apiFetch>)),
  };
});

import { useSetFolder } from "../folders";
import { getAppsListQueryKey } from "../generated/apps/apps";

beforeEach(() => {
  apiFetch.mockClear();
});

describe("useSetFolder for apps", () => {
  it("PATCHes the app's folder route and invalidates the apps list (live and trash)", async () => {
    // Arrange
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidate = vi.spyOn(qc, "invalidateQueries");
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={qc}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => useSetFolder("w", "apps"), { wrapper });

    // Act
    result.current.mutate({ id: "a1", folder_id: "f1" });

    // Assert
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    const [path, init] = apiFetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe("/ws/w/apps/a1/folder");
    expect(init.method).toBe("PATCH");
    expect(JSON.parse(init.body as string)).toEqual({ folder_id: "f1" });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: getAppsListQueryKey("w") });
    expect(getAppsListQueryKey("w")).toEqual(["/api/ws/w/apps"]);
  });
});
