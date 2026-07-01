import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";

const apiFetch = vi.fn(async () => new Response(null, { status: 204 }));
const apiJson = vi.fn(async () => ({
  username: "u",
  engines: [],
  engine_jwt: null,
  engine_jwt_expires_at: null,
}));
vi.mock("@/api/client", () => ({
  apiFetch: (...a: unknown[]) => apiFetch(...(a as Parameters<typeof apiFetch>)),
  apiJson: (...a: unknown[]) => apiJson(...(a as Parameters<typeof apiJson>)),
}));

import { useUpdateCell, useNotebookRuntime, useActiveExecutions } from "../notebooks";

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  apiFetch.mockClear();
  apiJson.mockClear();
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
    await waitFor(() => expect(apiJson).toHaveBeenCalledWith("/ws/w/notebook-runtime"));
  });

  it("useActiveExecutions GETs the active-executions endpoint", async () => {
    renderHook(() => useActiveExecutions("w", "n1"), { wrapper });
    await waitFor(() =>
      expect(apiJson).toHaveBeenCalledWith("/ws/w/notebooks/n1/kernel/active-executions"),
    );
  });
});
