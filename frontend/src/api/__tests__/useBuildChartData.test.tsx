import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

// `useBuildChartData` is now a `useQuery` (not a mutation) over the
// build-only `chartsChData` endpoint (POST /charts/data), used by
// ChartExplore's lazy "View SQL" disclosure — it never submits or runs the
// query, so it stays reachable even when a workspace has SQL Lab switched
// off (it's gated by the `charts` entitlement, not `sql_lab`).
const chartsChData = vi.fn();
vi.mock("@/api/generated/charts/charts", () => ({
  chartsChData: (...a: unknown[]) => chartsChData(...a),
}));

import { useBuildChartData } from "../charts";

const wrapper = ({ children }: { children: React.ReactNode }) => (
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    {children}
  </QueryClientProvider>
);

const spec = { metrics: ["rev"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] };

beforeEach(() => {
  chartsChData.mockReset();
});

describe("useBuildChartData", () => {
  it("does not fetch while disabled (e.g. the disclosure is closed)", () => {
    chartsChData.mockResolvedValue({ data: { sql: "SELECT 1", database_id: "db1" } });

    renderHook(() => useBuildChartData("ws", "ds-1", spec, false), { wrapper });

    expect(chartsChData).not.toHaveBeenCalled();
  });

  it("fetches once when enabled and returns the compiled sql", async () => {
    chartsChData.mockResolvedValue({ data: { sql: "SELECT 1", database_id: "db1" } });

    const { result } = renderHook(() => useBuildChartData("ws", "ds-1", spec, true), { wrapper });

    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(chartsChData).toHaveBeenCalledTimes(1);
    expect(chartsChData).toHaveBeenCalledWith("ws", { dataset_id: "ds-1", query_spec: spec });
    expect(result.current.data).toEqual({ sql: "SELECT 1", database_id: "db1" });
  });
});
