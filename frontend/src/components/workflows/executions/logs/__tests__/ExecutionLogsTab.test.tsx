import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import ExecutionDetail from "@/routes/workflows/ExecutionDetail";
import { EXEC, line, LogServer, WS } from "./logServer";

/**
 * The Logs tab and the Gantt's run panel, through the real execution page,
 * its orval hooks and the log route served from memory (FE-TEST-1).
 */
const EXEC_PATH = `/api/ws/${WS}/workflows/executions/${EXEC}`;

function run(id: string, run_ix: number, task_id: string) {
  return {
    id,
    execution_id: EXEC,
    run_ix,
    task_ix: run_ix,
    task_id,
    state: "SUCCESS",
    history: ["CREATED", "RUNNING", "SUCCESS"],
    attempts: [{ history: ["RUNNING", "SUCCESS"], started_at: Date.parse("2026-10-10T10:00:00Z"), ended_at: Date.parse("2026-10-10T10:00:02Z") }],
    outputs: {},
    started_at: "2026-10-10T10:00:00.000001Z",
    ended_at: "2026-10-10T10:00:02.000001Z",
    created_at: "2026-10-10T10:00:00.000001Z",
    updated_at: "2026-10-10T10:00:02.000001Z",
  };
}

const execution = {
  id: EXEC,
  flow_id: null,
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
  created_at: "2026-10-10T09:59:00.000001Z",
  started_at: "2026-10-10T10:00:00.000001Z",
  ended_at: "2026-10-10T10:01:00.000001Z",
  updated_at: "2026-10-10T10:01:00.000001Z",
  source: "id: hello\n",
  // Listed out of order: the pickers and groups order by `run_ix`.
  runs: [run("r-2", 2, "load"), run("r-0", 0, "extract"), run("r-1", 1, "extract")],
  can_manage: false,
};

function Where() {
  const l = useLocation();
  return <output data-testid="where">{l.pathname + l.search}</output>;
}

function renderAt(path: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/explore/${WS}/workflows/executions/${EXEC}/${path}`]}>
        <Routes>
          <Route path="/explore/:workspace/workflows/executions/:id/:tab" element={<ExecutionDetail />} />
        </Routes>
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const where = () => screen.getByTestId("where").textContent ?? "";
const lastQuery = (server: LogServer) => server.queries[server.queries.length - 1];

async function pick(name: string, option: string) {
  await userEvent.click(screen.getByRole("combobox", { name }));
  await userEvent.click(await screen.findByRole("option", { name: option }));
}

let server: LogServer;
beforeEach(() => {
  server = new LogServer().install();
  server.others.set(EXEC_PATH, { status: 200, body: execution });
  server.state = "SUCCESS";
  server.complete = true;
  server.lines = [
    line(1, { message: "execution starts" }),
    line(2, { task_id: "extract", task_run_id: "r-0", level: "DEBUG", message: "extract debug" }),
    line(3, { task_id: "load", task_run_id: "r-2", level: "WARN", message: "load warns" }),
    line(4, { task_id: "extract", task_run_id: "r-1", level: "ERROR", message: "extract #1 fails" }),
  ];
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("ExecutionLogsTab", () => {
  it("shows every line, grouped by run, with no filter sent", async () => {
    renderAt("logs");
    expect(await screen.findByText("load warns")).toBeInTheDocument();
    const q = lastQuery(server);
    expect(q.has("min_level") || q.has("task_id") || q.has("task_run_id")).toBe(false);
    const groups = screen.getAllByTestId("log-group").map((g) => within(g).getByRole("button").textContent);
    expect(groups).toEqual(["Execution1 line", "extract #01 line", "extract #11 line", "load #21 line"]);
  });

  it("writes a level picked into the URL and reads with it", async () => {
    renderAt("logs");
    await screen.findByText("load warns");
    await pick("Level", "WARN and above");
    expect(where()).toContain("?level=WARN");
    await waitFor(() => expect(lastQuery(server).get("min_level")).toBe("WARN"));
    await waitFor(() => expect(screen.queryByText("extract debug")).toBeNull());
    expect(screen.getByText("load warns")).toBeInTheDocument();
    expect(lastQuery(server).has("after")).toBe(false);
  });

  it("reads its filters back from the URL: level, task and run", async () => {
    renderAt("logs?level=ERROR&task=extract&run=1");
    expect(await screen.findByText("extract #1 fails")).toBeInTheDocument();
    const q = lastQuery(server);
    expect(q.get("min_level")).toBe("ERROR");
    expect(q.get("task_id")).toBe("extract");
    expect(q.get("task_run_id")).toBe("r-1");
    expect(screen.getByRole("combobox", { name: "Level" })).toHaveTextContent("ERROR only");
    expect(screen.getByRole("combobox", { name: "Task" })).toHaveTextContent("extract");
    expect(screen.getByRole("combobox", { name: "Task run" })).toHaveTextContent("extract #1");
  });

  it("picking a task offers only its runs, and clears a run of another task", async () => {
    renderAt("logs?run=2");
    await screen.findByText("load warns");
    await pick("Task", "extract");
    expect(where()).toContain("?task=extract");
    expect(where()).not.toContain("run=");
    await userEvent.click(screen.getByRole("combobox", { name: "Task run" }));
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options).toEqual(["All", "extract #0", "extract #1"]);
  });

  it("ignores a run the execution does not have, and says so", async () => {
    renderAt("logs?run=7");
    await screen.findByText("load warns");
    expect(screen.getByText(/Task run #7 is not in this execution/)).toBeInTheDocument();
    expect(lastQuery(server).has("task_run_id")).toBe(false);
  });

  it("a run of another task than ?task= is ignored, and said so (review M5)", async () => {
    renderAt("logs?task=extract&run=2");
    expect(await screen.findByText("extract debug")).toBeInTheDocument();
    expect(screen.getByText("Task run #2 is a run of load, not of extract, so every run of extract is shown.")).toBeInTheDocument();
    const q = lastQuery(server);
    expect(q.get("task_id")).toBe("extract");
    expect(q.has("task_run_id")).toBe(false);
    expect(screen.getByRole("combobox", { name: "Task run" })).toHaveTextContent("All");
  });

  it("a filter that matches nothing says so, and Clear filters empties the URL", async () => {
    server.lines = [line(1, { level: "INFO", message: "only info" })];
    renderAt("logs?level=ERROR");
    expect(await screen.findByText("No lines match the filters")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(where()).toBe(`/explore/${WS}/workflows/executions/${EXEC}/logs`);
    expect(await screen.findByText("only info")).toBeInTheDocument();
  });
});

describe("The Gantt's run panel", () => {
  it("shows the selected run's lines, read with its task_run_id, ungrouped", async () => {
    server.lines = [
      line(1, { task_id: "extract", task_run_id: "r-1", message: "run one says" }),
      line(2, { task_id: "load", task_run_id: "r-2", message: "run two says" }),
    ];
    renderAt("gantt?run=1");
    const panel = await screen.findByTestId("gantt-run-logs");
    expect(await within(panel).findByText("run one says")).toBeInTheDocument();
    expect(within(panel).queryByText("run two says")).toBeNull();
    expect(server.queries.every((q) => q.get("task_run_id") === "r-1")).toBe(true);
    expect(within(panel).queryByTestId("log-group")).toBeNull();
    expect(within(panel).getByRole("log", { name: "Logs of extract #1" })).toBeInTheDocument();
    expect(within(panel).getByRole("log", { name: "Logs of extract #1" })).toHaveAttribute("tabindex", "0");
  });
});
