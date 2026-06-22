import { describe, it, expect } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { useDashboardTabs } from "../useDashboardTabs";

const wrap = (initial: string) =>
  ({ children }: { children: React.ReactNode }) => <MemoryRouter initialEntries={[initial]}>{children}</MemoryRouter>;

describe("useDashboardTabs", () => {
  it("reads ?tab= and clamps into range", () => {
    const { result } = renderHook(() => useDashboardTabs(3), { wrapper: wrap("/d?tab=2") });
    expect(result.current.activeTabPos).toBe(2);
  });
  it("clamps an out-of-range ?tab= to the last tab", () => {
    const { result } = renderHook(() => useDashboardTabs(2), { wrapper: wrap("/d?tab=9") });
    expect(result.current.activeTabPos).toBe(1);
  });
  it("defaults to 0 with no ?tab=", () => {
    const { result } = renderHook(() => useDashboardTabs(3), { wrapper: wrap("/d") });
    expect(result.current.activeTabPos).toBe(0);
  });
  it("setActiveTabPos writes the param", () => {
    const { result } = renderHook(() => useDashboardTabs(3), { wrapper: wrap("/d") });
    act(() => result.current.setActiveTabPos(2));
    expect(result.current.activeTabPos).toBe(2);
  });
  it("clamps to 0 when there are no tabs", () => {
    const { result } = renderHook(() => useDashboardTabs(0), { wrapper: wrap("/d?tab=1") });
    expect(result.current.activeTabPos).toBe(0);
  });
});
