import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useChartRun } from "../useChartRun";
import { EMPTY_SPEC } from "@/api/charts";

const SPEC_A = { ...EMPTY_SPEC, dimensions: ["region"], metrics: ["total_revenue"] };
const SPEC_B = { ...EMPTY_SPEC, dimensions: ["category"], metrics: ["total_revenue"] };

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("useChartRun", () => {
  it("does not run while required fields are missing", () => {
    const { result } = renderHook(() =>
      useChartRun({ datasetId: "d1", autoRun: true, buildSpec: () => SPEC_A, missing: () => ["a metric"] }),
    );
    act(() => { vi.advanceTimersByTime(1000); });
    expect(result.current.ranSpec).toBeNull();
  });

  it("auto-runs after the debounce when the query signature changes", () => {
    const { result } = renderHook(() =>
      useChartRun({ datasetId: "d1", autoRun: true, buildSpec: () => SPEC_A, missing: () => [] }),
    );
    act(() => { vi.advanceTimersByTime(600); });
    expect(result.current.ranSpec).toEqual(SPEC_A);
  });

  it("does not auto-run when auto is off, and runs on demand", () => {
    const { result } = renderHook(() =>
      useChartRun({ datasetId: "d1", autoRun: false, buildSpec: () => SPEC_A, missing: () => [] }),
    );
    act(() => { vi.advanceTimersByTime(1000); });
    expect(result.current.ranSpec).toBeNull();
    act(() => { result.current.run(); });
    expect(result.current.ranSpec).toEqual(SPEC_A);
  });

  it("reports the query as stale once the spec changes without a run", () => {
    const { result, rerender } = renderHook(
      ({ spec }) => useChartRun({ datasetId: "d1", autoRun: false, buildSpec: () => spec, missing: () => [] }),
      { initialProps: { spec: SPEC_A } },
    );
    act(() => { result.current.run(); });
    expect(result.current.isStale).toBe(false);
    rerender({ spec: SPEC_B });
    expect(result.current.isStale).toBe(true);
    expect(result.current.ranSpec).toEqual(SPEC_A);
  });

  it("bumps the nonce on every run so an identical spec still refetches", () => {
    const { result } = renderHook(() =>
      useChartRun({ datasetId: "d1", autoRun: false, buildSpec: () => SPEC_A, missing: () => [] }),
    );
    act(() => { result.current.run(); });
    const first = result.current.runNonce;
    act(() => { result.current.run(); });
    expect(result.current.runNonce).toBe(first + 1);
  });
});
