import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { useEventSourceFollow } from "@/components/workflows/executions/useEventSourceFollow";

class FakeEventSource {
  static all: FakeEventSource[] = [];
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  constructor(public url: string) {
    FakeEventSource.all.push(this);
  }
  close() {
    this.closed = true;
  }
}

beforeEach(() => {
  FakeEventSource.all = [];
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => vi.unstubAllGlobals());

describe("useEventSourceFollow", () => {
  it("opens an EventSource and hands each parsed event to onEvent", () => {
    const onEvent = vi.fn();
    const { result } = renderHook(() => useEventSourceFollow<{ n: number }>("/f", { enabled: true, onEvent }));
    expect(FakeEventSource.all[0].url).toBe("/f");
    expect(result.current).toEqual({ live: true, fallback: false });
    act(() => FakeEventSource.all[0].onmessage!({ data: '{"n":1}' }));
    expect(onEvent).toHaveBeenCalledWith({ n: 1 });
  });

  it("ignores a malformed event", () => {
    const onEvent = vi.fn();
    renderHook(() => useEventSourceFollow("/f", { enabled: true, onEvent }));
    act(() => FakeEventSource.all[0].onmessage!({ data: "not json" }));
    expect(onEvent).not.toHaveBeenCalled();
  });

  it("closes on error and signals the fallback, without reconnecting", () => {
    const { result } = renderHook(() => useEventSourceFollow("/f", { enabled: true, onEvent: vi.fn() }));
    act(() => FakeEventSource.all[0].onerror!());
    expect(FakeEventSource.all[0].closed).toBe(true);
    expect(result.current).toEqual({ live: false, fallback: true });
    expect(FakeEventSource.all).toHaveLength(1);
  });

  it("falls back at once where EventSource does not exist", () => {
    vi.stubGlobal("EventSource", undefined);
    const { result } = renderHook(() => useEventSourceFollow("/f", { enabled: true, onEvent: vi.fn() }));
    expect(result.current.fallback).toBe(true);
  });

  it("opens nothing while disabled, and closes on unmount", () => {
    const off = renderHook(() => useEventSourceFollow("/f", { enabled: false, onEvent: vi.fn() }));
    expect(FakeEventSource.all).toHaveLength(0);
    expect(off.result.current).toEqual({ live: false, fallback: false });
    const on = renderHook(() => useEventSourceFollow("/g", { enabled: true, onEvent: vi.fn() }));
    on.unmount();
    expect(FakeEventSource.all[0].closed).toBe(true);
  });
});
