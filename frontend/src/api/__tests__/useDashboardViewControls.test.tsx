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

  it("a manual refresh bumps the nonce AND moves the countdown origin", () => {
    // Two things have to move together. A nonce without an origin leaves the countdown showing
    // time to a refresh that already happened; an origin without a nonce restarts the clock
    // without re-running a single card query.
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d") });
    const nonce = result.current.refreshNonce;
    const at = result.current.refreshedAt;
    act(() => { vi.advanceTimersByTime(5000); result.current.refreshNow(); });
    expect(result.current.refreshNonce).toBe(nonce + 1);
    expect(result.current.refreshedAt).toBe(at + 5000);
  });

  it("the interval refresh moves the origin too, so the ring restarts", () => {
    const { result } = renderHook(() => useDashboardViewControls(), { wrapper: wrap("/d?refresh=1") });
    const at = result.current.refreshedAt;
    act(() => { vi.advanceTimersByTime(1000); });
    expect(result.current.refreshNonce).toBe(1);
    expect(result.current.refreshedAt).toBe(at + 1000);
  });
});
