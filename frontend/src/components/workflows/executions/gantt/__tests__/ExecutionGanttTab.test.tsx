import { Profiler, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import type { ExecutionDto, TaskRunDto } from "@/api/generated/model";
import ExecutionGanttTab from "@/components/workflows/executions/gantt/ExecutionGanttTab";

/** 10:00:00 to 10:01:40: a hundred seconds, so a second is one percent of the axis. */
const T0 = "2026-10-11T10:00:00Z";
const at = (s: number) => new Date(Date.parse(T0) + s * 1000).toISOString();

function run(run_ix: number, task_id: string, over: Partial<TaskRunDto> = {}): TaskRunDto {
  return {
    id: `r-${run_ix}`,
    execution_id: "e",
    run_ix,
    task_ix: run_ix,
    task_id,
    parent_ix: null,
    value: null,
    state: "SUCCESS",
    history: ["CREATED", "RUNNING", "SUCCESS"],
    attempts: [],
    outputs: {},
    created_seq: run_ix,
    ended_seq: run_ix + 1,
    started_at: null,
    ended_at: null,
    created_at: at(0),
    updated_at: at(99),
    ...over,
  };
}

// Unsorted, as FE-TEST-1 asks: the rows must come out in tree order whatever the input order.
const RUNS: TaskRunDto[] = [
  run(2, "load", { started_at: at(50), ended_at: at(100) }),
  run(5, "old", { attempts: [{ started_at: 30_000, history: ["RUNNING", "SUCCESS"] }], updated_at: at(40) }),
  run(3, "inner", { parent_ix: 1, value: "eu-west", started_at: at(26), ended_at: at(40) }),
  run(0, "extract", {
    started_at: at(0),
    ended_at: at(25),
    history: ["CREATED", "RUNNING", "FAILED", "RETRYING", "RUNNING", "SUCCESS"],
    attempts: [
      { started_at: 0, ended_at: 10_000, history: ["RUNNING", "FAILED"] },
      { started_at: 15_000, ended_at: 25_000, history: ["RUNNING", "SUCCESS"] },
    ],
    message: "retried once after a timeout",
  }),
  run(4, "skipped", { state: "SKIPPED", history: ["CREATED", "SKIPPED"], ended_at: at(60) }),
  run(1, "each", { type_id: null, started_at: at(25), ended_at: at(45) }),
];

function execution(over: Partial<ExecutionDto> = {}): ExecutionDto {
  return {
    id: "e",
    flow_id: "f",
    namespace: "company.team",
    flow_key: "hello",
    state: "SUCCESS",
    history: ["CREATED", "RUNNING", "SUCCESS"],
    inputs: {},
    seq: 1,
    execution_trigger: null,
    attempt_number: 1,
    origin_offset_ms: 0,
    replayed_from_execution_id: null,
    parent_execution_id: null,
    trigger_execution_id: null,
    loop_index: null,
    loop_value: null,
    created_at: T0,
    started_at: T0,
    ended_at: at(100),
    updated_at: at(130),
    source: "",
    runs: RUNS,
    can_manage: true,
    ...over,
  } as ExecutionDto;
}

function Where() {
  const loc = useLocation();
  const navigate = useNavigate();
  return (
    <>
      <div data-testid="where">{loc.pathname + loc.search}</div>
      <button type="button" onClick={() => navigate(-1)}>
        Back
      </button>
    </>
  );
}

type Slots = Parameters<typeof ExecutionGanttTab>[0]["slots"];
/** The slots are required; a test that is not about them renders nothing in them. Module-level, so stable. */
const NO_SLOTS: Slots = { logs: () => null, outputs: () => null };

function renderTab(exec: ExecutionDto, url = "/gantt", slots: Partial<Slots> = {}) {
  return render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route
          path="/gantt"
          element={
            <>
              <ExecutionGanttTab execution={exec} slots={{ ...NO_SLOTS, ...slots }} />
              <Where />
            </>
          }
        />
      </Routes>
    </MemoryRouter>,
  );
}

const bodyRows = () => within(screen.getByRole("treegrid", { name: "Task runs over time" })).getAllByRole("row").slice(1);
const rowFor = (ix: number) => bodyRows().find((r) => r.getAttribute("data-run-ix") === String(ix))!;

