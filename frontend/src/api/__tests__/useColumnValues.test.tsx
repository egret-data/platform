import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

const apiJson = vi.fn();
vi.mock("@/api/client", () => ({ apiJson: (...a: unknown[]) => apiJson(...a), apiFetch: vi.fn() }));

import { useColumnValues } from "../useColumnValues";

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

beforeEach(() => {
  apiJson.mockReset();
  apiJson.mockImplementation((url: string, opts?: { method?: string }) => {
    if (url.endsWith("/charts/data")) return Promise.resolve({ sql: "SELECT 1", database_id: "db1" });
    if (url.endsWith("/queries") && opts?.method === "POST") return Promise.resolve({ id: "q1", status: "finished" });
    if (url.includes("/queries/q1/results")) return Promise.resolve({ columns: [{ name: "region", type: "varchar" }], rows: [["EU"], ["US"], ["EU"]], page: 0, page_size: 1000, row_count: 3, truncated: false });
    if (url.includes("/queries/q1")) return Promise.resolve({ id: "q1", database_id: "db1", status: "finished", columns: null, preview: null, row_count: 3, truncated: false, error: null });
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
    const dataCall = apiJson.mock.calls.find(([url]) => String(url).endsWith("/charts/data"));
    const body = JSON.parse((dataCall![1] as { body: string }).body);
    expect(body.query_spec.columns).toEqual(["region"]);
    expect(body.query_spec.dimensions).toEqual([]);
  });
  it("is inert when disabled", () => {
    const { result } = renderHook(() => useColumnValues("eng", "ds1", "region", false), { wrapper });
    expect(result.current.values).toEqual([]);
    expect(apiJson).not.toHaveBeenCalled();
  });
});
