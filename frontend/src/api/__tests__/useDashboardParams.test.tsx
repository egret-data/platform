import { describe, it, expect } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import type { ReactNode } from "react";
import { useDashboardParams } from "../useDashboardParams";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "text", operator: "is", default: "EU" },
  { id: "p2", name: "Tags", slug: "tags", type: "category", operator: "is" },
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
});
