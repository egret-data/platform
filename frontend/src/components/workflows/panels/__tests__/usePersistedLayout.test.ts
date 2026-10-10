import { act, renderHook } from "@testing-library/react";
import { Code, FileText, TriangleAlert } from "lucide-react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { CODE_TAB, DEFAULT_FLOW_LAYOUT, type Layout } from "@/components/workflows/panels/layout";
import { registerTab } from "@/components/workflows/panels/registry";
import { LAYOUT_KEY_PREFIX, usePersistedLayout } from "@/components/workflows/panels/usePersistedLayout";

const FLOW = "0b6f1c2e-0000-4000-8000-000000000001";
const KEY = `egret.wf.layout.${FLOW}`;

/** Not the default: Docs moved next to Code, sizes 70/30. A hook that ignored storage returns the default instead. */
const STORED: Layout = {
  panels: [
    { id: "p1", size: 70, tabs: [CODE_TAB, "docs"], active: "docs" },
    { id: "p2", size: 30, tabs: ["problems"], active: "problems" },
  ],
};

beforeAll(() => {
  for (const [id, icon] of [[CODE_TAB, Code], ["docs", FileText], ["problems", TriangleAlert]] as const) {
    registerTab({ id, title: id, icon, closable: id !== CODE_TAB, render: () => null });
  }
});

beforeEach(() => {
  localStorage.clear();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("usePersistedLayout", () => {
  it("uses the documented key", () => {
    expect(`${LAYOUT_KEY_PREFIX}${FLOW}`).toBe(KEY);
  });

  it("writes a change after 300 ms, not before, and reads it back on the next mount", () => {
    const first = renderHook(() => usePersistedLayout(FLOW, DEFAULT_FLOW_LAYOUT));
    expect(first.result.current[0]).toEqual(DEFAULT_FLOW_LAYOUT);
    act(() => first.result.current[1](STORED));
    expect(first.result.current[0]).toEqual(STORED);
    act(() => { vi.advanceTimersByTime(299); });
    expect(localStorage.getItem(KEY)).toBeNull();
    act(() => { vi.advanceTimersByTime(1); });
    expect(JSON.parse(localStorage.getItem(KEY)!)).toEqual(STORED);
    first.unmount();

    const second = renderHook(() => usePersistedLayout(FLOW, DEFAULT_FLOW_LAYOUT));
    expect(second.result.current[0]).toEqual(STORED);
  });

  it("writes a pending change when it unmounts inside the debounce", () => {
    const { result, unmount } = renderHook(() => usePersistedLayout(FLOW, DEFAULT_FLOW_LAYOUT));
    act(() => result.current[1](STORED));
    unmount();
    expect(JSON.parse(localStorage.getItem(KEY)!)).toEqual(STORED);
  });

  it("keeps each flow's layout under its own key", () => {
    localStorage.setItem(KEY, JSON.stringify(STORED));
    const { result, rerender } = renderHook(({ flow }) => usePersistedLayout(flow, DEFAULT_FLOW_LAYOUT), {
      initialProps: { flow: FLOW },
    });
    expect(result.current[0]).toEqual(STORED);
    rerender({ flow: "another-flow" });
    expect(result.current[0]).toEqual(DEFAULT_FLOW_LAYOUT);
  });

  it.each([
    ["unparsable JSON", "{panels:"],
    ["the wrong shape", JSON.stringify({ panels: "code" })],
    ["a JSON null", "null"],
  ])("falls back to the defaults for %s", (_name, stored) => {
    localStorage.setItem(KEY, stored);
    const { result } = renderHook(() => usePersistedLayout(FLOW, DEFAULT_FLOW_LAYOUT));
    expect(result.current[0]).toEqual(DEFAULT_FLOW_LAYOUT);
  });

  it("normalises a stored layout that names a tab nothing registered", () => {
    localStorage.setItem(KEY, JSON.stringify({
      panels: [
        { id: "p1", size: 50, tabs: ["topology"], active: "topology" },
        { id: "p2", size: 50, tabs: ["problems", "docs"], active: "docs" },
      ],
    }));
    const { result } = renderHook(() => usePersistedLayout(FLOW, DEFAULT_FLOW_LAYOUT));
    expect(result.current[0]).toEqual({
      panels: [{ id: "p2", size: 100, tabs: [CODE_TAB, "problems", "docs"], active: "docs" }],
    });
  });

  it("still works in memory when localStorage throws on read and on write", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("SecurityError"); });
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("QuotaExceededError"); });
    const { result, unmount } = renderHook(() => usePersistedLayout(FLOW, DEFAULT_FLOW_LAYOUT));
    expect(result.current[0]).toEqual(DEFAULT_FLOW_LAYOUT);
    act(() => result.current[1](STORED));
    act(() => { vi.advanceTimersByTime(300); });
    expect(setItem).toHaveBeenCalled();
    expect(result.current[0]).toEqual(STORED);
    expect(() => unmount()).not.toThrow();
  });
});
