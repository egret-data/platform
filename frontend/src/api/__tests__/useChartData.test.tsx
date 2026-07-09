import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";

// The BI modules now route through the orval-generated client → the mutator →
// `apiFetch` (which returns a raw `Response`). We stub `apiFetch` and assert on
// its calls; the mutator builds the `{ status, data, headers }` envelope.
const apiFetch = vi.fn();
vi.mock("@/api/client", () => ({
  apiJson: vi.fn(),
  apiFetch: (...a: unknown[]) => apiFetch(...a),
  ApiError: class ApiError extends Error {},
  UnauthorizedError: class UnauthorizedError extends Error {},
}));

const jsonRes = (body: unknown) => ({
  status: 200,
  headers: new Headers({ "content-type": "application/json" }),
  json: () => Promise.resolve(body),
});

import { useChartData } from "../useChartData";
import type { Chart } from "@/api/charts";

const chart: Chart = {
  id: "c1", dataset_id: "ds1", name: "S", description: "", viz_type: "table",
  params: { spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } },
  created_by: null, updated_at: "", folder_id: null,
};

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  apiFetch.mockReset();
  apiFetch.mockImplementation((url: string, opts?: { method?: string }) => {
    if (url.endsWith("/charts/data")) return Promise.resolve(jsonRes({ sql: "SELECT 1", database_id: "db1" }));
    if (url.endsWith("/queries") && opts?.method === "POST") return Promise.resolve(jsonRes({ id: "q1", status: "finished" }));
    if (url.includes("/queries/q1/results")) return Promise.resolve(jsonRes({ columns: [{ name: "rev", type: "double" }], rows: [[5]], page: 0, page_size: 100, row_count: 1, truncated: false }));
    if (url.includes("/queries/q1")) return Promise.resolve(jsonRes({ id: "q1", database_id: "db1", status: "finished", columns: null, preview: null, row_count: 1, truncated: false, error: null }));
    return Promise.reject(new Error("unexpected " + url));
  });
});

describe("useChartData", () => {
  it("builds, submits, polls, and returns the result rows", async () => {
    const { result } = renderHook(() => useChartData("eng", chart), { wrapper });
    await waitFor(() => expect(result.current.columns.length).toBe(1));
    expect(result.current.rows).toEqual([[5]]);
    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeNull();
  });

  it("refetches when the nonce changes", async () => {
    let nonce = 0;
    const { result, rerender } = renderHook(() => useChartData("eng", chart, nonce), { wrapper });
    await waitFor(() => expect(result.current.rows).toEqual([[5]]));
    const buildCalls = apiFetch.mock.calls.filter((c: unknown[]) => String(c[0]).endsWith("/charts/data")).length;
    nonce = 1;
    rerender();
    await waitFor(() => {
      const after = apiFetch.mock.calls.filter((c: unknown[]) => String(c[0]).endsWith("/charts/data")).length;
      expect(after).toBe(buildCalls + 1);
    });
  });
});
