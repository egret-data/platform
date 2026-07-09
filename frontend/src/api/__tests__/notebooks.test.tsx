import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";

// The module is now a thin adapter over the orval-generated client, which
// routes every request through the shared `apiFetch` (via the custom mutator).
// So we mock only that seam; `ApiError` and the rest of `@/api/client` stay real.
const apiFetch = vi.fn(async () => new Response(null, { status: 204 }));
vi.mock("@/api/client", async (importActual) => {
  const actual = await importActual<typeof import("@/api/client")>();
  return {
    ...actual,
    apiFetch: (...a: unknown[]) => apiFetch(...(a as Parameters<typeof apiFetch>)),
  };
});

import { useUpdateCell, useNotebookRuntime, useActiveExecutions } from "../notebooks";

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  apiFetch.mockClear();
});

describe("notebook api hooks", () => {
  it("useUpdateCell sends source in the PATCH body and no sql_target", async () => {
    const { result } = renderHook(() => useUpdateCell("w", "n1"), { wrapper });
    result.current.mutate({ cid: "c1", source: "print(1)" });
    await waitFor(() => expect(apiFetch).toHaveBeenCalled());
    const [, init] = apiFetch.mock.calls[0] as unknown as [string, RequestInit];
    const body = JSON.parse(init.body as string);
    expect(body).toMatchObject({ source: "print(1)" });
    expect(body).not.toHaveProperty("sql_target");
  });

  it("useNotebookRuntime GETs /ws/{ws}/notebook-runtime", async () => {
    renderHook(() => useNotebookRuntime("w"), { wrapper });
    await waitFor(() =>
      expect(apiFetch).toHaveBeenCalledWith(
        "/ws/w/notebook-runtime",
        expect.objectContaining({ method: "GET" }),
      ),
    );
  });

  it("useActiveExecutions GETs the active-executions endpoint", async () => {
    renderHook(() => useActiveExecutions("w", "n1"), { wrapper });
    await waitFor(() =>
      expect(apiFetch).toHaveBeenCalledWith(
        "/ws/w/notebooks/n1/kernel/active-executions",
        expect.objectContaining({ method: "GET" }),
      ),
    );
  });
});
