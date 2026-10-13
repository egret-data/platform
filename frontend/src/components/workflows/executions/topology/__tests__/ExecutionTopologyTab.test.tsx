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

/** first -> loop(body) -> call -> later. The nodes are not in drawing order (FE-TEST-1). */
const GRAPH: FlowGraph = {
  nodes: [
    task("tasks.later", LOG),
    task("tasks.call", SUBFLOW, { kind: "executable" }),
    end("tasks.loop"),
    task("tasks.loop.body", LOG),
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
    edge("tasks.loop.body", "end:tasks.loop", "dynamic"),
    edge("end:tasks.loop", "tasks.call"),
    edge("tasks.call", "tasks.later"),
    edge("tasks.later", "end:flow"),
  ],
  clusters: [cluster("tasks.loop", "loop", ["start:tasks.loop", "tasks.loop", "tasks.loop.body", "end:tasks.loop"])],
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

const STATES: Record<string, NodeState> = {
  "tasks.first": st("SUCCESS", { run_id: "r-0" }),
  "tasks.loop": st("RUNNING", { run_id: "r-1", ended_at: null }),
  // A folded body: no run of its own.
  "tasks.loop.body": st("FAILED", { iterations: { total: 3, by_state: { SUCCESS: 2, FAILED: 1 } } }),
  "tasks.call": st("RUNNING", { run_id: "r-2", ended_at: null }),
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

function execution(state: string): ExecutionDto {
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
    runs: [run("first", "SUCCESS", 0), run("loop", "RUNNING", 1), run("call", "RUNNING", 2)],
    can_manage: false,
  } as unknown as ExecutionDto;
}

function child(id: string, state: string, over: Record<string, unknown>) {
  return {
    id,
    flow_id: null,
    namespace: "company.team",
    flow_key: "hello",
    state,
    history: ["CREATED", state],
    inputs: {},
    seq: 2,
    attempt_number: 1,
    origin_offset_ms: 0,
    parent_execution_id: null,
    parent_run_ix: null,
    trigger_execution_id: null,
    trigger_run_ix: null,
    loop_index: null,
    loop_value: null,
    created_at: T0,
    updated_at: T3,
    ...over,
  };
}

const iter = (n: number, state: string, loop_index: number, loop_value: string) =>
  child(`5e5e5e5e-0000-4000-8000-00000000010${n}`, state, { parent_execution_id: EXEC, parent_run_ix: 1, loop_index, loop_value });

/** Out of order; a retry of `first`, the Subflow's child and another Loop run's iteration are not this loop's iterations. */
const CHILDREN = [
  iter(2, "SUCCESS", 2, "c"),
  child("5e5e5e5e-0000-4000-8000-0000000000f1", "RUNNING", { parent_execution_id: EXEC, parent_run_ix: 7, loop_index: 0, loop_value: "z" }),
  child("5e5e5e5e-0000-4000-8000-0000000000e1", "SUCCESS", { parent_execution_id: EXEC, parent_run_ix: 0 }),
  iter(0, "FAILED", 0, "a"),
  child(CHILD, "KILLED", { trigger_execution_id: EXEC, trigger_run_ix: 2 }),
  iter(1, "SUCCESS", 1, "b"),
];

type Reply = { status: number; body: unknown };
type Call = { path: string; query: URLSearchParams };

/** The routes from memory through a stubbed `fetch`, so the tab runs its real orval hooks (FE-TEST-1). */
class Server {
  calls: Call[] = [];
  graphs: Reply[];
  children: unknown[] = CHILDREN;
  private served = 0;

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
      if (query.get("subflows")) return ok(EXPANDED, EXPANDED_STATES);
      return this.graphs[Math.min(this.served++, this.graphs.length - 1)];
    }
    if (path === LIST_PATH) return { status: 200, body: { rows: this.children, total: this.children.length, limit: 200, offset: 0 } };
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

  it("lists a Loop body node's iterations, in order, each with its state and a link to its child execution", async () => {
    const server = new Server(ok(GRAPH));
    renderTab(execution("FAILED"));
    await userEvent.click(await findNode("tasks.loop.body"));
    const panel = await screen.findByRole("region", { name: /body/ });
    expect(within(panel).getByText("2 of 3 iterations done, 1 failed")).toBeInTheDocument();
    const rows = await within(panel).findAllByRole("row");
    // A header, then the three iterations of `tasks.loop`'s run (#1); not the retry, not the Subflow's child.
    expect(rows).toHaveLength(4);
    expect(rows.slice(1).map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual(["#0a", "#1b", "#2c"]);
    expect(within(rows[1]).getByText("Failed")).toBeInTheDocument();
    expect(within(rows[2]).getByText("Success")).toBeInTheDocument();
    expect(within(rows[1]).getByRole("link")).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/executions/5e5e5e5e-0000-4000-8000-000000000100/topology?node=tasks.body`,
    );
    expect(server.listReads()[0].query.get("parent")).toBe(EXEC);
    expect(screen.queryByTestId("gantt-run-panel")).toBeNull();
    expect(where()).toContain("?node=tasks.loop.body");
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
  it("draws the child execution's states, and links a child node to it", async () => {
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
  });
});
