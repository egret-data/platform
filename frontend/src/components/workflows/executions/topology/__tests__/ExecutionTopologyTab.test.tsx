import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ExecutionDto, FlowGraph, NodeState, TaskRunDto } from "@/api/generated/model";
import type { GanttRunSlots } from "@/components/workflows/executions/gantt/GanttView";
import ExecutionTopologyTab from "@/components/workflows/executions/topology/ExecutionTopologyTab";
import { installXyflowJsdom } from "@/components/workflows/topology/__tests__/xyflowJsdom";
import { cluster, edge, end, start, task } from "@/components/workflows/topology/__tests__/fixtures";

const WS = "eng";
const EXEC = "5e5e5e5e-0000-4000-8000-0000000000aa";
const CHILD = "5e5e5e5e-0000-4000-8000-0000000000c1";
const BASE = `/api/ws/${WS}/workflows`;
const GRAPH_PATH = `${BASE}/executions/${EXEC}/graph`;
const LIST_PATH = `${BASE}/executions`;

const LOG = "io.kestra.plugin.core.log.Log";
const LOOP = "io.kestra.plugin.core.flow.ForEach";
const SUBFLOW = "io.kestra.plugin.core.flow.Subflow";

/**
 * first -> loop(body, big; errors: h) -> call -> later. The nodes are not in drawing order
 * (FE-TEST-1). `h` is the Loop's own `errors:` handler: it sits in the loop's cluster with a body
 * task's uid shape (`tasks.loop.h`), but runs in the execution itself, never in an iteration.
 */
const GRAPH: FlowGraph = {
  nodes: [
    task("tasks.later", LOG),
    task("tasks.call", SUBFLOW, { kind: "executable" }),
    end("tasks.loop"),
    task("tasks.loop.body", LOG),
    task("tasks.loop.h", LOG, { branch: "errors" }),
    task("tasks.loop.big", LOG),
    task("tasks.loop", LOOP, { kind: "flowable" }),
    start("tasks.loop"),
    task("tasks.first", LOG),
    end("flow"),
    start("flow"),
  ],
  edges: [
    edge("start:flow", "tasks.first"),
    edge("tasks.first", "start:tasks.loop"),
    edge("start:tasks.loop", "tasks.loop"),
    edge("tasks.loop", "tasks.loop.body", "dynamic"),
    edge("tasks.loop.body", "tasks.loop.big", "dynamic"),
    edge("tasks.loop.big", "end:tasks.loop", "dynamic"),
    edge("tasks.loop", "tasks.loop.h", "error"),
    edge("tasks.loop.h", "end:tasks.loop", "error"),
    edge("end:tasks.loop", "tasks.call"),
    edge("tasks.call", "tasks.later"),
    edge("tasks.later", "end:flow"),
  ],
  clusters: [cluster("tasks.loop", "loop", ["start:tasks.loop", "tasks.loop", "tasks.loop.body", "tasks.loop.big", "tasks.loop.h", "end:tasks.loop"])],
};

/** `GRAPH` with `tasks.call` expanded to its child flow's one task, `tasks.x`. */
const EXPANDED: FlowGraph = {
  nodes: [...GRAPH.nodes, start("tasks.call"), end("tasks.call"), task("tasks.call/tasks.x", LOG)],
  edges: [
    ...GRAPH.edges.filter((e) => e.target !== "tasks.call" && e.source !== "tasks.call"),
    edge("end:tasks.loop", "start:tasks.call"),
    edge("start:tasks.call", "tasks.call"),
    edge("tasks.call", "tasks.call/tasks.x"),
    edge("tasks.call/tasks.x", "end:tasks.call"),
    edge("end:tasks.call", "tasks.later"),
  ],
  clusters: [...GRAPH.clusters, cluster("tasks.call", "subflow", ["start:tasks.call", "tasks.call", "tasks.call/tasks.x", "end:tasks.call"])],
};

