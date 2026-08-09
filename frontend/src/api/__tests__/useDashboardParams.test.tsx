import { describe, it, expect } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import type { ReactNode } from "react";
import { useDashboardParams } from "../useDashboardParams";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "text", operator: "is", default: "EU" },
  { id: "p2", name: "Tags", slug: "tags", type: "category", operator: "is" },
  { id: "p3", name: "City", slug: "city", type: "location", operator: "is" },
];
const wrap = (initial: string) => ({ children }: { children: ReactNode }) =>
  <MemoryRouter initialEntries={[initial]}>{children}</MemoryRouter>;

describe("useDashboardParams", () => {
  it("seeds from default when the URL has no value", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    expect(result.current.values.region).toBe("EU");
  });
  it("reads a scalar value from the URL", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d?region=US") });
    expect(result.current.values.region).toBe("US");
  });
  it("reads a JSON array value (category) from the URL", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap(`/d?tags=${encodeURIComponent('["a","b"]')}`) });
    expect(result.current.values.tags).toEqual(["a", "b"]);
  });
  it("setValue writes to the URL", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    act(() => result.current.setValue("region", "APAC"));
    expect(result.current.values.region).toBe("APAC");
  });

  it("round-trips a multi-value location, including a value containing a comma", () => {
    // A genuine round trip: the write goes into the MemoryRouter's search params and the read
    // comes back out of them. The comma-bearing value is the point — it is the case a
    // comma-joined encoding could not survive even in principle, so this pins the encoding
    // against anyone who reintroduces one.
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    const picked = ["Hà Nội", "Đà Nẵng, Việt Nam"];
    act(() => result.current.setValue("city", picked));
    expect(result.current.values.city).toEqual(picked);
  });

  it("does not JSON-decode a scalar list value", () => {
    // Reachable today: DashboardView's drill "Filter by" and crossfilter click behaviour both
    // call setValue with a SCALAR. Decoding it turned "42" into the NUMBER 42, and `in [42]`
    // against a text column matches nothing. Assert the RAW value, not a label.
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d?tags=42") });
    expect(result.current.values.tags).toBe("42");
  });
});
