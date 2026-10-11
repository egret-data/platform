import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "@/api/client";
import { useServerValidation } from "@/components/workflows/editor/useServerValidation";

const byId = vi.fn();
const unsaved = vi.fn();
vi.mock("@/api/generated/workflow/workflow", () => ({
  workflowValidateFlow: (...a: unknown[]) => byId(...a),
  workflowValidateUnsavedFlow: (...a: unknown[]) => unsaved(...a),
}));

const report = (errors: string[], warnings: string[] = []) => ({
  status: 200,
  data: {
    errors: errors.map((p) => ({ code: "e", message: `bad ${p}`, path: p })),
    warnings: warnings.map((p) => ({ code: "w", message: `odd ${p}`, path: p })),
  },
  headers: new Headers(),
});

/** A request that settles only when the test says so, and records its abort. */
function deferred() {
  let resolve!: (v: unknown) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const signalOf = (mock: typeof byId, call: number) =>
  (mock.mock.calls[call].at(-1) as { signal: AbortSignal }).signal;

beforeEach(() => {
  vi.useFakeTimers();
  byId.mockReset();
  unsaved.mockReset();
});
afterEach(() => vi.useRealTimers());

const flush = () => act(async () => { await Promise.resolve(); await Promise.resolve(); });

describe("useServerValidation", () => {
  it("waits 600 ms after the last edit before it calls the server", async () => {
    unsaved.mockResolvedValue(report([]));
    const { rerender } = renderHook(({ src }) => useServerValidation("ws1", src), {
      initialProps: { src: "id: a" },
    });
    await act(async () => { vi.advanceTimersByTime(599); });
    expect(unsaved).not.toHaveBeenCalled();
    rerender({ src: "id: ab" });
    await act(async () => { vi.advanceTimersByTime(599); });
    expect(unsaved).not.toHaveBeenCalled();
    await act(async () => { vi.advanceTimersByTime(1); });
    expect(unsaved).toHaveBeenCalledTimes(1);
    expect(unsaved.mock.calls[0][0]).toBe("ws1");
    expect(unsaved.mock.calls[0][1]).toEqual({ source: "id: ab" });
  });

  it("with a flow id calls validate-by-id, without one the id-less route", async () => {
    byId.mockResolvedValue(report([]));
    unsaved.mockResolvedValue(report([]));
    renderHook(() => useServerValidation("ws1", "id: a", "f-1"));
    await act(async () => { vi.advanceTimersByTime(600); });
    expect(byId).toHaveBeenCalledTimes(1);
    expect(byId.mock.calls[0].slice(0, 3)).toEqual(["ws1", "f-1", { source: "id: a" }]);
    expect(unsaved).not.toHaveBeenCalled();
  });

  it("reports errors and warnings with their severity once the request lands", async () => {
    unsaved.mockResolvedValue(report(["tasks.a"], ["id"]));
    const { result } = renderHook(() => useServerValidation("ws1", "id: a"));
    expect(result.current.status).toBe("validating");
    await act(async () => { vi.advanceTimersByTime(600); });
    await flush();
    expect(result.current.status).toBe("ok");
    expect(result.current.findings).toEqual([
      { severity: "error", code: "e", message: "bad tasks.a", path: "tasks.a" },
      { severity: "warning", code: "w", message: "odd id", path: "id" },
    ]);
  });

  it("aborts the older request when a newer one starts, and ignores its answer", async () => {
    const first = deferred();
    unsaved.mockReturnValueOnce(first.promise).mockResolvedValueOnce(report(["second"]));
    const { result, rerender } = renderHook(({ src }) => useServerValidation("ws1", src), {
      initialProps: { src: "v1" },
    });
    await act(async () => { vi.advanceTimersByTime(600); });
    expect(signalOf(unsaved, 0).aborted).toBe(false);
    rerender({ src: "v2" });
    await act(async () => { vi.advanceTimersByTime(600); });
    expect(signalOf(unsaved, 0).aborted).toBe(true);
    expect(signalOf(unsaved, 1).aborted).toBe(false);
    await flush();
    // The stale request answers late; its findings must not replace the newer ones.
    first.resolve(report(["first"]));
    await flush();
    expect(result.current.findings.map((f) => f.path)).toEqual(["second"]);
  });

  it("turns a 400 parse error into one finding at the line and column of the message", async () => {
    unsaved.mockRejectedValue(
      new ApiError(400, "invalid input: did not find expected key (line 3, column 7)"),
    );
    const { result } = renderHook(() => useServerValidation("ws1", "id: a"));
    await act(async () => { vi.advanceTimersByTime(600); });
    await flush();
    expect(result.current.status).toBe("ok");
    expect(result.current.error).toBeNull();
    expect(result.current.findings).toEqual([
      {
        severity: "error",
        code: "parse_error",
        message: "invalid input: did not find expected key (line 3, column 7)",
        path: "",
        at: { line: 3, col: 7 },
      },
    ]);
  });

  it("gives status error, not an empty list, when the request fails; retry runs it again", async () => {
    unsaved.mockRejectedValueOnce(new TypeError("Failed to fetch")).mockResolvedValueOnce(report(["x"]));
    const { result } = renderHook(() => useServerValidation("ws1", "id: a"));
    await act(async () => { vi.advanceTimersByTime(600); });
    await flush();
    expect(result.current.status).toBe("error");
    expect(result.current.error).toBe("Failed to fetch");
    await act(async () => { result.current.retry(); });
    await act(async () => { vi.advanceTimersByTime(0); });
    await flush();
    expect(unsaved).toHaveBeenCalledTimes(2);
    expect(result.current.status).toBe("ok");
    expect(result.current.findings.map((f) => f.path)).toEqual(["x"]);
  });

  it("a 403 is an error too, never findings", async () => {
    unsaved.mockRejectedValue(new ApiError(403, "forbidden"));
    const { result } = renderHook(() => useServerValidation("ws1", "id: a"));
    await act(async () => { vi.advanceTimersByTime(600); });
    await flush();
    expect(result.current.status).toBe("error");
    expect(result.current.findings).toEqual([]);
  });

  it("does nothing while disabled (a read-only viewer cannot call validate)", async () => {
    const { result } = renderHook(() => useServerValidation("ws1", "id: a", undefined, { enabled: false }));
    await act(async () => { vi.advanceTimersByTime(5000); });
    expect(unsaved).not.toHaveBeenCalled();
    expect(result.current.status).toBe("idle");
  });

  it("aborts the request in flight on unmount", async () => {
    unsaved.mockReturnValue(deferred().promise);
    const { unmount } = renderHook(() => useServerValidation("ws1", "id: a"));
    await act(async () => { vi.advanceTimersByTime(600); });
    unmount();
    expect(signalOf(unsaved, 0).aborted).toBe(true);
  });
});