const T0 = "2026-10-13T10:00:00.000001Z";
const T3 = "2026-10-13T10:00:03.000001Z";
const st = (state: string, over: Partial<NodeState> = {}): NodeState => ({
  state,
  attempts: 1,
  run_id: null,
  started_at: T0,
  ended_at: T3,
  ...over,
});

const iterationId = (i: number) => `5e5e5e5e-0000-4000-8000-${String(100 + i).padStart(12, "0")}`;
const iterRun = (loop_index: number, state: string) => ({ loop_index, state, child_execution_id: iterationId(loop_index), run_id: `it-${loop_index}` });

/**
 * `body`'s own runs: it SUCCEEDED in iteration #1, which failed on a later body task (the server's
 * [A,B,C] case, graph/states.rs), and failed in #2. Out of order is the server's job, so it comes
 * in order. `big` ran in 201 iterations, more than any list page holds.
 */
const BODY_RUNS = [iterRun(0, "SUCCESS"), iterRun(1, "SUCCESS"), iterRun(2, "FAILED")];
const BIG_RUNS = Array.from({ length: 201 }, (_, i) => iterRun(i, "SUCCESS"));

const STATES: Record<string, NodeState> = {
  "tasks.first": st("SUCCESS", { run_id: "r-0" }),
  "tasks.loop": st("RUNNING", { run_id: "r-1", ended_at: null }),
  // Folded bodies: no run of their own.
  "tasks.loop.body": st("FAILED", { iterations: { total: 3, by_state: { SUCCESS: 2, FAILED: 1 }, runs: BODY_RUNS } }),
  "tasks.loop.big": st("SUCCESS", { iterations: { total: 201, by_state: { SUCCESS: 201 }, runs: BIG_RUNS } }),
  "tasks.call": st("RUNNING", { run_id: "r-2", ended_at: null, child_execution_id: CHILD }),
};
const EXPANDED_STATES: Record<string, NodeState> = {
  ...STATES,
  "tasks.call/tasks.x": st("KILLED", { run_id: "child-r-0" }),
};

function run(task_id: string, state: string, run_ix: number): TaskRunDto {
  return {
    id: `r-${run_ix}`,
    execution_id: EXEC,
    run_ix,
    task_ix: run_ix,
    task_id,
    state,
    history: ["CREATED", "RUNNING", state],
    attempts: [],
    outputs: {},
    started_at: T0,
    ended_at: state === "RUNNING" ? null : T3,
    created_at: T0,
    updated_at: T3,
  } as unknown as TaskRunDto;
}

function execution(state: string, afterRun = false): ExecutionDto {
  return {
    id: EXEC,
    flow_id: null,
    namespace: "company.team",
    flow_key: "hello",
    state,
    history: ["CREATED", "RUNNING", ...(state === "RUNNING" ? [] : [state])],
    inputs: {},
    seq: 1,
    execution_trigger: null,
    attempt_number: 1,
    origin_offset_ms: 0,
    created_at: T0,
    started_at: T0,
    ended_at: state === "RUNNING" ? null : T3,
    updated_at: T3,
    source: "",
    // A finished execution has only finished runs, unless an afterExecution run is still live.
    runs:
      state === "RUNNING" || afterRun
        ? [run("first", "SUCCESS", 0), run("loop", "RUNNING", 1), run("call", "RUNNING", 2)]
        : [run("first", "SUCCESS", 0), run("loop", "SUCCESS", 1), run("call", "SUCCESS", 2)],
    can_manage: false,
  } as unknown as ExecutionDto;
}

type Reply = { status: number; body: unknown };
type Call = { path: string; query: URLSearchParams };

/** The routes from memory through a stubbed `fetch`, so the tab runs its real orval hooks (FE-TEST-1). */
class Server {
  calls: Call[] = [];
  graphs: Reply[];
  /** The expanded reads, in turn; the last one for good. */
  expandedGraphs: Reply[] = [ok(EXPANDED, EXPANDED_STATES)];
  private served = 0;
  private servedExpanded = 0;

