import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { TaskRunDto } from "@/api/generated/model";
import { LogView, type LogViewProps } from "@/components/workflows/executions/logs/LogView";
import { formatLogLine } from "@/components/workflows/executions/logs/useLogFollow";
import { EXEC, line, LogServer, WS } from "./logServer";

/**
 * jsdom has no layout: `scrollTop` is always 0 and ignores writes. This one
 * keeps what is written (unclamped), so the view's own scrolling, and a
 * user's, can be read back. `clientHeight` stays 0, so the view assumes its
 * 480 px default viewport.
 */
const scrollTops = new WeakMap<Element, number>();
const original = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTop")!;
beforeAll(() => {
  Object.defineProperty(Element.prototype, "scrollTop", {
    configurable: true,
    get() {
      return scrollTops.get(this) ?? 0;
    },
    set(v: number) {
      scrollTops.set(this, v);
    },
  });
});
afterAll(() => {
  Object.defineProperty(Element.prototype, "scrollTop", original);
});

function run(id: string, run_ix: number, task_id: string, attempts = 1): TaskRunDto {
  return {
    id,
    execution_id: EXEC,
    run_ix,
    task_ix: run_ix,
    task_id,
    state: "SUCCESS",
    history: [],
    attempts: Array.from({ length: attempts }, (_, i) => ({ history: ["RUNNING", "SUCCESS"], started_at: i })),
    outputs: {},
    created_at: "2026-10-10T10:00:00.000001Z",
    updated_at: "2026-10-10T10:00:01.000001Z",
  };
}

const RUNS = [run("r-1", 1, "load", 2), run("r-0", 0, "extract")];

function view(props: Partial<LogViewProps> = {}) {
  return render(<LogView ws={WS} executionId={EXEC} filters={{}} runs={RUNS} {...props} />);
}

const settle = () => act(() => vi.advanceTimersByTimeAsync(1));
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

/** The rows on screen, in DOM order: `# label` for a group header, the message for a line. */
function rowTexts(): string[] {
  const scroller = screen.getByTestId("log-scroller");
  return [...scroller.querySelectorAll<HTMLElement>("[data-testid=log-group], [data-testid=log-line]")].map((el) =>
    el.dataset.testid === "log-group"
      ? `# ${within(el).getByRole("button").textContent}`
      : el.querySelector("pre")!.textContent!,
  );
}

