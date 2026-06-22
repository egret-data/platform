import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import type { Chart } from "@/api/charts";

const apiJson = vi.fn();
vi.mock("@/api/client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: vi.fn() }));

import { useChartData } from "../useChartData";

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

const chart = { id: "c1", dataset_id: "ds1", viz_type: "bar", params: { spec: { metrics: ["m"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } } as unknown as Chart;

beforeEach(() => { apiJson.mockReset(); apiJson.mockResolvedValue({ sql: "S", database_id: "db1" }); });

describe("useChartData enabled gate", () => {
  it("fires no request and is not loading when enabled=false", () => {
    const { result } = renderHook(() => useChartData("eng", chart, undefined, false), { wrapper });
    expect(apiJson).not.toHaveBeenCalled();
    expect(result.current.isLoading).toBe(false);
  });
});