  constructor(...graphs: Reply[]) {
    this.graphs = graphs;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string) => {
        const [path, qs] = String(input).split("?");
        const query = new URLSearchParams(qs ?? "");
        this.calls.push({ path, query });
        const { status, body } = this.route(path, query);
        return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
      }),
    );
  }

  private route(path: string, query: URLSearchParams): Reply {
    if (path === GRAPH_PATH) {
      if (query.get("subflows")) return this.expandedGraphs[Math.min(this.servedExpanded++, this.expandedGraphs.length - 1)];
      return this.graphs[Math.min(this.served++, this.graphs.length - 1)];
    }
    // The tab must not need the children list (fix round 1): any read of it is a defect.
    if (path === LIST_PATH) return { status: 500, body: { error: "the tab read the children list" } };
    return { status: 404, body: { error: `no route ${path}` } };
  }

  graphReads(): Call[] {
    return this.calls.filter((c) => c.path === GRAPH_PATH);
  }
  listReads(): Call[] {
    return this.calls.filter((c) => c.path === LIST_PATH);
  }
}

const ok = (graph: FlowGraph, states: Record<string, NodeState> = STATES): Reply => ({ status: 200, body: { graph, states } });

const SLOTS: GanttRunSlots = {
  logs: (r) => <p>{`logs of ${r.id}`}</p>,
  outputs: (r) => <p>{`outputs of ${r.id}`}</p>,
};

function Where() {
  const l = useLocation();
  return <output data-testid="where">{l.pathname + l.search}</output>;
}

