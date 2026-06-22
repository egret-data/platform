import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import type { ReactNode } from "react";
import { useDashboardViewControls } from "../useDashboardViewControls";

function wrap(initial: string) {
  return ({ children }: { children: ReactNode }) => <MemoryRouter initialEntries={[initial]}>{children}</MemoryRouter>;
}

describe("useDashboardViewControls", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("parses ?refresh and ?fullscreen from the URL", () => {
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d?refresh=60&fullscreen=true") });
    expect(result.current.refreshSeconds).toBe(60);
    expect(result.current.isFullscreen).toBe(true);
  });

  it("treats a missing/invalid refresh as off", () => {
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d?refresh=abc") });
    expect(result.current.refreshSeconds).toBeNull();
    expect(result.current.isFullscreen).toBe(false);
  });

  it("ticks the refresh nonce on the interval", () => {
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d?refresh=1") });
    const start = result.current.refreshNonce;
    act(() => { vi.advanceTimersByTime(3000); });
    expect(result.current.refreshNonce).toBeGreaterThan(start);
  });
});
