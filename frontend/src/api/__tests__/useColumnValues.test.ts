import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";

const buildSpy = vi.fn().mockResolvedValue({ sql: "SELECT 1", database_id: "db1" });
const submitSpy = vi.fn().mockResolvedValue({ id: "q1" });

vi.mock("@/api/charts", () => ({
  useBuildChartData: () => ({ mutateAsync: buildSpy }),
}));
vi.mock("@/api/sqllab", () => ({
  useSubmitQuery: () => ({ mutateAsync: submitSpy }),
  useQueryStatus: () => ({ data: { status: "running" } }),
  useResults: () => ({ data: undefined }),
}));

import { useColumnValues } from "../useColumnValues";

const wrapper = ({ children }: { children: ReactNode }) =>
  createElement(QueryClientProvider, { client: new QueryClient() }, children);

describe("useColumnValues", () => {
  beforeEach(() => { buildSpy.mockClear(); submitSpy.mockClear(); });

  it("passes the filters arg into the option query spec", async () => {
    const filters = [{ col: "country", op: "in" as const, value: ["US"] }];
    renderHook(() => useColumnValues("ws", "ds1", "city", true, filters), { wrapper });
    await vi.waitFor(() => expect(buildSpy).toHaveBeenCalled());
    expect(buildSpy.mock.calls[0][0].query_spec.filters).toEqual(filters);
    expect(buildSpy.mock.calls[0][0].query_spec.columns).toEqual(["city"]);
  });

  it("defaults filters to [] when omitted", async () => {
    renderHook(() => useColumnValues("ws", "ds1", "city", true), { wrapper });
    await vi.waitFor(() => expect(buildSpy).toHaveBeenCalled());
    expect(buildSpy.mock.calls[0][0].query_spec.filters).toEqual([]);
  });
});
