import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fullTimestamp } from "@/lib/relativeTime";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import ExecutionDetail from "@/routes/workflows/ExecutionDetail";

const WS = "eng";
const EXEC = "5e5e5e5e-0000-4000-8000-0000000000aa";
const NEW = "5e5e5e5e-0000-4000-8000-0000000000bb";
const FLOW = "0a7c1d2e-0000-4000-8000-000000000001";
const BASE = `/api/ws/${WS}/workflows`;
const EXEC_PATH = `${BASE}/executions/${EXEC}`;

function run(task_id: string, state: string, run_ix: number, extra: Record<string, unknown> = {}) {
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
    started_at: null,
    ended_at: null,
    created_at: "2026-10-10T10:00:00.000001Z",
    updated_at: "2026-10-10T10:00:01.000001Z",
    ...extra,
  };
}

const LOG_SOURCE = "id: hello\nnamespace: company.team\ntasks:\n  - id: extract\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
const pauseSource = (props: string) =>
  `id: hello\nnamespace: company.team\ntasks:\n  - id: gate\n    type: io.kestra.plugin.core.flow.Pause\n${props}`;

function execution(state: string, extra: Record<string, unknown> = {}) {
  const terminal = !["RUNNING", "PAUSED", "CREATED"].includes(state);
  return {
    id: EXEC,
    flow_id: FLOW,
    namespace: "company.team",
    flow_key: "hello",
    state,
    history: ["CREATED", "RUNNING", ...(state === "RUNNING" ? [] : [state])],
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
    ended_at: terminal ? "2026-10-10T10:01:05.000001Z" : null,
    // Five minutes after the end: a duration taken from `updated_at` reads differently.
    updated_at: "2026-10-10T10:06:05.000001Z",
    source: LOG_SOURCE,
    runs: [run("extract", "SUCCESS", 0), run("load", state === "RUNNING" ? "RUNNING" : "SUCCESS", 1)],
    can_manage: true,
    worker_enabled: true,
    ...extra,
  };
}

type Reply = { status: number; body: unknown };
type Call = { method: string; path: string; query: URLSearchParams };

/**
 * The execution routes from memory, through a stubbed `fetch`, so the page
 * runs its real orval hooks and mutator (FE-TEST-1).
 */
class Server {
  calls: Call[] = [];
  /** `GET …/executions/{EXEC}`, in turn; the last one for good. */
  gets: Reply[];
  /** `GET …/executions/{NEW}`. */
  newExecution: Reply = { status: 200, body: { ...execution("RUNNING"), id: NEW, replayed_from_execution_id: EXEC } };
  children: unknown[] = [];
  childTotal: number | null = null;
  resume: Reply = { status: 200, body: execution("RUNNING") };
  replay: Reply = { status: 201, body: { ...execution("RUNNING"), id: NEW } };
  kill: Reply = { status: 202, body: execution("KILLING", { ended_at: null }) };
  /** `GET …/executions/{EXEC}/graph` (W7c): a graph with no tasks. */
  graph: Reply = {
    status: 200,
    body: { graph: { nodes: [{ uid: "start:flow", node: "start" }, { uid: "end:flow", node: "end" }], edges: [], clusters: [] }, states: {} },
  };
  private served = 0;

  constructor(...gets: Reply[]) {
    this.gets = gets;
    vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
      const method = (init?.method ?? "GET").toUpperCase();
      const [path, qs] = String(input).split("?");
      this.calls.push({ method, path, query: new URLSearchParams(qs ?? "") });
      const { status, body } = this.route(method, path);
      return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    }));
  }

  private route(method: string, path: string): Reply {
    if (method === "GET" && path === EXEC_PATH) return this.gets[Math.min(this.served++, this.gets.length - 1)];
    if (method === "GET" && path === `${BASE}/executions/${NEW}`) return this.newExecution;
    if (method === "GET" && path === `${BASE}/executions`) {
      return { status: 200, body: { rows: this.children, total: this.childTotal ?? this.children.length, limit: 50, offset: 0 } };
    }
    if (method === "GET" && path === `${EXEC_PATH}/graph`) return this.graph;
    if (method === "POST" && path === `${EXEC_PATH}/resume`) return this.resume;
    if (method === "POST" && path === `${EXEC_PATH}/replay`) return this.replay;
    if (method === "POST" && path === `${EXEC_PATH}/kill`) return this.kill;
    return { status: 404, body: { error: `no route ${method} ${path}` } };
  }

  /** The reads of the execution itself, not of its children. */
  executionReads(): number {
    return this.calls.filter((c) => c.method === "GET" && c.path === EXEC_PATH).length;
  }
}

