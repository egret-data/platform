import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import ExecutionDetail from "@/routes/workflows/ExecutionDetail";
import { EXEC, EXEC_PATH, FileServer, uri, WS } from "../../files/__tests__/fileServer";

/**
 * The Outputs tab, the Gantt run panel's outputs, and the Overview's input
 * file rows, through the real execution page, its orval hooks and the routes
 * served from memory (FE-TEST-1).
 */
function run(id: string, run_ix: number, task_id: string, outputs: Record<string, unknown> | null, extra: Record<string, unknown> = {}) {
  return {
    id,
    execution_id: EXEC,
    run_ix,
    task_ix: run_ix,
    task_id,
    state: "SUCCESS",
    history: ["CREATED", "RUNNING", "SUCCESS"],
    attempts: [{ history: ["RUNNING", "SUCCESS"], started_at: 0, ended_at: 1000 }],
    outputs,
    started_at: "2026-10-10T10:00:00.000001Z",
    ended_at: "2026-10-10T10:00:02.000001Z",
    created_at: "2026-10-10T10:00:00.000001Z",
    updated_at: "2026-10-10T10:00:02.000001Z",
    ...extra,
  };
}

function execution(extra: Record<string, unknown> = {}) {
  return {
    id: EXEC,
    flow_id: null,
    namespace: "company.team",
    flow_key: "hello",
    state: "SUCCESS",
    history: ["CREATED", "RUNNING", "SUCCESS"],
    inputs: {},
    seq: 1,
    execution_trigger: null,
    effective_labels: [],
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
    // Out of `run_ix` order, and the middle run has no outputs: the list orders and filters.
    runs: [
      run("r-2", 2, "load", { rows: 12, file: uri("load.csv") }),
      run("r-0", 0, "extract", { uri: uri("extract.ion") }),
      // `null`, the API's shape for a run with no outputs (a Log, a Sleep, a running task).
      run("r-1", 1, "log", null),
    ],
    can_manage: false,
    ...extra,
  };
}

function serve(exec: unknown): FileServer {
  const server = new FileServer().file(uri("load.csv"), 1024).file(uri("extract.ion"), 2048);
  server.others.set(EXEC_PATH, { status: 200, body: exec });
  server.others.set(`/api/ws/${WS}/workflows/executions`, { status: 200, body: { rows: [], total: 0, limit: 50, offset: 0 } });
  return server.install();
}

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

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ExecutionOutputsTab", () => {
  it("lists the runs that have outputs, in run order, and opens the first", async () => {
    serve(execution());
    renderAt("outputs");
    const list = await screen.findByRole("navigation", { name: "Task runs with outputs" });
    const links = within(list).getAllByRole("link");
    expect(links.map((l) => l.textContent)).toEqual(["extract#0", "load#2"]);
    expect(links[0]).toHaveAttribute("aria-current", "true");
    expect(screen.getByRole("tree", { name: "Outputs of extract" })).toBeInTheDocument();
  });

  it("selects a run through ?run=, pushed so Back walks it", async () => {
    serve(execution());
    renderAt("outputs");
    const list = await screen.findByRole("navigation", { name: "Task runs with outputs" });
    await userEvent.click(within(list).getByRole("link", { name: /load/ }));
    expect(where()).toBe(`/explore/${WS}/workflows/executions/${EXEC}/outputs?run=2`);
    expect(screen.getByRole("tree", { name: "Outputs of load" })).toBeInTheDocument();
    expect(within(list).getByRole("link", { name: /load/ })).toHaveAttribute("aria-current", "true");
    const files = screen.getByRole("region", { name: "Files" });
    expect(await within(files).findByText("1.0 KiB")).toBeInTheDocument();
  });

  it("says when ?run= names a run without outputs or not in this execution", async () => {
    serve(execution());
    renderAt("outputs?run=7");
    expect(await screen.findByText(/Task run #7 has no outputs in this execution/)).toBeInTheDocument();
    expect(screen.queryByRole("tree")).toBeNull();
  });

  it("shows the loop-iteration note in the tab too", async () => {
    serve(execution({ loop_index: 0, loop_value: "eu-west", parent_execution_id: "5e5e5e5e-0000-4000-8000-0000000000ff" }));
    renderAt("outputs");
    expect(await screen.findByText(/resolve inside this iteration only/)).toHaveTextContent("eu-west");
  });

  it("is empty when no run has outputs, and says why by the execution's state", async () => {
    serve(execution({ runs: [run("r-1", 1, "log", null), run("r-3", 3, "sleep", {})] }));
    renderAt("outputs");
    expect(await screen.findByText("No outputs")).toBeInTheDocument();
    expect(screen.getByText("No task run of this execution has outputs.")).toBeInTheDocument();
  });
});

describe("Outputs in the Gantt run panel", () => {
  it("says a run with null outputs has none, instead of crashing the page", async () => {
    serve(execution());
    renderAt("gantt?run=1");
    const panel = await screen.findByTestId("gantt-run-outputs");
    expect(within(panel).getByText("This task run has no outputs.")).toBeInTheDocument();
    expect(within(panel).queryByRole("tree")).toBeNull();
  });

  it("notes that a loop iteration's expressions resolve only inside it", async () => {
    serve(execution({ loop_index: 0, loop_value: "eu-west", parent_execution_id: "5e5e5e5e-0000-4000-8000-0000000000ff" }));
    renderAt("gantt?run=2");
    const panel = await screen.findByTestId("gantt-run-outputs");
    expect(within(panel).getByText(/resolve inside this iteration only/)).toHaveTextContent("The parent execution cannot reach these outputs.");
  });

  it("shows the selected run's outputs tree in the panel", async () => {
    serve(execution());
    renderAt("gantt?run=2");
    const panel = await screen.findByTestId("gantt-run-outputs");
    expect(within(panel).getByRole("tree", { name: "Outputs of load" })).toBeInTheDocument();
    expect(await within(panel).findByText("1.0 KiB")).toBeInTheDocument();
  });
});

describe("Overview inputs", () => {
  it("shows a kestra:/// input as a file row with its size, Preview and Download", async () => {
    const server = serve(execution({ inputs: { who: "world", data: uri("load.csv") } }));
    renderAt("overview");
    const inputs = await screen.findByRole("region", { name: "Inputs" });
    expect(await within(inputs).findByText("1.0 KiB")).toBeInTheDocument();
    expect(within(inputs).getByRole("button", { name: "Preview load.csv" })).toBeInTheDocument();
    expect(within(inputs).getByRole("link", { name: "Download load.csv" })).toBeInTheDocument();
    expect(server.callsTo("metas").map((c) => c.query.get("path"))).toEqual([uri("load.csv")]);
    expect(within(inputs).getByText("world")).toBeInTheDocument();
  });

  it("finds files inside a structured input", async () => {
    serve(execution({ inputs: { many: [uri("load.csv"), uri("extract.ion")] } }));
    renderAt("overview");
    const inputs = await screen.findByRole("region", { name: "Inputs" });
    expect(await within(inputs).findByText("2.0 KiB")).toBeInTheDocument();
    expect(within(inputs).getByRole("link", { name: "Download load.csv" })).toBeInTheDocument();
    expect(within(inputs).getByRole("link", { name: "Download extract.ion" })).toBeInTheDocument();
  });
});
