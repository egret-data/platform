import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { AFTER_RUN_FOLLOW_MS } from "@/components/workflows/executionState";
import { downloadLogs, formatLogLine, useLogFollow, type LogFilters } from "@/components/workflows/executions/logs/useLogFollow";
import { EXEC, line, LogServer, page, WS } from "./logServer";

/** Let the read in flight answer, and run whatever it scheduled at once. */
const settle = () => act(() => vi.advanceTimersByTimeAsync(1));
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

function follow(filters: LogFilters = {}, options = {}) {
  return renderHook(({ f }) => useLogFollow(WS, EXEC, f, options), { initialProps: { f: filters } });
}

const ids = (r: { current: { lines: { id: number }[] } }) => r.current.lines.map((l) => l.id);

let visibility: DocumentVisibilityState = "visible";

beforeEach(() => {
  vi.useFakeTimers();
  visibility = "visible";
  vi.spyOn(document, "visibilityState", "get").mockImplementation(() => visibility);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("useLogFollow: the cursor", () => {
  it("sends no `after` first, then each `next_after`; a null `next_after` keeps the cursor it had", async () => {
    const server = new LogServer().install();
    server.replies = [
      page([line(3), line(7)]),
      // Nothing new: `next_after` is null, the cursor stays at 7.
      page([]),
      page([line(9)]),
      page([]),
    ];
    const { result } = follow();
    await settle();
    expect(ids(result)).toEqual([3, 7]);
    await advance(2000);
    await advance(2000);
    await advance(2000);
    expect(server.afters()).toEqual([null, 7, 7, 9]);
    expect(ids(result)).toEqual([3, 7, 9]);
  });

  it("sends the filters, upper-case level, and never the string \"null\" for an unset one", async () => {
    const server = new LogServer().install();
    follow({ minLevel: "WARN", taskRunId: "r-1" });
    await settle();
    const q = server.queries[0];
    expect(q.get("min_level")).toBe("WARN");
    expect(q.get("task_run_id")).toBe("r-1");
    expect(q.get("limit")).toBe("500");
    expect(q.has("task_id")).toBe(false);
    expect(q.has("after")).toBe(false);
    expect(q.toString()).not.toContain("null");
  });

  it("a filter change starts again from the beginning, and drops the other filter's lines", async () => {
    const server = new LogServer().install();
    server.lines = [line(1), line(2, { level: "WARN" }), line(3)];
    const { result, rerender } = follow();
    await settle();
    expect(ids(result)).toEqual([1, 2, 3]);
    rerender({ f: { minLevel: "WARN" } });
    // Until the new read answers, the old filter's lines are not shown as the new one's.
    expect(result.current.lines).toEqual([]);
    expect(result.current.loading).toBe(true);
    await settle();
    expect(ids(result)).toEqual([2]);
    const last = server.queries[server.queries.length - 1];
    expect(last.has("after")).toBe(false);
    expect(last.get("min_level")).toBe("WARN");
  });
});

describe("useLogFollow: when it reads", () => {
  it("reads every 2 s while more may come", async () => {
    const server = new LogServer().install();
    server.lines = [line(1)];
    follow();
    await settle();
    expect(server.queries).toHaveLength(1);
    await advance(1990);
    expect(server.queries).toHaveLength(1);
    await advance(20);
    expect(server.queries).toHaveLength(2);
    await advance(2000);
    expect(server.queries).toHaveLength(3);
  });

  it("reads the next page at once when a page comes back full", async () => {
    const server = new LogServer().install();
    server.lines = Array.from({ length: 7 }, (_, i) => line(i + 1));
    const { result } = follow({}, { pageSize: 3 });
    await settle();
    await settle();
    await settle();
    // 3 + 3 + 1: the third page is short, so the fourth waits 2 s.
    expect(ids(result)).toEqual([1, 2, 3, 4, 5, 6, 7]);
    expect(server.queries).toHaveLength(3);
  });

  it("stops only on `complete` with no items", async () => {
    const server = new LogServer().install();
    server.replies = [
      // Complete, but this read still brought lines: there may be more.
      page([line(1)], { execution_state: "SUCCESS", complete: true }),
      page([], { execution_state: "SUCCESS", complete: true }),
    ];
    const { result } = follow();
    await settle();
    expect(result.current.done).toBe(false);
    await advance(2000);
    expect(server.queries).toHaveLength(2);
    expect(result.current.done).toBe(true);
    await advance(30_000);
    expect(server.queries).toHaveLength(2);
  });

  describe("the bound on an execution that never becomes idle (complete stays false)", () => {
    const ENDED = "2026-10-10T10:00:00Z";
    const endedAgo = (ms: number) => new Date(Date.now() - ms).toISOString();
    const idle = () => page([], { execution_state: "SUCCESS", complete: false });

    it("keeps reading every 2 s inside the follow window after the execution ended", async () => {
      const server = new LogServer().install();
      server.replies = [idle()];
      const { result } = follow({}, { endedAt: endedAgo(60_000) });
      await settle();
      await advance(10_000);
      expect(server.queries.length).toBeGreaterThanOrEqual(5);
      expect(result.current.done).toBe(false);
    });

    it("stops once the execution has been terminal for the whole window", async () => {
      const server = new LogServer().install();
      server.replies = [idle()];
      const { result } = follow({}, { endedAt: endedAgo(AFTER_RUN_FOLLOW_MS + 1000) });
      await settle();
      expect(result.current.done).toBe(true);
      await advance(30_000);
      expect(server.queries).toHaveLength(1);
    });

    it("does not stop while the page still says the execution is running", async () => {
      const server = new LogServer().install();
      server.replies = [page([], { execution_state: "RUNNING", complete: false })];
      follow({}, { endedAt: ENDED });
      await settle();
      await advance(6000);
      expect(server.queries.length).toBeGreaterThanOrEqual(3);
    });

    it("still stops on complete with no items, as before", async () => {
      const server = new LogServer().install();
      server.replies = [page([], { execution_state: "SUCCESS", complete: true })];
      const { result } = follow({}, { endedAt: endedAgo(1000) });
      await settle();
      expect(result.current.done).toBe(true);
      await advance(30_000);
      expect(server.queries).toHaveLength(1);
    });
  });

  it("does not stop on a terminal state alone: the reader lags the writer", async () => {
    const server = new LogServer().install();
    server.replies = [
      page([], { execution_state: "SUCCESS", complete: false }),
      page([], { execution_state: "FAILED", complete: false }),
      page([line(4)], { execution_state: "FAILED", complete: false }),
      page([], { execution_state: "FAILED", complete: true }),
    ];
    const { result } = follow();
    await settle();
    await advance(2000);
    expect(result.current.done).toBe(false);
    await advance(2000);
    expect(ids(result)).toEqual([4]);
    await advance(2000);
    expect(server.queries).toHaveLength(4);
    expect(result.current.done).toBe(true);
    expect(result.current.executionState).toBe("FAILED");
  });

  it("reads nothing while the tab is hidden, and catches up when it shows again", async () => {
    const server = new LogServer().install();
    server.lines = [line(1)];
    follow();
    await settle();
    visibility = "hidden";
    await advance(10_000);
    expect(server.queries).toHaveLength(1);
    visibility = "visible";
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await settle();
    expect(server.queries).toHaveLength(2);
  });
});

describe("useLogFollow: failures", () => {
  it("a failed read keeps the lines, exposes the error, and Retry reads again at once", async () => {
    const server = new LogServer().install();
    server.lines = [line(1), line(2)];
    const { result } = follow();
    await settle();
    server.failures = 1;
    await advance(2000);
    expect(ids(result)).toEqual([1, 2]);
    expect(String(result.current.error)).toContain("the database is unavailable");
    server.lines.push(line(3));
    const before = server.queries.length;
    await act(async () => result.current.retry());
    await settle();
    expect(server.queries.length).toBe(before + 1);
    expect(server.afters().at(-1)).toBe(2);
    expect(result.current.error).toBeNull();
    expect(ids(result)).toEqual([1, 2, 3]);
  });

  it("keeps trying on its own after a failure, backing off", async () => {
    const server = new LogServer().install();
    server.lines = [line(1)];
    follow();
    await settle();
    server.failures = 2;
    await advance(2000); // fails
    expect(server.queries).toHaveLength(2);
    await advance(2000); // fails again; the next wait is longer than 2 s
    expect(server.queries).toHaveLength(3);
    await advance(2000);
    expect(server.queries).toHaveLength(3);
    // 4 s after the second failure, the time since the last good read.
    await advance(2000);
    expect(server.queries).toHaveLength(4);
  });

  it("a failed first read stops and waits for Retry", async () => {
    const server = new LogServer().install();
    server.failures = 1;
    server.lines = [line(5)];
    const { result } = follow();
    await settle();
    expect(result.current.error).not.toBeNull();
    expect(result.current.loading).toBe(false);
    await advance(60_000);
    expect(server.queries).toHaveLength(1);
    await act(async () => result.current.retry());
    await settle();
    expect(ids(result)).toEqual([5]);
  });
});

describe("useLogFollow: the window and Load earlier", () => {
  it("sets the oldest pages aside past the window, and Load earlier reads them back in order", async () => {
    const server = new LogServer().install();
    server.lines = Array.from({ length: 10 }, (_, i) => line(i + 1));
    const { result } = follow({}, { pageSize: 3, windowLines: 4 });
    for (let i = 0; i < 4; i++) await settle();
    // Pages [1-3] [4-6] [7-9] [10]: only the newest that fit in 4 lines (and at least one page) stay.
    expect(ids(result)).toEqual([7, 8, 9, 10]);
    expect(result.current.hasEarlier).toBe(true);
    await act(async () => result.current.loadEarlier());
    await settle();
    expect(ids(result)).toEqual([4, 5, 6, 7, 8, 9, 10]);
    // Re-read from the dropped page's own cursor.
    expect(server.afters().at(-1)).toBe(3);
    await act(async () => result.current.loadEarlier());
    await settle();
    expect(ids(result)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    expect(server.afters().at(-1)).toBeNull();
    expect(result.current.hasEarlier).toBe(false);
    // Following on does not set aside what the user brought back.
    server.lines.push(line(11));
    await advance(2000);
    expect(ids(result)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
  });
  it("a page that came back short is read back without repeating the lines after it", async () => {
    const server = new LogServer().install();
    // The first read sees only two lines (the rest had not settled); the next two pages are full.
    server.lines = [line(1), line(2)];
    const { result } = follow({}, { pageSize: 3, windowLines: 3 });
    await settle();
    server.lines.push(...[3, 4, 5, 6, 7, 8].map((i) => line(i)));
    await advance(2000);
    await settle();
    expect(ids(result)).toEqual([6, 7, 8]);
    await act(async () => result.current.loadEarlier());
    await settle();
    await act(async () => result.current.loadEarlier());
    await settle();
    // Re-read from the start, the page now runs on to line 3, which is already held.
    expect(ids(result)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
  });
});

describe("useLogFollow: Load earlier against a poll", () => {
  it("a poll that lands while Load earlier is in flight drops no page (review I1)", async () => {
    const server = new LogServer().install();
    server.lines = Array.from({ length: 9 }, (_, i) => line(i + 1));
    const { result } = follow({}, { pageSize: 3, windowLines: 3 });
    for (let i = 0; i < 4; i++) await settle();
    expect(ids(result)).toEqual([7, 8, 9]);
    // Hold the Load earlier read until a poll has run.
    const served = globalThis.fetch as unknown as (...a: unknown[]) => Promise<Response>;
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    let first = true;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (...a: unknown[]) => {
        if (first) {
          first = false;
          await gate;
        }
        return served(...a);
      }),
    );
    await act(async () => result.current.loadEarlier());
    server.lines.push(line(10), line(11), line(12));
    await advance(2000);
    await settle();
    release();
    await settle();
    await settle();
    expect(ids(result)).toEqual([4, 5, 6, 7, 8, 9, 10, 11, 12]);
    // The one page still set aside is the first, and it reads back whole.
    expect(result.current.hasEarlier).toBe(true);
    await act(async () => result.current.loadEarlier());
    await settle();
    expect(ids(result)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    expect(result.current.hasEarlier).toBe(false);
  });

  it("followLatest holds the window again after Load earlier, and Load earlier still reaches what it sets aside", async () => {
    const server = new LogServer().install();
    server.lines = Array.from({ length: 9 }, (_, i) => line(i + 1));
    const { result } = follow({}, { pageSize: 3, windowLines: 3 });
    for (let i = 0; i < 4; i++) await settle();
    await act(async () => result.current.loadEarlier());
    await settle();
    expect(ids(result)).toEqual([4, 5, 6, 7, 8, 9]);
    await act(async () => result.current.followLatest());
    expect(ids(result)).toEqual([7, 8, 9]);
    server.lines.push(line(10), line(11), line(12));
    await advance(2000);
    expect(ids(result)).toEqual([10, 11, 12]);
    await act(async () => result.current.loadEarlier());
    await settle();
    expect(ids(result)).toEqual([7, 8, 9, 10, 11, 12]);
  });
});

describe("downloadLogs", () => {
  it("reads every page with the current filters, and writes one line per log line", async () => {
    vi.useRealTimers();
    const server = new LogServer().install();
    server.lines = [
      line(1, { message: "start" }),
      line(2, { level: "ERROR", task_id: "extract", task_run_id: "r-0", attempt: 1, message: "boom" }),
      line(3, { level: "DEBUG" }),
      line(4, { level: "WARN", message: "late" }),
    ];
    const file = await downloadLogs(WS, EXEC, { minLevel: "WARN" }, { pageSize: 1 });
    expect(file.capped).toBe(false);
    expect(file.lines).toBe(2);
    expect(file.text).toBe(`${formatLogLine(server.lines[1])}\n${formatLogLine(server.lines[3])}\n`);
    expect(file.text).toContain("ERROR [extract attempt 2] boom");
    // Every read carried the filter, and the cursor walked: none, 2, 4.
    expect(server.queries.every((q) => q.get("min_level") === "WARN")).toBe(true);
    expect(server.afters()).toEqual([null, 2, 4]);
  });

  it("stops reading when its signal is aborted", async () => {
    vi.useRealTimers();
    const server = new LogServer().install();
    server.lines = Array.from({ length: 6 }, (_, i) => line(i + 1));
    const abort = new AbortController();
    abort.abort();
    await expect(downloadLogs(WS, EXEC, {}, { pageSize: 2, signal: abort.signal })).rejects.toThrow();
    expect(server.queries.length).toBeLessThanOrEqual(1);
  });

  it("stops at the cap and says so", async () => {
    vi.useRealTimers();
    const server = new LogServer().install();
    server.lines = Array.from({ length: 5 }, (_, i) => line(i + 1));
    const file = await downloadLogs(WS, EXEC, {}, { pageSize: 2, maxLines: 3 });
    expect(file.capped).toBe(true);
    expect(file.lines).toBe(3);
    expect(file.text.trimEnd().split("\n")).toHaveLength(3);
  });

  it("is not capped when the log has exactly the cap", async () => {
    vi.useRealTimers();
    const server = new LogServer().install();
    server.lines = Array.from({ length: 3 }, (_, i) => line(i + 1));
    const file = await downloadLogs(WS, EXEC, {}, { pageSize: 2, maxLines: 3 });
    expect(file.capped).toBe(false);
    expect(file.lines).toBe(3);
  });
});
