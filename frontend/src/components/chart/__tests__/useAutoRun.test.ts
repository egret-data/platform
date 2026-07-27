import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook } from "@testing-library/react";
import { useAutoRun } from "../useAutoRun";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("useAutoRun", () => {
  it("fires the callback after the debounce when enabled and signature changes", () => {
    const cb = vi.fn();
    const lastRunRef = { current: "" };
    const { rerender } = renderHook(
      ({ sig }) => useAutoRun({ enabled: true, signature: sig, delay: 500, onRun: cb, lastRunRef }),
      { initialProps: { sig: "a" } },
    );
    expect(cb).not.toHaveBeenCalled();
    vi.advanceTimersByTime(500);
    expect(cb).toHaveBeenCalledTimes(1);
    rerender({ sig: "b" });
    vi.advanceTimersByTime(500);
    expect(cb).toHaveBeenCalledTimes(2);
  });
  it("does not fire when disabled", () => {
    const cb = vi.fn();
    renderHook(() => useAutoRun({ enabled: false, signature: "a", delay: 500, onRun: cb, lastRunRef: { current: "" } }));
    vi.advanceTimersByTime(1000);
    expect(cb).not.toHaveBeenCalled();
  });
  it("does not fire on an empty signature", () => {
    const cb = vi.fn();
    renderHook(() => useAutoRun({ enabled: true, signature: "", delay: 500, onRun: cb, lastRunRef: { current: "" } }));
    vi.advanceTimersByTime(1000);
    expect(cb).not.toHaveBeenCalled();
  });
  it("does not fire if lastRunRef already equals the signature", () => {
    const cb = vi.fn();
    renderHook(() => useAutoRun({ enabled: true, signature: "a", delay: 500, onRun: cb, lastRunRef: { current: "a" } }));
    vi.advanceTimersByTime(1000);
    expect(cb).not.toHaveBeenCalled();
  });
});
