import { describe, it, expect, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import type { ReactNode } from "react";
import { useDashboardParams } from "../useDashboardParams";
import { useFilterDrafts } from "../useFilterDrafts";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "text", operator: "is" },
  { id: "p2", name: "Owner", slug: "owner", type: "text", operator: "is" },
];
const wrap = (initial: string) => ({ children }: { children: ReactNode }) =>
  <MemoryRouter initialEntries={[initial]}>{children}</MemoryRouter>;

const setup = (initial = "/d") =>
  renderHook(() => useFilterDrafts("dash-1", useDashboardParams(params)), { wrapper: wrap(initial) });

beforeEach(() => localStorage.clear());

describe("useFilterDrafts with auto-apply on (the default)", () => {
  it("writes straight through, so the cards see the change immediately", () => {
    const { result } = setup();
    expect(result.current.autoApply).toBe(true);
    act(() => result.current.setValue("region", "EU"));
    expect(result.current.appliedValues.region).toBe("EU");
    expect(result.current.changedCount).toBe(0);
  });
});

describe("useFilterDrafts with auto-apply off", () => {
  it("holds the change out of appliedValues until Apply", () => {
    const { result } = setup();
    act(() => result.current.setAutoApply(false));
    act(() => result.current.setValue("region", "EU"));

    // THE ASSERTION THAT MATTERS: the cards read appliedValues, and it must not have moved.
    expect(result.current.appliedValues.region).toBeUndefined();
    expect(result.current.editorValues.region).toBe("EU");
    expect(result.current.changedCount).toBe(1);

    act(() => result.current.apply());
    expect(result.current.appliedValues.region).toBe("EU");
    expect(result.current.changedCount).toBe(0);
  });

  it("applies every drafted filter, not just the last one", () => {
    const { result } = setup();
    act(() => result.current.setAutoApply(false));
    act(() => { result.current.setValue("region", "EU"); });
    act(() => { result.current.setValue("owner", "ana"); });
    expect(result.current.changedCount).toBe(2);
    act(() => result.current.apply());
    expect(result.current.appliedValues).toMatchObject({ region: "EU", owner: "ana" });
  });

  it("Cancel drops the draft and leaves the applied values alone", () => {
    const { result } = setup("/d?region=EU");
    act(() => result.current.setAutoApply(false));
    act(() => result.current.setValue("region", "US"));
    act(() => result.current.cancel());
    expect(result.current.editorValues.region).toBe("EU");
    expect(result.current.changedCount).toBe(0);
  });

  it("turning auto-apply back on applies the pending draft rather than stranding it", () => {
    const { result } = setup();
    act(() => result.current.setAutoApply(false));
    act(() => result.current.setValue("region", "EU"));
    act(() => result.current.setAutoApply(true));
    expect(result.current.appliedValues.region).toBe("EU");
    expect(result.current.changedCount).toBe(0);
  });
});

describe("the auto-apply preference", () => {
  it("persists per dashboard, and defaults to on", () => {
    const { result } = setup();
    act(() => result.current.setAutoApply(false));
    expect(localStorage.getItem("egret:dash-auto-apply:dash-1")).toBe("0");
    expect(setup().result.current.autoApply).toBe(false);
    localStorage.clear();
    expect(setup().result.current.autoApply).toBe(true);
  });
});