function renderTab(exec: ExecutionDto, search = "") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const ui = (e: ExecutionDto) => (
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/explore/${WS}/workflows/executions/${EXEC}/topology${search}`]}>
        <Routes>
          <Route
            path="/explore/:workspace/workflows/executions/:id/:tab"
            element={
              <div style={{ width: 800, height: 600 }}>
                <ExecutionTopologyTab ws={WS} execution={e} slots={SLOTS} now={Date.parse(T3)} />
              </div>
            }
          />
        </Routes>
        <Where />
      </MemoryRouter>
    </QueryClientProvider>
  );
  const utils = render(ui(exec));
  return { ...utils, update: (e: ExecutionDto) => utils.rerender(ui(e)) };
}

/** The node button carrying `uid`, or null. */
const nodeButton = (uid: string) => screen.queryAllByRole("button").find((b) => b.getAttribute("data-uid") === uid) ?? null;
const findNode = (uid: string) => waitFor(() => {
  const b = nodeButton(uid);
  if (!b) throw new Error(`no node ${uid}`);
  return b;
});
const where = () => screen.getByTestId("where").textContent ?? "";

beforeAll(installXyflowJsdom);
beforeEach(() => {
  try {
    localStorage.clear();
  } catch {
    /* no storage in this environment */
  }
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("ExecutionTopologyTab: updates (spec §6.3)", () => {
  it("reads the graph every 2 s while the execution runs, once more when it ends, then never", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const server = new Server(ok(GRAPH));
    const { update } = renderTab(execution("RUNNING"));
    await findNode("tasks.first");
    expect(server.graphReads()).toHaveLength(1);
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(server.graphReads()).toHaveLength(2));
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(server.graphReads()).toHaveLength(3));

    // The execution's own poll saw it end: one read for the final states, then none.
    update(execution("SUCCESS"));
    await waitFor(() => expect(server.graphReads()).toHaveLength(4));
    await act(() => vi.advanceTimersByTimeAsync(20_000));
    expect(server.graphReads()).toHaveLength(4);
  });

  it("keeps reading every 2 s while a terminal execution still has a live afterExecution run, and stops when it ends", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const server = new Server(ok(GRAPH));
    const { update } = renderTab(execution("SUCCESS", true));
    await findNode("tasks.first");
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(server.graphReads()).toHaveLength(2));
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(server.graphReads()).toHaveLength(3));
    update(execution("SUCCESS"));
    await waitFor(() => expect(server.graphReads()).toHaveLength(4));
    await act(() => vi.advanceTimersByTimeAsync(20_000));
    expect(server.graphReads()).toHaveLength(4);
  });

  it("sends no request after the first when the execution is already terminal", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const server = new Server(ok(GRAPH));
    renderTab(execution("FAILED"));
    await findNode("tasks.first");
    await act(() => vi.advanceTimersByTimeAsync(20_000));
    expect(server.graphReads()).toHaveLength(1);
  });

  it("backs off while reads fail, keeping the graph and saying why", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const fail: Reply = { status: 503, body: { error: "the database is unavailable" } };
    const server = new Server(ok(GRAPH), fail, fail, fail, ok(GRAPH));
    renderTab(execution("RUNNING"));
    await findNode("tasks.first");
    // t=2 s: the first failure. The wait is the time since the last good answer: 2 s, then 4 s.
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(server.graphReads()).toHaveLength(2));
    expect(await screen.findByText(/the database is unavailable/)).toBeInTheDocument();
    expect(nodeButton("tasks.first")).not.toBeNull();
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(server.graphReads()).toHaveLength(3));
    // Last good answer at t=0, failure at t=4 s: the next read waits 4 s, not 2.
    await act(() => vi.advanceTimersByTimeAsync(3000));
    expect(server.graphReads()).toHaveLength(3);
    await act(() => vi.advanceTimersByTimeAsync(1000));
    await waitFor(() => expect(server.graphReads()).toHaveLength(4));
  });
});

describe("ExecutionTopologyTab: selection (spec §6.3)", () => {
  it("opens the Gantt's run panel for a task node, with that run's logs and outputs, in the URL", async () => {
    new Server(ok(GRAPH));
    renderTab(execution("FAILED"));
    await userEvent.click(await findNode("tasks.first"));
    const panel = await screen.findByTestId("gantt-run-panel");
    expect(within(panel).getByRole("heading", { level: 2 })).toHaveTextContent("first");
    expect(within(panel).getByText("logs of r-0")).toBeInTheDocument();
    expect(within(panel).getByText("outputs of r-0")).toBeInTheDocument();
    expect(where()).toContain("?node=tasks.first");

    // Another run replaces it.
    await userEvent.click(nodeButton("tasks.call")!);
    expect(within(await screen.findByTestId("gantt-run-panel")).getByText("logs of r-2")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Close the task run" }));
    expect(screen.queryByTestId("gantt-run-panel")).toBeNull();
    expect(where()).not.toContain("node=");
    // Focus goes back to the node, not to the page.
    expect(document.activeElement).toBe(nodeButton("tasks.call"));
  });

  it("opens the panel from a shared link", async () => {
    new Server(ok(GRAPH));
    renderTab(execution("FAILED"), "?node=tasks.call");
    expect(within(await screen.findByTestId("gantt-run-panel")).getByText("logs of r-2")).toBeInTheDocument();
  });

  it("lists a Loop body node's own state in each iteration, under a header naming the task, linking each child execution", async () => {
    const server = new Server(ok(GRAPH));
    renderTab(execution("FAILED"));
    await userEvent.click(await findNode("tasks.loop.body"));
    const panel = await screen.findByTestId("loop-iterations");
    expect(within(panel).getByRole("heading", { level: 2 })).toHaveTextContent("body");
    expect(within(panel).getByText("2 of 3 iterations done, 1 failed")).toBeInTheDocument();
    // The column is the task's, so it is not read as the iteration's own state.
    expect(within(panel).getByRole("columnheader", { name: "State of body" })).toBeInTheDocument();
    const rows = within(panel).getAllByRole("row").slice(1);
    expect(rows.map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual(["#0", "#1", "#2"]);
    // #1: the iteration failed elsewhere; `body` itself succeeded there, and that is what is shown.
    expect(within(rows[1]).getByText("Success")).toBeInTheDocument();
    expect(within(rows[2]).getByText("Failed")).toBeInTheDocument();
    expect(within(rows[1]).getByRole("link")).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/executions/${iterationId(1)}/topology?node=tasks.body`,
    );
    expect(screen.queryByTestId("gantt-run-panel")).toBeNull();
    expect(where()).toContain("?node=tasks.loop.body");
    expect(server.listReads()).toHaveLength(0);

    await userEvent.click(within(panel).getByRole("button", { name: "Close the iterations" }));
    expect(screen.queryByTestId("loop-iterations")).toBeNull();
    expect(document.activeElement).toBe(nodeButton("tasks.loop.body"));
  });

  it("lists every iteration, past any page size", async () => {
    new Server(ok(GRAPH));
    renderTab(execution("FAILED"));
    await userEvent.click(await findNode("tasks.loop.big"));
    const panel = await screen.findByTestId("loop-iterations");
    const rows = within(panel).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(201);
    expect(within(rows[0]).getAllByRole("cell")[0]).toHaveTextContent("#0");
    expect(within(rows[200]).getAllByRole("cell")[0]).toHaveTextContent("#200");
    expect(within(panel).queryByText(/not listed/)).toBeNull();
  });

  it("does not list iterations for a Loop's own errors handler that has not run", async () => {
    new Server(ok(GRAPH));
    renderTab(execution("SUCCESS"));
    await userEvent.click(await findNode("tasks.loop.h"));
    expect(await screen.findByText("h has not run in this execution.")).toBeInTheDocument();
    expect(screen.queryByTestId("loop-iterations")).toBeNull();
  });

  it("says when a node has not run", async () => {
    new Server(ok(GRAPH));
    renderTab(execution("FAILED"));
    await userEvent.click(await findNode("tasks.later"));
    expect(await screen.findByText("later has not run in this execution.")).toBeInTheDocument();
    expect(screen.queryByTestId("gantt-run-panel")).toBeNull();
  });
});

