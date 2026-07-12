import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import React from "react";

// The object-scoped card-data hook routes through the orval-generated client →
// the mutator → `apiFetch`. We stub `apiFetch` and assert on the POST body it
// receives (the resolved dashboard-parameter filters).
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

import { useDashboardCardData } from "../useSavedChartData";
import type { ChartFilter } from "@/api/charts";

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  apiFetch.mockReset();
  apiFetch.mockImplementation(() =>
    Promise.resolve(jsonRes({ columns: [{ name: "rev", type: "double" }], rows: [[5]] })),
  );
});

const cardDataCalls = () =>
  apiFetch.mock.calls.filter((c: unknown[]) => String(c[0]).includes("/cards/") && String(c[0]).endsWith("/data"));

describe("useDashboardCardData", () => {
  it("POSTs the resolved dashboard-param filters in the request body", async () => {
    const filters: ChartFilter[] = [{ col: "region", op: "eq", value: "EU" }];
    const { result } = renderHook(() => useDashboardCardData("eng", "d1", "k", undefined, true, filters), { wrapper });
    await waitFor(() => expect(result.current.rows).toEqual([[5]]));
    const call = cardDataCalls().at(-1)!;
    const opts = call[1] as { method?: string; body?: string };
    expect(opts.method).toBe("POST");
    expect(JSON.parse(opts.body ?? "{}")).toEqual({ filters, temporal_units: {} });
  });

  it("POSTs resolved temporal-unit overrides in the request body", async () => {
    const temporalUnits = { created_at: "month" };
    const { result } = renderHook(() => useDashboardCardData("eng", "d1", "k", undefined, true, [], temporalUnits), { wrapper });
    await waitFor(() => expect(result.current.rows).toEqual([[5]]));
    const opts = cardDataCalls().at(-1)![1] as { body?: string };
    expect(JSON.parse(opts.body ?? "{}")).toEqual({ filters: [], temporal_units: temporalUnits });
  });

  it("refetches when the filters change (new query key)", async () => {
    let filters: ChartFilter[] = [{ col: "region", op: "eq", value: "EU" }];
    const { result, rerender } = renderHook(() => useDashboardCardData("eng", "d1", "k", undefined, true, filters), { wrapper });
    await waitFor(() => expect(result.current.rows).toEqual([[5]]));
    const before = cardDataCalls().length;
    filters = [{ col: "region", op: "eq", value: "US" }];
    rerender();
    await waitFor(() => expect(cardDataCalls().length).toBe(before + 1));
    // The new fetch carries the changed filter value.
    expect(JSON.parse((cardDataCalls().at(-1)![1] as { body?: string }).body ?? "{}")).toEqual({ filters, temporal_units: {} });
  });

  it("does not fetch until a chart-backed card id is present", () => {
    renderHook(() => useDashboardCardData("eng", "d1", null, undefined, true, []), { wrapper });
    expect(cardDataCalls().length).toBe(0);
  });
});
