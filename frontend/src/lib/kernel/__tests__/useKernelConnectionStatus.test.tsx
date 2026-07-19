import { describe, it, expect } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useKernelConnectionStatus } from "../useKernelConnectionStatus";
import type { KernelStatus } from "../KernelConnection";

// Minimal fake matching the shape useKernelConnectionStatus depends on.
function makeFake(initial: KernelStatus) {
  let status = initial;
  const listeners = new Set<(s: KernelStatus) => void>();
  return {
    get status() { return status; },
    onStatusChange(cb: (s: KernelStatus) => void) { listeners.add(cb); cb(status); return () => listeners.delete(cb); },
    emit(s: KernelStatus) { status = s; listeners.forEach((cb) => cb(s)); },
  };
}

describe("useKernelConnectionStatus", () => {
  it("returns the current status and updates on change", () => {
    const fake = makeFake("disconnected");
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const { result } = renderHook(() => useKernelConnectionStatus(fake as any));
    expect(result.current).toBe("disconnected");
    act(() => fake.emit("connected"));
    expect(result.current).toBe("connected");
  });
});