const ok = (body: unknown): Reply => ({ status: 200, body });

function Where() {
  const l = useLocation();
  return <output data-testid="where">{l.pathname + l.search}</output>;
}

function renderPage(tab = "overview") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/explore/${WS}/workflows/executions/${EXEC}/${tab}`]}>
        <Routes>
          <Route path="/explore/:workspace/workflows/executions/:id/:tab" element={<ExecutionDetail />} />
        </Routes>
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const summary = () => screen.getByTestId("execution-summary");
const fact = (term: string) => within(summary()).getByText(term, { selector: "dt" }).nextElementSibling as HTMLElement;

beforeEach(() => {
  toast.success.mockReset();
  toast.error.mockReset();
  toast.warning.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("ExecutionDetail: the worker-off notice (KI-013)", () => {
  it("says the worker is off and what to set while the execution has not ended", async () => {
    new Server(ok(execution("RUNNING", { worker_enabled: false })));
    renderPage();
    const notice = await screen.findByTestId("worker-off-notice");
    expect(notice).toHaveTextContent("executions are created but do not run");
    expect(notice).toHaveTextContent("EGRET_WORKFLOW__WORKER_ENABLED=true");
  });

  it("says nothing once the execution has ended, nor while the worker runs", async () => {
    new Server(ok(execution("SUCCESS", { worker_enabled: false })));
    const first = renderPage();
    expect(await screen.findByRole("heading", { level: 1, name: EXEC })).toBeInTheDocument();
    expect(screen.queryByTestId("worker-off-notice")).not.toBeInTheDocument();
    first.unmount();
    vi.unstubAllGlobals();

    new Server(ok(execution("RUNNING")));
    renderPage();
    expect(await screen.findByRole("heading", { level: 1, name: EXEC })).toBeInTheDocument();
    expect(screen.queryByTestId("worker-off-notice")).not.toBeInTheDocument();
  });
});

describe("ExecutionDetail: the header", () => {
  it("shows the id, the flow with its link, the namespace, the state, the recorded start and duration, and the trigger", async () => {
    new Server(ok(execution("SUCCESS", { execution_trigger: { id: "nightly", type: "io.kestra.plugin.core.trigger.Schedule", variables: {} } })));
    renderPage();
    expect(await screen.findByRole("heading", { level: 1, name: EXEC })).toBeInTheDocument();
    expect(within(summary()).getByRole("link", { name: "company.team.hello" })).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/flows/${FLOW}/edit`,
    );
    expect(fact("Namespace")).toHaveTextContent("company.team");
    expect(screen.getByRole("banner")).toHaveTextContent("Success");
    // The recorded start, not `created_at` (a minute earlier).
    expect(fact("Started")).toHaveTextContent(fullTimestamp("2026-10-10T10:00:00.000001Z"));
    // `started_at` to `ended_at`; `updated_at` would make it 6 min 5 s.
    expect(fact("Duration")).toHaveTextContent(/^1 min 5 s$/);
    expect(fact("Trigger")).toHaveTextContent("Schedule");
    // Neither link without its field.
    expect(within(summary()).queryByText("Replayed from")).toBeNull();
    expect(within(summary()).queryByText("Child of")).toBeNull();
  });

  it("links the execution it was replayed from, and the one it is a child of", async () => {
    const REPLAYED = "11111111-0000-4000-8000-000000000001";
    const PARENT = "22222222-0000-4000-8000-000000000002";
    new Server(ok(execution("SUCCESS", { replayed_from_execution_id: REPLAYED, trigger_execution_id: PARENT })));
    renderPage();
    await screen.findByRole("heading", { level: 1, name: EXEC });
    expect(within(fact("Replayed from")).getByRole("link", { name: REPLAYED })).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/executions/${REPLAYED}/overview`,
    );
    expect(within(fact("Child of")).getByRole("link", { name: PARENT })).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/executions/${PARENT}/overview`,
    );
  });

  it("links the parent of a loop iteration as Child of, through parent_execution_id", async () => {
    const PARENT = "44444444-0000-4000-8000-000000000004";
    new Server(ok(execution("SUCCESS", { parent_execution_id: PARENT, loop_index: 0, loop_value: "a" })));
    renderPage();
    await screen.findByRole("heading", { level: 1, name: EXEC });
    expect(within(fact("Child of")).getByRole("link", { name: PARENT })).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/executions/${PARENT}/overview`,
    );
  });

  it("marks a start and duration that were not recorded as estimated", async () => {
    new Server(ok(execution("SUCCESS", { started_at: null, ended_at: null })));
    renderPage();
    await screen.findByRole("heading", { level: 1, name: EXEC });
    expect(fact("Started")).toHaveTextContent(`≈ ${fullTimestamp("2026-10-10T09:59:00.000001Z")} (estimated)`);
    // `created_at` to `updated_at`, the old recipe.
    expect(fact("Duration")).toHaveTextContent("≈ 7 min 5 s (estimated)");
  });

  it("names the tab in the URL, and a tab click moves it", async () => {
    new Server(ok(execution("SUCCESS")));
    renderPage();
    await screen.findByRole("heading", { level: 1, name: EXEC });
    expect(screen.getByRole("tab", { name: "Overview" })).toHaveAttribute("aria-selected", "true");
    await userEvent.click(screen.getByRole("tab", { name: "Logs" }));
    expect(screen.getByTestId("where")).toHaveTextContent(`/explore/${WS}/workflows/executions/${EXEC}/logs`);
    expect(screen.getByRole("tab", { name: "Logs" })).toHaveAttribute("aria-selected", "true");
  });
});

describe("ExecutionDetail: the Topology tab (W7c spec §6.3)", () => {
  it("comes second, in Kestra's order", async () => {
    new Server(ok(execution("SUCCESS")));
    renderPage();
    await screen.findByRole("heading", { level: 1, name: EXEC });
    expect(screen.getAllByRole("tab").map((t) => t.textContent)).toEqual(["Overview", "Topology", "Gantt", "Logs", "Outputs"]);
  });

  it("opens from its URL, and loads the execution's graph", async () => {
    const server = new Server(ok(execution("SUCCESS")));
    renderPage("topology");
    await screen.findByRole("heading", { level: 1, name: EXEC });
    expect(screen.getByRole("tab", { name: "Topology" })).toHaveAttribute("aria-selected", "true");
    // The lazily loaded tab draws the graph's answer: a flow with no tasks.
    expect(await screen.findByText("This flow has no tasks")).toBeInTheDocument();
    expect(server.calls.filter((c) => c.path === `${EXEC_PATH}/graph`)).toHaveLength(1);
  });

  it("is reached by a tab click", async () => {
    new Server(ok(execution("SUCCESS")));
    renderPage();
    await screen.findByRole("heading", { level: 1, name: EXEC });
    await userEvent.click(screen.getByRole("tab", { name: "Topology" }));
    expect(screen.getByTestId("where")).toHaveTextContent(`/explore/${WS}/workflows/executions/${EXEC}/topology`);
  });

  it("still sends an unknown tab to overview", async () => {
    new Server(ok(execution("SUCCESS")));
    renderPage("topologie");
    await screen.findByRole("heading", { level: 1, name: EXEC });
    expect(screen.getByTestId("where")).toHaveTextContent(`/explore/${WS}/workflows/executions/${EXEC}/overview`);
  });
});

describe("ExecutionDetail: polling", () => {
  it("polls every 2 s while the execution is not terminal, and stops once it is", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const server = new Server(ok(execution("RUNNING")), ok(execution("RUNNING")), ok(execution("SUCCESS")));
    renderPage();
    await screen.findByRole("heading", { level: 1, name: EXEC });
    expect(server.executionReads()).toBe(1);
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(server.executionReads()).toBe(2));
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(server.executionReads()).toBe(3));
    expect(await within(screen.getByRole("banner")).findByText("Success")).toBeInTheDocument();
    // Terminal: no further read, however long the page stays open.
    await act(() => vi.advanceTimersByTimeAsync(10_000));
    expect(server.executionReads()).toBe(3);
  });

  it("keeps the last answer through a failed poll, says so with Retry, and clears it when a poll succeeds", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const server = new Server(
      ok(execution("RUNNING")),
      { status: 503, body: { error: "the database is unavailable" } },
      ok(execution("SUCCESS")),
    );
    renderPage();
    await screen.findByRole("heading", { level: 1, name: EXEC });
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(server.executionReads()).toBe(2));
    expect(await screen.findByText(/The execution could not be refreshed: the database is unavailable/)).toBeInTheDocument();
    expect(screen.getByText(/it keeps trying on its own/)).toBeInTheDocument();
    expect(within(screen.getByRole("banner")).getByText("Running")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    // The next read lands within the backoff, succeeds, and clears the error.
    await act(() => vi.advanceTimersByTimeAsync(5000));
    await waitFor(() => expect(server.executionReads()).toBe(3));
    expect(await within(screen.getByRole("banner")).findByText("Success")).toBeInTheDocument();
    expect(screen.queryByText(/could not be refreshed/)).not.toBeInTheDocument();
    await act(() => vi.advanceTimersByTimeAsync(60_000));
    expect(server.executionReads()).toBe(3);
  });

  it("shows a skeleton while loading", () => {
    vi.stubGlobal("fetch", vi.fn(() => new Promise(() => {})));
    renderPage();
    expect(screen.getByTestId("execution-loading")).toBeInTheDocument();
    expect(screen.queryByText(/Loading/)).not.toBeInTheDocument();
  });

  it("says the execution was not found on a 404, with a way back", async () => {
    new Server({ status: 404, body: { error: "not found: execution" } });
    renderPage();
    expect(await screen.findByText("Execution not found")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to executions" })).toHaveAttribute("href", `/explore/${WS}/workflows/executions`);
  });

  it("shows any other first-load error with the server's message and a Retry that reads again", async () => {
    const server = new Server({ status: 503, body: { error: "the database is unavailable" } }, ok(execution("SUCCESS")));
    renderPage();
    expect(await screen.findByText(/could not be loaded: the database is unavailable/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("heading", { level: 1, name: EXEC })).toBeInTheDocument();
    expect(server.executionReads()).toBe(2);
  });
});

describe("ExecutionDetail: permissions", () => {
  it("shows neither action without can_manage, paused or finished", async () => {
    for (const state of ["PAUSED", "FAILED"]) {
      new Server(ok(execution(state, { can_manage: false, source: pauseSource("    pauseDuration: PT1M\n") })));
      const { unmount } = renderPage();
      await screen.findByRole("heading", { level: 1, name: EXEC });
      expect(screen.queryByRole("button", { name: "Resume" })).toBeNull();
      expect(screen.queryByRole("button", { name: "Replay" })).toBeNull();
      unmount();
    }
    // The control: the same execution with the permission shows Replay.
    new Server(ok(execution("FAILED")));
    renderPage();
    expect(await screen.findByRole("button", { name: "Replay" })).toBeEnabled();
  });
});

describe("ExecutionDetail: Resume", () => {
  // `resume_with` as the engine writes it for that pause (`executor::Resume::wire`).
  const paused = (props: string) =>
    execution("PAUSED", {
      source: pauseSource(props),
      runs: [run("gate", "PAUSED", 0, { resume_with: props ? "TERMINAL:SUCCESS" : "NEVER", not_before_ms: props ? 60_000 : null })],
    });

  it("is shown only while PAUSED", async () => {
    new Server(ok(execution("RUNNING")));
    renderPage();
    await screen.findByRole("heading", { level: 1, name: EXEC });
    expect(screen.queryByRole("button", { name: "Resume" })).toBeNull();
    expect(screen.getByRole("button", { name: "Kill" })).toBeInTheDocument();
  });

  it("is enabled for every pause, an unbounded one included, and says what it does", async () => {
    new Server(ok(paused("")));
    renderPage();
    const button = await screen.findByRole("button", { name: "Resume" });
    expect(button).toBeEnabled();
    expect(screen.queryByText(/cannot be resumed/)).toBeNull();
    await userEvent.click(button);
    const dialog = await screen.findByRole("alertdialog", { name: "Resume this execution?" });
    expect(dialog).toHaveTextContent("Every paused task ends now as a success, whatever its duration or timeout");
  });

  it("is disabled, saying why in visible text, when only a LoopUntil waits", async () => {
    const loopSource =
      "id: hello\nnamespace: company.team\ntasks:\n  - id: until\n    type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{ false }}\"\n";
    const server = new Server(
      ok(
        execution("PAUSED", {
          source: loopSource,
          runs: [
            run("until", "PAUSED", 0, { resume_with: "ITERATE", not_before_ms: 3_600_000 }),
            run("work", "SUCCESS", 1, { parent_ix: 0 }),
          ],
        }),
      ),
    );
    renderPage();
    const button = await screen.findByRole("button", { name: "Resume" });
    expect(button).toBeDisabled();
    const reason = "Waiting on a loop condition, not a Pause task";
    expect(screen.getByText(reason)).toBeVisible();
    expect(button).toHaveAccessibleDescription(reason);
    await userEvent.click(button);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(server.calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("is enabled when a Pause waits beside a waiting LoopUntil", async () => {
    new Server(
      ok(
        execution("PAUSED", {
          runs: [
            run("until", "PAUSED", 0, { resume_with: "ITERATE", not_before_ms: 3_600_000 }),
            run("gate", "PAUSED", 1, { resume_with: "TERMINAL:FAILED", not_before_ms: 60_000 }),
          ],
        }),
      ),
    );
    renderPage();
    expect(await screen.findByRole("button", { name: "Resume" })).toBeEnabled();
    expect(screen.queryByText("Waiting on a loop condition, not a Pause task")).toBeNull();
  });

  it.each([
    ["RUNNING", "success", "Resumed the execution"],
    ["SUCCESS", "success", "Resumed the execution"],
    ["FAILED", "warning", "Resumed, but the execution then ended Failed."],
    ["PAUSED", "warning", "Resume was sent, but the execution is still paused. Refresh to see its latest state."],
  ] as const)("asks first, resumes, refetches, and on a %s answer says so (%s)", async (answered, tone, message) => {
    const server = new Server(ok(paused("    pauseDuration: PT1M\n")), ok(paused("    pauseDuration: PT1M\n")), ok(execution("RUNNING")));
    server.resume = { status: 200, body: execution(answered) };
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Resume" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Resume this execution?" });
    // Nothing sent until confirmed.
    expect(server.calls.some((c) => c.method === "POST")).toBe(false);
    const readsBefore = server.executionReads();
    await userEvent.click(within(dialog).getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(server.calls.filter((c) => c.method === "POST" && c.path === `${EXEC_PATH}/resume`)).toHaveLength(1));
    await waitFor(() => expect(server.executionReads()).toBeGreaterThan(readsBefore));
    await waitFor(() => expect(toast[tone]).toHaveBeenCalledWith(message));
    expect(toast[tone === "success" ? "warning" : "success"]).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("keeps the dialog open with the server's message when resume is refused", async () => {
    const server = new Server(ok(paused("    pauseDuration: PT1M\n")));
    server.resume = { status: 409, body: { error: "execution x is RUNNING, not PAUSED; only a paused execution can be resumed" } };
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Resume" }));
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Resume" }));
    expect(await within(dialog).findByText(/only a paused execution can be resumed/)).toBeInTheDocument();
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    expect(toast.success).not.toHaveBeenCalled();
  });
});

describe("ExecutionDetail: Replay", () => {
  it("is not offered while the execution runs, so no reason wraps under the actions (KI-193)", async () => {
    new Server(ok(execution("RUNNING")));
    renderPage();
    expect(await screen.findByRole("button", { name: "Kill" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Replay" })).toBeNull();
    expect(screen.queryByText(/Replay is available/)).toBeNull();
  });

  it("asks first, naming the frozen source and the inputs, and on 201 opens the new execution", async () => {
    const server = new Server(ok(execution("FAILED", { inputs: { who: "world", n: 3 } })));
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Replay" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Replay this execution?" });
    expect(dialog).toHaveTextContent("Replay starts a new execution from this execution's frozen YAML and inputs.");
    expect(dialog).toHaveTextContent("Inputs: who, n.");
    expect(server.calls.some((c) => c.method === "POST")).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "Replay" }));
    await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent(`/explore/${WS}/workflows/executions/${NEW}/overview`));
    expect(await screen.findByRole("heading", { level: 1, name: NEW })).toBeInTheDocument();
    expect(server.calls.filter((c) => c.method === "POST" && c.path === `${EXEC_PATH}/replay`)).toHaveLength(1);
    // Navigating to the result: no toast (FE-ACTION-2).
    expect(toast.success).not.toHaveBeenCalled();
  });

  it.each([
    [409, "the flow is disabled; enable it to run it"],
    [400, "execution x is iteration 2 of a Loop in execution y, not a run of the flow, and cannot be replayed on its own; replay its loop root y"],
  ])("shows the server's message on a %i and stays", async (status, message) => {
    const server = new Server(ok(execution("SUCCESS")));
    server.replay = { status, body: { error: message } };
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Replay" }));
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Replay" }));
    expect(await within(dialog).findByText(message)).toBeInTheDocument();
    expect(screen.getByTestId("where")).toHaveTextContent(`/executions/${EXEC}/overview`);
  });
});

describe("ExecutionDetail: Kill (workflow kill spec §7)", () => {
  const killing = () => execution("KILLING", { ended_at: null });
  const killCalls = (server: Server) => server.calls.filter((c) => c.method === "POST" && c.path === `${EXEC_PATH}/kill`);

  it.each(["RUNNING", "PAUSED", "CREATED"])("is shown and enabled while %s", async (state) => {
    new Server(ok(execution(state, { ended_at: null })));
    renderPage();
    expect(await screen.findByRole("button", { name: "Kill" })).toBeEnabled();
  });

  it("is not shown once the execution has ended, nor without can_manage", async () => {
    for (const state of ["SUCCESS", "KILLED"]) {
      new Server(ok(execution(state)));
      const { unmount } = renderPage();
      // The control: Replay is there, so the actions rendered.
      expect(await screen.findByRole("button", { name: "Replay" })).toBeEnabled();
      expect(screen.queryByRole("button", { name: /^Kill/ })).toBeNull();
      unmount();
    }
    new Server(ok(execution("RUNNING", { can_manage: false })));
    renderPage();
    await screen.findByRole("heading", { level: 1, name: EXEC });
    expect(screen.queryByRole("button", { name: /^Kill/ })).toBeNull();
  });

  it("reads Killing… and is disabled, saying why, while KILLING", async () => {
    const server = new Server(ok(killing()));
    renderPage();
    const button = await screen.findByRole("button", { name: "Killing…" });
    expect(button).toBeDisabled();
    const reason = "The kill was sent; a worker is stopping the running task";
    expect(screen.getByText(reason)).toBeVisible();
    expect(button).toHaveAccessibleDescription(reason);
    expect(within(screen.getByRole("banner")).getByText("Killing")).toBeInTheDocument();
    await userEvent.click(button);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(killCalls(server)).toHaveLength(0);
  });

  it("asks first, with the cascade checkbox on, sends cascade=true, toasts, and follows KILLING to KILLED", async () => {
    const server = new Server(ok(execution("RUNNING")), ok(killing()), ok(execution("KILLED")));
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Kill" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Kill this execution?" });
    const cascade = within(dialog).getByRole("checkbox", { name: "Also kill subflow executions" });
    expect(cascade).toBeChecked();
    // Nothing sent until confirmed.
    expect(killCalls(server)).toHaveLength(0);
    await userEvent.click(within(dialog).getByRole("button", { name: "Kill" }));
    await waitFor(() => expect(killCalls(server)).toHaveLength(1));
    expect(killCalls(server)[0].query.get("cascade")).toBe("true");
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Killing the execution: the running task is being stopped"));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    // The invalidation reads it again: KILLING, which is not terminal, so the page keeps polling...
    expect(await within(screen.getByRole("banner")).findByText("Killing")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Killing…" })).toBeDisabled();
    // ...until the worker has stopped the task.
    expect(await within(screen.getByRole("banner")).findByText("Killed", {}, { timeout: 4000 })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Kill/ })).toBeNull();
  });

  it("sends cascade=false when the checkbox is cleared", async () => {
    const server = new Server(ok(execution("RUNNING")));
    server.kill = { status: 202, body: execution("KILLED") };
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Kill" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Kill this execution?" });
    await userEvent.click(within(dialog).getByRole("checkbox", { name: "Also kill subflow executions" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Kill" }));
    await waitFor(() => expect(killCalls(server)).toHaveLength(1));
    expect(killCalls(server)[0].query.get("cascade")).toBe("false");
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Killed the execution"));
  });

  it("starts the checkbox on again each time the dialog opens", async () => {
    new Server(ok(execution("RUNNING")));
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Kill" }));
    let dialog = await screen.findByRole("alertdialog", { name: "Kill this execution?" });
    await userEvent.click(within(dialog).getByRole("checkbox", { name: "Also kill subflow executions" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await userEvent.click(screen.getByRole("button", { name: "Kill" }));
    dialog = await screen.findByRole("alertdialog", { name: "Kill this execution?" });
    expect(within(dialog).getByRole("checkbox", { name: "Also kill subflow executions" })).toBeChecked();
  });

  it("on a 409 execution_terminated, says so inline in the dialog, not in a toast, reads the execution again, and turns the confirm off", async () => {
    const server = new Server(ok(execution("RUNNING")), ok(execution("SUCCESS")));
    server.kill = {
      status: 409,
      body: { error: `conflict: execution ${EXEC} is SUCCESS and has already ended; there is nothing to kill`, code: "execution_terminated" },
    };
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Kill" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Kill this execution?" });
    const readsBefore = server.executionReads();
    await userEvent.click(within(dialog).getByRole("button", { name: "Kill" }));
    expect(
      await within(dialog).findByText("The execution has already ended, so there is nothing to kill. The page now shows its final state."),
    ).toBeInTheDocument();
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    await waitFor(() => expect(server.executionReads()).toBeGreaterThan(readsBefore));
    expect(toast.success).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
    expect(toast.warning).not.toHaveBeenCalled();
    // Nothing is left to kill: the confirm is off, a second click sends nothing, and Cancel still closes.
    const confirm = within(dialog).getByRole("button", { name: "Kill" });
    expect(confirm).toBeDisabled();
    await userEvent.click(confirm);
    expect(killCalls(server)).toHaveLength(1);
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  });

  it("shows the cascade help in plain words", async () => {
    new Server(ok(execution("RUNNING")));
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Kill" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Kill this execution?" });
    expect(within(dialog).getByRole("checkbox", { name: "Also kill subflow executions" })).toHaveAccessibleDescription(
      "Its loop iterations and subflow executions, and theirs. Unchecked, they keep running.",
    );
  });

  it("keeps the dialog open with the server's message on any other refusal", async () => {
    const server = new Server(ok(execution("RUNNING")));
    server.kill = { status: 403, body: { error: "missing permission workflow:manage" } };
    renderPage();
    await userEvent.click(await screen.findByRole("button", { name: "Kill" }));
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Kill" }));
    expect(await within(dialog).findByText(/missing permission workflow:manage/)).toBeInTheDocument();
    expect(toast.success).not.toHaveBeenCalled();
  });
});

describe("ExecutionDetail: Overview", () => {
  it("lists the inputs, a kestra:/// value as a file row", async () => {
    new Server(
      ok(execution("SUCCESS", { inputs: { who: "world", data: `kestra:///company/team/hello/executions/${EXEC}/inputs/data/orders.csv` } })),
    );
    renderPage();
    const inputs = await screen.findByRole("region", { name: "Inputs" });
    const rows = within(inputs).getAllByRole("row").slice(1);
    expect(rows[0].textContent).toBe("whoworld");
    // The file row (Task 6); this server has no file routes, so the file reads as gone.
    const file = within(rows[1]).getByTestId("file-row");
    expect(within(file).getByText("orders.csv")).toHaveAttribute("title", expect.stringContaining("kestra:///company/team"));
    expect(await within(file).findByText("This file is no longer available")).toBeInTheDocument();
  });

  it("shows the trigger's variables, a structured one as collapsed JSON", async () => {
    new Server(
      ok(
        execution("SUCCESS", {
          execution_trigger: {
            id: "hook",
            type: "io.kestra.plugin.core.trigger.Webhook",
            variables: { body: { order: 42 }, method: "POST" },
          },
        }),
      ),
    );
    renderPage();
    const trigger = await screen.findByRole("region", { name: "Trigger" });
    expect(trigger).toHaveTextContent("Webhook");
    expect(trigger).toHaveTextContent("hook");
    expect(within(trigger).getByText("POST")).toBeInTheDocument();
    expect(within(trigger).queryByText(/"order": 42/)).toBeNull();
    await userEvent.click(within(trigger).getByRole("button", { name: "Show body" }));
    expect(within(trigger).getByText(/"order": 42/)).toBeInTheDocument();
  });

  it("says when no trigger started the execution", async () => {
    new Server(ok(execution("SUCCESS")));
    renderPage();
    const trigger = await screen.findByRole("region", { name: "Trigger" });
    expect(trigger).toHaveTextContent("API");
    expect(trigger).toHaveTextContent("No trigger started this execution");
  });

  it("reads the children with parent=, showing each one's loop index and value, linked", async () => {
    const CHILD = "33333333-0000-4000-8000-000000000003";
    const server = new Server(ok(execution("SUCCESS")));
    server.children = [
      { ...execution("SUCCESS"), id: CHILD, parent_execution_id: EXEC, loop_index: 2, loop_value: "eu-west" },
    ];
    server.childTotal = 120;
    renderPage();
    const children = await screen.findByRole("region", { name: "Child executions" });
    const link = await within(children).findByRole("link", { name: CHILD.slice(0, 8) });
    expect(link).toHaveAttribute("href", `/explore/${WS}/workflows/executions/${CHILD}/overview`);
    const row = link.closest("tr")!;
    expect(row).toHaveTextContent("#2");
    expect(row).toHaveTextContent("eu-west");
    expect(row).toHaveTextContent("Loop iteration");
    expect(children).toHaveTextContent("Showing the first 1 of 120 child executions.");
    const list = server.calls.find((c) => c.method === "GET" && c.path === `${BASE}/executions`)!;
    expect(list.query.get("parent")).toBe(EXEC);
    expect(list.query.get("root_only")).toBeNull();
  });

  it("says when there are no child executions", async () => {
    new Server(ok(execution("SUCCESS")));
    renderPage();
    const children = await screen.findByRole("region", { name: "Child executions" });
    expect(await within(children).findByText("No child executions")).toBeInTheDocument();
  });

  it("lists the task runs in run order, each opening the Gantt with that run selected", async () => {
    new Server(
      ok(
        execution("SUCCESS", {
          runs: [
            run("extract", "SUCCESS", 0, { started_at: "2026-10-10T10:00:00Z", ended_at: "2026-10-10T10:00:45Z" }),
            run("each", "SUCCESS", 1, { value: "eu-west", started_at: "2026-10-10T10:00:45Z", ended_at: "2026-10-10T10:01:00Z" }),
          ],
        }),
      ),
    );
    renderPage();
    const runs = await screen.findByRole("region", { name: "Task runs" });
    const rows = within(runs).getAllByRole("row").slice(1);
    expect(rows.map((r) => r.textContent)).toEqual(["extract—Success45 s", "eacheu-westSuccess15 s"]);
    await userEvent.click(within(runs).getByRole("link", { name: "each" }));
    expect(screen.getByTestId("where")).toHaveTextContent(`/explore/${WS}/workflows/executions/${EXEC}/gantt?run=1`);
    // The link lands on the Gantt with that run's panel open (Task 4).
    const grid = await screen.findByRole("treegrid", { name: "Task runs over time" });
    expect(within(grid).getAllByRole("row").slice(1).map((r) => r.getAttribute("aria-selected"))).toEqual(["false", "true"]);
    expect(within(screen.getByTestId("gantt-run-panel")).getByRole("heading", { name: "each eu-west" })).toBeInTheDocument();
  });

  it("says a running task run's duration is so far", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date("2026-10-10T10:00:30Z"));
    new Server(ok(execution("RUNNING", { runs: [run("slow", "RUNNING", 0, { started_at: "2026-10-10T10:00:00Z" })] })));
    renderPage();
    const runs = await screen.findByRole("region", { name: "Task runs" });
    const row = within(runs).getAllByRole("row")[1];
    expect(row).toHaveTextContent(/slow—Running\d+ s so far/);
  });

  it("says when the execution has no task runs yet", async () => {
    new Server(ok(execution("SUCCESS", { runs: [] })));
    renderPage();
    expect(await screen.findByText("No task runs yet")).toBeInTheDocument();
  });
});