afterEach(() => {
  vi.useRealTimers();
});

describe("ExecutionGanttTab: rows", () => {
  it("draws one row per run, depth-first and by run_ix, each saying its state in words", () => {
    renderTab(execution());
    expect(bodyRows().map((r) => r.getAttribute("data-run-ix"))).toEqual(["0", "1", "3", "2", "4", "5"]);
    expect(within(rowFor(4)).getByText("Skipped")).toBeInTheDocument();
    expect(within(rowFor(2)).getByText("Success")).toBeInTheDocument();
    // A loop's child is labelled with its value.
    expect(within(rowFor(3)).getByText("eu-west")).toBeInTheDocument();
    expect(within(rowFor(0)).getByText("2 attempts")).toBeInTheDocument();
  });

  it("places bars on the axis from started_at and ended_at", () => {
    renderTab(execution());
    const load = screen.getByTestId("gantt-bar-2");
    expect(load.style.left).toBe("50%");
    expect(load.style.width).toBe("50%");
    const extract = screen.getByTestId("gantt-bar-0");
    expect(extract.style.left).toBe("0%");
    expect(extract.style.width).toBe("25%");
  });

  it("draws each attempt as its own segment, the retried one in its own state", () => {
    renderTab(execution());
    const first = screen.getByTestId("gantt-attempt-0-0");
    const second = screen.getByTestId("gantt-attempt-0-1");
    expect([first.style.left, first.style.width, first.dataset.state]).toEqual(["0%", "10%", "FAILED"]);
    expect([second.style.left, second.style.width, second.dataset.state]).toEqual(["15%", "10%", "SUCCESS"]);
    expect(first.className).toContain("bg-destructive");
    expect(second.className).toContain("bg-success");
    // In words too, for a screen reader.
    expect(within(rowFor(0)).getByText(/Attempt 1: Failed, 10 s\. Attempt 2: Success, 10 s\./)).toBeInTheDocument();
  });

  it("marks a run that never ran instead of drawing it", () => {
    renderTab(execution());
    expect(screen.queryByTestId("gantt-bar-4")).toBeNull();
    expect(screen.getByTestId("gantt-marker-4").style.left).toBe("60%");
    expect(within(rowFor(4)).getByText("Skipped: it never ran.")).toBeInTheDocument();
  });

  it("hatches an estimated bar, labels it, and puts the note at the top", () => {
    renderTab(execution());
    const old = screen.getByTestId("gantt-bar-5");
    expect(old.dataset.estimated).toBe("true");
    expect(old.style.backgroundImage).toContain("repeating-linear-gradient");
    expect(old.style.backgroundImage).toContain("var(--success)");
    expect([old.style.left, old.style.width]).toEqual(["30%", "10%"]);
    expect(within(rowFor(5)).getByText("estimated")).toBeInTheDocument();
    expect(screen.getByTestId("gantt-estimated-note")).toHaveTextContent("Estimated bars are hatched.");
    // A recorded bar is not hatched.
    expect(screen.getByTestId("gantt-bar-2").dataset.estimated).toBeUndefined();
  });

  it("has no note when every time was recorded", () => {
    renderTab(execution({ runs: RUNS.filter((r) => r.run_ix !== 5) }));
    expect(screen.queryByTestId("gantt-estimated-note")).toBeNull();
  });

  it("labels the axis with elapsed-time ticks", () => {
    renderTab(execution());
    const labels = screen.getAllByTestId("gantt-tick").map((t) => t.textContent);
    expect(labels[0]).toBe("0 s");
    expect(labels).toContain("1 min");
  });

  it("says when there are no task runs", () => {
    renderTab(execution({ runs: [], state: "FAILED" }));
    expect(screen.getByText("No task runs yet")).toBeInTheDocument();
    expect(screen.getByText("The execution ended before any task ran.")).toBeInTheDocument();
  });
});

