import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";

// Migrated to the generated client → mutator → `apiFetch` (returns a `Response`).
const apiFetch = vi.fn().mockResolvedValue({
  status: 200,
  headers: new Headers({ "content-type": "application/json" }),
  json: () => Promise.resolve({ id: "new1", name: "X (copy)" }),
});
vi.mock("../client", () => ({
  apiJson: vi.fn(),
  apiFetch: (...a: unknown[]) => apiFetch(...a),
  ApiError: class ApiError extends Error {},
  UnauthorizedError: class UnauthorizedError extends Error {},
}));

import { useDashboardMutations } from "../dashboards";

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, children);

describe("useDashboardMutations.duplicate", () => {
  beforeEach(() => apiFetch.mockClear());
  it("POSTs to the duplicate endpoint and returns the copy", async () => {
    const { result } = renderHook(() => useDashboardMutations("eng"), { wrapper });
    let copy: { id: string } | undefined;
    await act(async () => { copy = await result.current.duplicate.mutateAsync("src1"); });
    expect(apiFetch).toHaveBeenCalledWith("/ws/eng/dashboards/src1/duplicate", expect.objectContaining({ method: "POST" }));
    expect(copy?.id).toBe("new1");
  });
});
