import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";

const apiJson = vi.fn().mockResolvedValue({ id: "new1", name: "X (copy)" });
vi.mock("../client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: vi.fn() }));

import { useDashboardMutations } from "../dashboards";

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, children);

describe("useDashboardMutations.duplicate", () => {
  beforeEach(() => apiJson.mockClear());
  it("POSTs to the duplicate endpoint and returns the copy", async () => {
    const { result } = renderHook(() => useDashboardMutations("eng"), { wrapper });
    let copy: { id: string } | undefined;
    await act(async () => { copy = await result.current.duplicate.mutateAsync("src1"); });
    expect(apiJson).toHaveBeenCalledWith("/ws/eng/dashboards/src1/duplicate", expect.objectContaining({ method: "POST" }));
    expect(copy?.id).toBe("new1");
  });
});
