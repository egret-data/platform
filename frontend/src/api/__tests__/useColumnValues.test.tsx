import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

// `useColumnValues` now runs through `useSpecData` -> `chartsChRunSpec` (`POST
// /charts/run`), a single request, rather than the old build -> submit ->
// poll pipeline through the free-form SQL endpoint.
const chartsChRunSpec = vi.fn();
vi.mock("@/api/generated/charts/charts", () => ({
  chartsChRunSpec: (...a: unknown[]) => chartsChRunSpec(...a),
}));

import { useColumnValues } from "../useColumnValues";

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  chartsChRunSpec.mockReset();
  chartsChRunSpec.mockResolvedValue({
    data: { columns: [{ name: "region", type: "varchar" }], rows: [["EU"], ["US"], ["EU"]] },
  });
});

describe("useColumnValues", () => {
  it("returns distinct column values", async () => {
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "region", true), { wrapper });
    await waitFor(() => expect(result.current.values.length).toBe(2));
    expect(result.current.values).toEqual(["EU", "US"]);
  });

  it("queries the column via `columns` (not a dimensions-only group-by the engine would reject)", async () => {
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "region", true), { wrapper });
    await waitFor(() => expect(result.current.values.length).toBe(2));
    const [, body] = chartsChRunSpec.mock.calls[0];
    expect(body.query_spec.columns).toEqual(["region"]);
    expect(body.query_spec.dimensions).toEqual([]);
  });

  it("passes the filters arg into the option query spec", async () => {
    const filters = [{ col: "country", op: "in" as const, value: ["US"] }];
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "city", true, filters), { wrapper });
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    const [, body] = chartsChRunSpec.mock.calls[0];
    expect(body.query_spec.filters).toEqual(filters);
    expect(body.query_spec.columns).toEqual(["city"]);
  });

  it("defaults filters to [] when omitted", async () => {
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "city", true), { wrapper });
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    const [, body] = chartsChRunSpec.mock.calls[0];
    expect(body.query_spec.filters).toEqual([]);
  });

  it("is inert when disabled", () => {
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "region", false), { wrapper });
    expect(result.current.values).toEqual([]);
    expect(chartsChRunSpec).not.toHaveBeenCalled();
  });

  it("does not refetch on an unrelated rerender (a fresh `spec` object identity every render must not loop)", async () => {
    const { result, rerender } = renderHook(() => useColumnValues("eng", "ds1", "region", true), { wrapper });
    await waitFor(() => expect(result.current.values.length).toBe(2));
    const callsAfterFirstLoad = chartsChRunSpec.mock.calls.length;
    rerender();
    rerender();
    rerender();
    await waitFor(() => expect(result.current.isLoading).toBe(false));
    expect(chartsChRunSpec.mock.calls.length).toBe(callsAfterFirstLoad);
  });
});