let server: LogServer;
beforeEach(() => {
  vi.useFakeTimers();
  server = new LogServer().install();
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("LogView: lines", () => {
  it("groups lines by task run in run order, the execution's own lines first, whatever order they came in", async () => {
    server.lines = [
      line(1, { task_id: "load", task_run_id: "r-1", attempt: 0, message: "load starts" }),
      line(2, { message: "execution says hi" }),
      line(3, { task_id: "extract", task_run_id: "r-0", attempt: 0, message: "extract runs" }),
      line(4, { task_id: "load", task_run_id: "r-1", attempt: 1, message: "load retries" }),
      line(5, { message: "execution says bye" }),
    ];
    view();
    await settle();
    expect(rowTexts()).toEqual([
      "# Execution2 lines",
      "execution says hi",
      "execution says bye",
      "# extract #01 line",
      "extract runs",
      "# load #12 lines",
      "load starts",
      "load retries",
    ]);
  });

  it("a level badge says its level in words", async () => {
    server.lines = [line(1, { level: "ERROR" }), line(2, { level: "WARN" }), line(3, { level: "TRACE" })];
    view({ grouped: false });
    await settle();
    expect(screen.getAllByTestId("log-level").map((b) => b.textContent)).toEqual(["ERROR", "WARN", "TRACE"]);
  });

  it("names the attempt only in a run that had more than one", async () => {
    server.lines = [
      line(1, { task_id: "extract", task_run_id: "r-0", attempt: 0, message: "once" }),
      line(2, { task_id: "load", task_run_id: "r-1", attempt: 0, message: "first try" }),
      line(3, { task_id: "load", task_run_id: "r-1", attempt: 1, message: "second try" }),
    ];
    view();
    await settle();
    const attemptOf = (message: string) =>
      within(screen.getByText(message).closest<HTMLElement>("[data-testid=log-line]")!).queryByTestId("log-attempt")?.textContent ?? null;
    expect(attemptOf("once")).toBeNull();
    expect(attemptOf("first try")).toBe("Attempt 1");
    expect(attemptOf("second try")).toBe("Attempt 2");
  });

  it("names attempts when the lines differ even if the run list does not say so", async () => {
    server.lines = [
      line(1, { task_id: "x", task_run_id: "r-9", attempt: 0, message: "a" }),
      line(2, { task_id: "x", task_run_id: "r-9", attempt: 2, message: "b" }),
    ];
    view({ grouped: false, runs: [] });
    await settle();
    expect(screen.getAllByTestId("log-attempt").map((a) => a.textContent)).toEqual(["Attempt 1", "Attempt 3"]);
  });

  it("keeps a message's whitespace as written", async () => {
    const message = "  indented\n\ttabbed   spaced\n\ntrailing  ";
    server.lines = [line(1, { message })];
    view({ grouped: false });
    await settle();
    const pre = screen.getByTestId("log-line").querySelector("pre")!;
    expect(pre.textContent).toBe(message);
    expect(pre).toHaveClass("whitespace-pre");
    // Four text lines, so the row is four lines tall: 4 × 20 + 4.
    expect(screen.getByTestId("log-line")).toHaveStyle({ height: "84px" });
  });

  it("collapses a group from its header, and expands it again", async () => {
    server.lines = [
      line(1, { task_id: "extract", task_run_id: "r-0", message: "extract runs" }),
      line(2, { task_id: "load", task_run_id: "r-1", message: "load runs" }),
    ];
    view();
    await settle();
    const header = screen.getByRole("button", { name: /extract #0/ });
    expect(header).toHaveAttribute("aria-expanded", "true");
    await act(async () => header.click());
    expect(screen.queryByText("extract runs")).toBeNull();
    expect(screen.getByText("load runs")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /extract #0/ })).toHaveAttribute("aria-expanded", "false");
    await act(async () => screen.getByRole("button", { name: "Collapse all" }).click());
    expect(screen.queryByText("load runs")).toBeNull();
    await act(async () => screen.getByRole("button", { name: "Expand all" }).click());
    expect(screen.getByText("extract runs")).toBeInTheDocument();
    expect(screen.getByText("load runs")).toBeInTheDocument();
  });

  it("renders only the rows near the viewport, not all ten thousand", async () => {
    server.lines = Array.from({ length: 10_000 }, (_, i) => line(i + 1));
    view({ grouped: false, pageSize: 2000, windowLines: 20_000 });
    for (let i = 0; i < 6; i++) await settle();
    const rendered = screen.getAllByTestId("log-line");
    expect(rendered.length).toBeGreaterThan(10);
    expect(rendered.length).toBeLessThan(100);
    // Following: it is at the bottom, so the last line is among them.
    expect(rendered.at(-1)!.querySelector("pre")!.textContent).toBe("line 10000");
    // The spacers stand in for the rest: the scroll height is every row's.
    const scroller = screen.getByTestId("log-scroller");
    const spacers = [scroller.firstElementChild as HTMLElement, scroller.lastElementChild as HTMLElement];
    const placed = rendered.length * 24 + spacers.reduce((n, s) => n + parseFloat(s.style.height), 0);
    expect(placed).toBe(10_000 * 24);
  });
});

describe("LogView: following and scrolling", () => {
  it("scrolls to new lines while the user is at the bottom", async () => {
    server.lines = Array.from({ length: 30 }, (_, i) => line(i + 1));
    view({ grouped: false });
    await settle();
    const scroller = screen.getByTestId("log-scroller");
    expect(scroller.scrollTop).toBe(30 * 24);
    server.lines.push(line(31), line(32));
    await advance(2000);
    expect(scroller.scrollTop).toBe(32 * 24);
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
  });

  it("stays put when the user has scrolled up, and offers Jump to latest", async () => {
    server.lines = Array.from({ length: 60 }, (_, i) => line(i + 1));
    view({ grouped: false });
    await settle();
    const scroller = screen.getByTestId("log-scroller");
    scroller.scrollTop = 100;
    fireEvent.scroll(scroller);
    const jump = screen.getByRole("button", { name: "Jump to latest" });
    server.lines.push(line(61), line(62));
    await advance(2000);
    expect(scroller.scrollTop).toBe(100);
    await act(async () => jump.click());
    expect(scroller.scrollTop).toBe(62 * 24);
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
    // And it follows again.
    server.lines.push(line(63));
    await advance(2000);
    expect(scroller.scrollTop).toBe(63 * 24);
  });

  it("Load earlier brings back lines set aside, and keeps the line being read in place", async () => {
    // Three pages of 30; the window holds one. 30 rows (720 px) overflow the 480 px viewport.
    server.lines = Array.from({ length: 90 }, (_, i) => line(i + 1));
    view({ grouped: false, pageSize: 30, windowLines: 30 });
    for (let i = 0; i < 4; i++) await settle();
    const scroller = screen.getByTestId("log-scroller");
    const toTop = () => {
      scroller.scrollTop = 0;
      fireEvent.scroll(scroller);
    };
    toTop();
    expect(rowTexts()[0]).toBe("line 61");
    // The user reads line 61, 10 px into it.
    scroller.scrollTop = 10;
    fireEvent.scroll(scroller);
    await act(async () => screen.getByRole("button", { name: "Load earlier" }).click());
    await settle();
    // Thirty rows of 24 px went in above it, and it has not moved on screen.
    expect(scroller.scrollTop).toBe(30 * 24 + 10);
    toTop();
    expect(rowTexts()[0]).toBe("line 31");
    await act(async () => screen.getByRole("button", { name: "Load earlier" }).click());
    await settle();
    toTop();
    expect(rowTexts()[0]).toBe("line 1");
    expect(screen.queryByRole("button", { name: "Load earlier" })).toBeNull();
  });
});

describe("LogView: states", () => {
  it("shows a skeleton until the first page", () => {
    view();
    expect(screen.getByTestId("logs-loading")).toBeInTheDocument();
  });

  it("says No logs yet while lines may still come", async () => {
    server.complete = false;
    server.state = "SUCCESS";
    view();
    await settle();
    expect(screen.getByText("No logs yet")).toBeInTheDocument();
  });

  it("says No logs once the execution is complete and nothing came", async () => {
    server.complete = true;
    server.state = "SUCCESS";
    view();
    await settle();
    expect(screen.getByText("No logs")).toBeInTheDocument();
    expect(screen.queryByText("No logs yet")).toBeNull();
  });

  it("a filter that matches nothing says so and offers to clear it", async () => {
    server.complete = true;
    const onClearFilters = vi.fn();
    view({ filters: { minLevel: "ERROR" }, filtersActive: true, onClearFilters });
    await settle();
    expect(screen.getByText("No lines match the filters")).toBeInTheDocument();
    await act(async () => screen.getByRole("button", { name: "Clear filters" }).click());
    expect(onClearFilters).toHaveBeenCalled();
  });

  it("a failed first read is an error with Retry, not empty", async () => {
    server.failures = 1;
    server.lines = [line(1, { message: "finally" })];
    view();
    await settle();
    expect(screen.getByText(/The logs could not be loaded: the database is unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("No logs yet")).toBeNull();
    await act(async () => screen.getByRole("button", { name: "Retry" }).click());
    await settle();
    expect(screen.getByText("finally")).toBeInTheDocument();
  });

  it("a failed poll keeps the lines on screen and says so", async () => {
    server.lines = [line(1, { message: "kept" })];
    view({ grouped: false });
    await settle();
    server.failures = 1;
    await advance(2000);
    expect(screen.getByText("kept")).toBeInTheDocument();
    expect(screen.getByText(/The logs could not be refreshed: the database is unavailable/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});

describe("LogView: Download", () => {
  function captureSaves() {
    const saved: { name: string; blob: Blob }[] = [];
    let last: Blob | null = null;
    vi.stubGlobal("URL", Object.assign(Object.create(URL), {
      createObjectURL: (b: Blob) => {
        last = b;
        return "blob:log";
      },
      revokeObjectURL: () => {},
    }));
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      saved.push({ name: this.download, blob: last! });
    });
    return saved;
  }

  it("reads every page with the current filters and saves the lines as a .log file", async () => {
    vi.useRealTimers();
    server.lines = [
      line(1, { level: "INFO", task_run_id: "r-0", task_id: "extract", message: "skip me" }),
      line(2, { level: "WARN", task_run_id: "r-0", task_id: "extract", message: "keep me" }),
      line(3, { level: "ERROR", task_run_id: "r-1", task_id: "load", message: "other run" }),
      line(4, { level: "ERROR", task_run_id: "r-0", task_id: "extract", message: "and me" }),
    ];
    server.complete = true;
    const saved = captureSaves();
    view({ grouped: false, filters: { minLevel: "WARN", taskRunId: "r-0" }, downloadPageSize: 1 });
    await screen.findByText("keep me");
    const before = server.queries.length;
    await userEvent.click(screen.getByRole("button", { name: "Download" }));
    await vi.waitFor(() => expect(saved).toHaveLength(1));
    expect(saved[0].name).toBe(`${EXEC}.log`);
    expect(await saved[0].blob.text()).toBe(`${formatLogLine(server.lines[1])}\n${formatLogLine(server.lines[3])}\n`);
    const reads = server.queries.slice(before);
    // Page by page: none, 2, 4, then the empty page that ends it.
    expect(reads.map((q) => q.get("after"))).toEqual([null, "2", "4"]);
    expect(reads.every((q) => q.get("min_level") === "WARN" && q.get("task_run_id") === "r-0")).toBe(true);
    expect(screen.queryByText(/The file holds the first/)).toBeNull();
  });

  it("says so when the file stops at the cap", async () => {
    vi.useRealTimers();
    server.lines = Array.from({ length: 5 }, (_, i) => line(i + 1));
    server.complete = true;
    const saved = captureSaves();
    view({ grouped: false, downloadMaxLines: 3 });
    await screen.findByText("line 5");
    await userEvent.click(screen.getByRole("button", { name: "Download" }));
    await vi.waitFor(() => expect(saved).toHaveLength(1));
    expect((await saved[0].blob.text()).trimEnd().split("\n")).toHaveLength(3);
    expect(screen.getByRole("status")).toHaveTextContent("The file holds the first 3 lines; the execution has more.");
  });

  it("a failed download says what happened", async () => {
    vi.useRealTimers();
    server.lines = [line(1)];
    server.complete = true;
    captureSaves();
    view({ grouped: false });
    await screen.findByText("line 1");
    server.failures = 1;
    await userEvent.click(screen.getByRole("button", { name: "Download" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The logs could not be downloaded: the database is unavailable. Try again.");
  });
});