describe("ExecutionGanttTab: collapsing", () => {
  it("hides a row's descendants with its toggle, and shows them again", async () => {
    const user = userEvent.setup();
    renderTab(execution());
    await user.click(screen.getByRole("button", { name: "Collapse each" }));
    expect(bodyRows().map((r) => r.getAttribute("data-run-ix"))).toEqual(["0", "1", "2", "4", "5"]);
    await user.click(screen.getByRole("button", { name: "Expand each" }));
    expect(bodyRows().map((r) => r.getAttribute("data-run-ix"))).toEqual(["0", "1", "3", "2", "4", "5"]);
    // Toggling selects nothing.
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/gantt$/);
  });

  it("collapses with Left and expands with Right", async () => {
    const user = userEvent.setup();
    renderTab(execution());
    await user.tab();
    await user.keyboard("{ArrowDown}{ArrowLeft}");
    expect(bodyRows()).toHaveLength(5);
    await user.keyboard("{ArrowRight}");
    expect(bodyRows()).toHaveLength(6);
    // Left on a child goes to its parent.
    await user.keyboard("{ArrowDown}{ArrowLeft}");
    expect(rowFor(1)).toHaveFocus();
  });
});

describe("ExecutionGanttTab: selection", () => {
  it("is one tab stop; arrows move, Enter opens the run in ?run=, Escape closes it", async () => {
    const user = userEvent.setup();
    renderTab(execution());
    await user.tab();
    expect(rowFor(0)).toHaveFocus();
    expect(bodyRows().filter((r) => r.tabIndex === 0)).toHaveLength(1);
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}");
    expect(rowFor(2)).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(screen.getByTestId("where")).toHaveTextContent("/gantt?run=2");
    expect(rowFor(2)).toHaveAttribute("aria-selected", "true");
    const panel = screen.getByTestId("gantt-run-panel");
    expect(within(panel).getByRole("heading", { name: "load" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/gantt$/);
    expect(screen.queryByTestId("gantt-run-panel")).toBeNull();
    expect(rowFor(2)).toHaveFocus();
    await user.keyboard("{End}");
    expect(rowFor(5)).toHaveFocus();
    await user.keyboard("{Home}");
    expect(rowFor(0)).toHaveFocus();
    await user.keyboard(" ");
    expect(screen.getByTestId("where")).toHaveTextContent("/gantt?run=0");
  });

  it("opens the run named in the URL, with its attempts, message and the logs and outputs slots", () => {
    renderTab(execution(), "/gantt?run=0");
    const panel = screen.getByTestId("gantt-run-panel");
    expect(within(panel).getByRole("heading", { name: "extract" })).toBeInTheDocument();
    const attempts = within(panel).getByRole("list", { name: "Attempts" });
    expect(within(attempts).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["Attempt 1Failed10 s", "Attempt 2Success10 s"]);
    expect(within(panel).getByText("retried once after a timeout")).toBeInTheDocument();
    expect(within(panel).getByRole("region", { name: "Logs" })).toBeInTheDocument();
    expect(within(panel).getByRole("region", { name: "Outputs" })).toBeInTheDocument();
    expect(rowFor(0)).toHaveAttribute("aria-selected", "true");
    expect(rowFor(0).tabIndex).toBe(0);
  });

  it("hands the selected run to the logs and outputs slots", () => {
    renderTab(execution(), "/gantt?run=3", {
      logs: (r) => <p>logs of {r.id}</p>,
      outputs: (r) => <p>outputs of {r.task_id}</p>,
    });
    expect(screen.getByRole("region", { name: "Logs" })).toHaveTextContent("logs of r-3");
    expect(screen.getByRole("region", { name: "Outputs" })).toHaveTextContent("outputs of inner");
  });

  it("selects on click, closes on a second click and with the close button", async () => {
    const user = userEvent.setup();
    renderTab(execution());
    await user.click(within(rowFor(4)).getByText("skipped"));
    expect(screen.getByTestId("where")).toHaveTextContent("/gantt?run=4");
    expect(within(screen.getByTestId("gantt-run-panel")).getByText("Skipped: it never ran.")).toBeInTheDocument();
    await user.click(within(rowFor(4)).getByText("skipped"));
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/gantt$/);
    await user.click(within(rowFor(2)).getByText("load"));
    await user.click(screen.getByRole("button", { name: "Close the task run" }));
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/gantt$/);
  });

  it("says so when ?run= names no run of this execution", async () => {
    const user = userEvent.setup();
    renderTab(execution(), "/gantt?run=99");
    expect(screen.getByText(/Task run #99 is not in this execution/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/gantt$/);
  });
});

describe("ExecutionGanttTab: tree semantics", () => {
  it("is a treegrid whose rows say their level, place among siblings, and expansion", async () => {
    const user = userEvent.setup();
    renderTab(execution());
    const attrs = (ix: number) => {
      const r = rowFor(ix);
      return [r.getAttribute("aria-level"), r.getAttribute("aria-posinset"), r.getAttribute("aria-setsize"), r.getAttribute("aria-expanded")];
    };
    // Roots 0, 1, 2, 4, 5; `inner` (3) is the only child of `each` (1).
    expect(attrs(0)).toEqual(["1", "1", "5", null]);
    expect(attrs(1)).toEqual(["1", "2", "5", "true"]);
    expect(attrs(3)).toEqual(["2", "1", "1", null]);
    expect(attrs(5)).toEqual(["1", "5", "5", null]);
    await user.click(screen.getByRole("button", { name: "Collapse each" }));
    expect(attrs(1)).toEqual(["1", "2", "5", "false"]);
  });
});

describe("ExecutionGanttTab: focus and history", () => {
  it("returns focus to the run's row when the panel's Close is used", async () => {
    const user = userEvent.setup();
    renderTab(execution());
    await user.click(within(rowFor(2)).getByText("load"));
    await user.click(screen.getByRole("button", { name: "Close the task run" }));
    expect(screen.queryByTestId("gantt-run-panel")).toBeNull();
    expect(rowFor(2)).toHaveFocus();
  });

  it("closes with Escape from inside the panel, focus back on the row", async () => {
    const user = userEvent.setup();
    renderTab(execution(), "/gantt?run=0");
    screen.getByRole("button", { name: "Close the task run" }).focus();
    await user.keyboard("{Escape}");
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/gantt$/);
    expect(rowFor(0)).toHaveFocus();
  });

  it("leaves Escape alone when something inside the panel has used it", async () => {
    const user = userEvent.setup();
    renderTab(execution(), "/gantt?run=0", {
      logs: () => (
        <input
          aria-label="Search logs"
          onKeyDown={(e) => {
            if (e.key === "Escape") e.preventDefault();
          }}
        />
      ),
    });
    screen.getByRole("textbox", { name: "Search logs" }).focus();
    await user.keyboard("{Escape}");
    expect(screen.getByTestId("gantt-run-panel")).toBeInTheDocument();
  });

  it("puts focus on the grid when an unknown run's note is closed", async () => {
    const user = userEvent.setup();
    renderTab(execution(), "/gantt?run=99");
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(rowFor(0)).toHaveFocus();
  });

  it("pushes one history entry per change of selection, so Back walks them", async () => {
    const user = userEvent.setup();
    renderTab(execution());
    await user.tab();
    await user.keyboard("{Enter}");
    // Enter again on the selected row is not a new entry.
    await user.keyboard("{Enter}");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(screen.getByTestId("where")).toHaveTextContent("/gantt?run=1");
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByTestId("where")).toHaveTextContent("/gantt?run=0");
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByTestId("where")).toHaveTextContent(/^\/gantt$/);
  });
});