describe("ExecutionTopologyTab: Subflow expansion", () => {
  it("draws the child execution's states, and links a child node to the child the graph names", async () => {
    const server = new Server(ok(GRAPH));
    renderTab(execution("FAILED"));
    await userEvent.click(await screen.findByRole("button", { name: "Expand subflow call" }));
    const x = await findNode("tasks.call/tasks.x");
    expect(within(x).getByText("Killed")).toBeInTheDocument();
    expect(server.graphReads().at(-1)?.query.get("subflows")).toBe("tasks.call");
    expect(where()).toContain("expand=tasks.call");

    await userEvent.click(x);
    const link = await screen.findByRole("link", { name: /Open the child execution/ });
    expect(link).toHaveAttribute("href", `/explore/${WS}/workflows/executions/${CHILD}/topology?node=tasks.x`);
    expect(server.listReads()).toHaveLength(0);

    // Escape from inside the panel closes it and gives focus back to the node.
    link.focus();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("link", { name: /Open the child execution/ })).toBeNull();
    expect(where()).not.toContain("node=");
    expect(document.activeElement).toBe(nodeButton("tasks.call/tasks.x"));
  });

  it("says the child has not started yet, then links it once a poll names it", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const server = new Server(ok(GRAPH));
    const notYet = { ...EXPANDED_STATES, "tasks.call": st("RUNNING", { run_id: "r-2", ended_at: null }) };
    server.expandedGraphs = [ok(EXPANDED, notYet), ok(EXPANDED, EXPANDED_STATES)];
    renderTab(execution("RUNNING"), "?expand=tasks.call&node=tasks.call%2Ftasks.x");
    expect(await screen.findByText("The Subflow call has not started its child execution yet.")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /Open the child execution/ })).toBeNull();
    await act(() => vi.advanceTimersByTimeAsync(2000));
    expect(await screen.findByRole("link", { name: /Open the child execution/ })).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/executions/${CHILD}/topology?node=tasks.x`,
    );
    await userEvent.click(screen.getByRole("button", { name: "Close the child task" }));
    expect(screen.queryByRole("link", { name: /Open the child execution/ })).toBeNull();
  });
});
