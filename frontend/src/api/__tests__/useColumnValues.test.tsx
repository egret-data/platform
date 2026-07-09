import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

// The BI modules now route through the generated client → mutator → `apiFetch`.
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

import { useColumnValues } from "../useColumnValues";

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  apiFetch.mockReset();
  apiFetch.mockImplementation((url: string, opts?: { method?: string }) => {
    if (url.endsWith("/charts/data")) return Promise.resolve(jsonRes({ sql: "SELECT 1", database_id: "db1" }));
    if (url.endsWith("/queries") && opts?.method === "POST") return Promise.resolve(jsonRes({ id: "q1", status: "finished" }));
    if (url.includes("/queries/q1/results")) return Promise.resolve(jsonRes({ columns: [{ name: "region", type: "varchar" }], rows: [["EU"], ["US"], ["EU"]], page: 0, page_size: 1000, row_count: 3, truncated: false }));
    if (url.includes("/queries/q1")) return Promise.resolve(jsonRes({ id: "q1", database_id: "db1", status: "finished", columns: null, preview: null, row_count: 3, truncated: false, error: null }));
    return Promise.reject(new Error("unexpected " + url));
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
    const dataCall = apiFetch.mock.calls.find(([url]) => String(url).endsWith("/charts/data"));
    const body = JSON.parse((dataCall![1] as { body: string }).body);
    expect(body.query_spec.columns).toEqual(["region"]);
    expect(body.query_spec.dimensions).toEqual([]);
  });
  it("passes the filters arg into the option query spec", async () => {
    const filters = [{ col: "country", op: "in" as const, value: ["US"] }];
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "city", true, filters), { wrapper });
    await waitFor(() => expect(result.current.values.length).toBeGreaterThan(0));
    const dataCall = apiFetch.mock.calls.find(([url]) => String(url).endsWith("/charts/data"));
    const body = JSON.parse((dataCall![1] as { body: string }).body);
    expect(body.query_spec.filters).toEqual(filters);
    expect(body.query_spec.columns).toEqual(["city"]);
  });
  it("defaults filters to [] when omitted", async () => {
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "city", true), { wrapper });
    await waitFor(() => expect(result.current.values.length).toBeGreaterThan(0));
    const dataCall = apiFetch.mock.calls.find(([url]) => String(url).endsWith("/charts/data"));
    const body = JSON.parse((dataCall![1] as { body: string }).body);
    expect(body.query_spec.filters).toEqual([]);
  });
  it("is inert when disabled", () => {
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "region", false), { wrapper });
    expect(result.current.values).toEqual([]);
    expect(apiFetch).not.toHaveBeenCalled();
  });
});