describe("ExecutionGanttTab: slots", () => {
  function Counter({ id }: { id: string }) {
    const [n, setN] = useState(0);
    return (
      <button type="button" onClick={() => setN(n + 1)}>
        {id} clicked {n}
      </button>
    );
  }

  it("starts a stateful slot afresh when the selection changes", async () => {
    const user = userEvent.setup();
    renderTab(execution(), "/gantt?run=0", { logs: (r) => <Counter id={r.id} /> });
    await user.click(screen.getByRole("button", { name: "r-0 clicked 0" }));
    expect(screen.getByRole("button", { name: "r-0 clicked 1" })).toBeInTheDocument();
    await user.click(within(rowFor(2)).getByText("load"));
    expect(screen.getByRole("button", { name: "r-2 clicked 0" })).toBeInTheDocument();
  });
});

describe("ExecutionGanttTab: live redraws", () => {
  const live = () =>
    execution({
      state: "RUNNING",
      ended_at: null,
      runs: [run(0, "work", { state: "RUNNING", history: ["CREATED", "RUNNING"], ended_seq: null, started_at: at(0) })],
    });

  const setVisibility = (state: "visible" | "hidden") => {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => state });
    document.dispatchEvent(new Event("visibilitychange"));
  };

  afterEach(() => {
    setVisibility("visible");
  });

  it("redraws on animation frames, at most every 40 ms, and not at all while hidden", () => {
    vi.useFakeTimers({
      toFake: ["requestAnimationFrame", "cancelAnimationFrame", "Date", "setTimeout", "clearTimeout", "setInterval", "clearInterval"],
      now: Date.parse(at(10)),
    });
    let renders = 0;
    render(
      <MemoryRouter initialEntries={["/gantt"]}>
        <Profiler id="gantt" onRender={() => renders++}>
          <ExecutionGanttTab execution={live()} slots={NO_SLOTS} />
        </Profiler>
      </MemoryRouter>,
    );
    const duration = () => within(rowFor(0)).getAllByRole("gridcell")[2].textContent;
    expect(duration()).toBe("10 s so far");

    // One act per frame: React batches every update inside one act into a single commit.
    const frames = (ms: number) => {
      for (let t = 0; t < ms; t += 16) act(() => vi.advanceTimersByTime(16));
    };
    renders = 0;
    frames(1000);
    // Every frame is 16 ms; a redraw per frame would be ~62 a second, the 40 ms floor allows 25.
    expect(renders).toBeGreaterThanOrEqual(15);
    expect(renders).toBeLessThanOrEqual(26);
    expect(duration()).toBe("11 s so far");

    setVisibility("hidden");
    renders = 0;
    frames(5000);
    expect(renders).toBe(0);
    expect(duration()).toBe("11 s so far");

    setVisibility("visible");
    frames(100);
    expect(duration()).toBe("16 s so far");
  });

  it("does not re-render the slots on clock ticks", () => {
    vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame", "Date"], now: Date.parse(at(10)) });
    let slotRenders = 0;
    function Slot() {
      slotRenders++;
      return <p>slot</p>;
    }
    const logs = () => <Slot />;
    render(
      <MemoryRouter initialEntries={["/gantt?run=0"]}>
        <ExecutionGanttTab execution={live()} slots={{ ...NO_SLOTS, logs }} />
      </MemoryRouter>,
    );
    const before = slotRenders;
    const duration = () => within(rowFor(0)).getAllByRole("gridcell")[2].textContent;
    for (let t = 0; t < 1000; t += 16) act(() => vi.advanceTimersByTime(16));
    expect(duration()).toBe("11 s so far");
    expect(slotRenders).toBe(before);
  });

  it("steps once a second under prefers-reduced-motion", () => {
    vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame", "Date"], now: Date.parse(at(10)) });
    const original = window.matchMedia;
    window.matchMedia = ((q: string) => ({ ...original(q), matches: q.includes("prefers-reduced-motion: reduce") })) as typeof window.matchMedia;
    try {
      let renders = 0;
      render(
        <MemoryRouter initialEntries={["/gantt"]}>
          <Profiler id="gantt" onRender={() => renders++}>
            <ExecutionGanttTab execution={live()} slots={NO_SLOTS} />
          </Profiler>
        </MemoryRouter>,
      );
      renders = 0;
      for (let t = 0; t < 3000; t += 16) act(() => vi.advanceTimersByTime(16));
      // About three in three seconds; the 40 ms cap would allow 75.
      expect(renders).toBeGreaterThanOrEqual(2);
      expect(renders).toBeLessThanOrEqual(4);
    } finally {
      window.matchMedia = original;
    }
  });

  it("does not redraw a finished execution", () => {
    vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame", "Date"], now: Date.parse(at(200)) });
    let renders = 0;
    render(
      <MemoryRouter initialEntries={["/gantt"]}>
        <Profiler id="gantt" onRender={() => renders++}>
          <ExecutionGanttTab execution={execution()} slots={NO_SLOTS} />
        </Profiler>
      </MemoryRouter>,
    );
    renders = 0;
    for (let t = 0; t < 1000; t += 16) act(() => vi.advanceTimersByTime(16));
    expect(renders).toBe(0);
  });
});
