import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";

const apiJson = vi.fn();
vi.mock("@/api/client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: vi.fn() }));

import { useChartData } from "../useChartData";
import type { Chart } from "@/api/charts";

const chart: Chart = {
  id: "c1", dataset_id: "ds1", name: "S", description: "", viz_type: "table",
  params: { spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } },
  created_by: null, updated_at: "",
};

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  apiJson.mockReset();
  apiJson.mockImplementation((url: string, opts?: { method?: string }) => {
    if (url.endsWith("/charts/data")) return Promise.resolve({ sql: "SELECT 1", database_id: "db1" });
    if (url.endsWith("/queries") && opts?.method === "POST") return Promise.resolve({ id: "q1", status: "finished" });
    if (url.includes("/queries/q1/results")) return Promise.resolve({ columns: [{ name: "rev", type: "double" }], rows: [[5]], page: 0, page_size: 100, row_count: 1, truncated: false });
    if (url.includes("/queries/q1")) return Promise.resolve({ id: "q1", database_id: "db1", status: "finished", columns: null, preview: null, row_count: 1, truncated: false, error: null });
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
});
