import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";

// `useChartData` now runs through `useSpecData` -> `chartsChRunSpec` (`POST
// /charts/run`), a single request that compiles and executes the spec
// server-side — no more build -> submit -> poll through the free-form SQL
// endpoint.
const chartsChRunSpec = vi.fn();
vi.mock("@/api/generated/charts/charts", () => ({
  chartsChRunSpec: (...a: unknown[]) => chartsChRunSpec(...a),
}));

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
  chartsChRunSpec.mockReset();
  chartsChRunSpec.mockResolvedValue({ data: { columns: [{ name: "rev", type: "double" }], rows: [[5]] } });
});

describe("useChartData", () => {
  it("runs the chart's spec in a single request and returns the result rows", async () => {
    const { result } = renderHook(() => useChartData("eng", chart), { wrapper });
    await waitFor(() => expect(result.current.columns.length).toBe(1));
    expect(result.current.rows).toEqual([[5]]);
    expect(result.current.isLoading).toBe(false);
    expect(result.current.error).toBeNull();
    expect(chartsChRunSpec).toHaveBeenCalledTimes(1);
    expect(chartsChRunSpec).toHaveBeenCalledWith("eng", { dataset_id: "ds1", query_spec: chart.params.spec });
  });

  it("refetches when the nonce changes", async () => {
    let nonce = 0;
    const { result, rerender } = renderHook(() => useChartData("eng", chart, nonce), { wrapper });
    await waitFor(() => expect(result.current.rows).toEqual([[5]]));
    const before = chartsChRunSpec.mock.calls.length;
    nonce = 1;
    rerender();
    await waitFor(() => expect(chartsChRunSpec.mock.calls.length).toBe(before + 1));
  });
});
