import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

const chartsChRunSpec = vi.fn();
vi.mock("@/api/generated/charts/charts", () => ({
  chartsChRunSpec: (...a: unknown[]) => chartsChRunSpec(...a),
}));

import { useSpecData } from "../useSpecData";

const wrapper = ({ children }: { children: React.ReactNode }) => (
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    {children}
  </QueryClientProvider>
);

const spec = { metrics: [], dimensions: [], columns: ["id"], filters: [], orderby: [], row_limit: 10, rollup_groups: [] };

beforeEach(() => {
  chartsChRunSpec.mockReset();
});

describe("useSpecData", () => {
  it("runs a spec in a single request and returns columns and rows", async () => {
    chartsChRunSpec.mockResolvedValue({ data: { columns: [{ name: "id", type: "int" }], rows: [[1]] } });

    const { result } = renderHook(() => useSpecData("ws", "ds-1", spec, true), { wrapper });

    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(chartsChRunSpec).toHaveBeenCalledTimes(1);
    expect(result.current.columns).toEqual([{ name: "id", type: "int" }]);
    expect(result.current.rows).toEqual([[1]]);
  });

  it("does not fetch while disabled", async () => {
    chartsChRunSpec.mockResolvedValue({ data: { columns: [], rows: [] } });

    renderHook(() => useSpecData("ws", "ds-1", spec, false), { wrapper });

    expect(chartsChRunSpec).not.toHaveBeenCalled();
  });

  it("surfaces the error message", async () => {
    chartsChRunSpec.mockRejectedValue(new Error("boom"));

    const { result } = renderHook(() => useSpecData("ws", "ds-1", spec, true), { wrapper });

    await waitFor(() => expect(result.current.error).toContain("boom"));
  });
});
