import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import ExecutionDetail from "@/routes/workflows/ExecutionDetail";
import { EXEC, line, LogServer, WS } from "@/components/workflows/executions/logs/__tests__/logServer";

/**
 * The execution page and its log view follow a run by EventSource (7e #2, #3) and
 * fall back to polling when the stream fails. Through the real page, hooks and
 * log route served from memory (FE-TEST-1).
 */
const EXEC_PATH = `/api/ws/${WS}/workflows/executions/${EXEC}`;

class FakeEventSource {
  static all: FakeEventSource[] = [];
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  constructor(public url: string) {
    FakeEventSource.all.push(this);
  }
  close() {
    this.closed = true;
  }
}
const stream = (suffix: string) => FakeEventSource.all.find((s) => s.url.endsWith(suffix));

const execution = {
  id: EXEC,
  flow_id: null,
  namespace: "company.team",
  flow_key: "hello",
  state: "RUNNING",
  history: ["CREATED", "RUNNING"],
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
  ended_at: null,
  updated_at: "2026-10-10T10:00:00.000001Z",
  source: "id: hello\n",
  runs: [],
  idle: false,
  can_manage: false,
  worker_enabled: true,
};

function renderAt(tab: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/explore/${WS}/workflows/executions/${EXEC}/${tab}`]}>
        <Routes>
          <Route path="/explore/:workspace/workflows/executions/:id/:tab" element={<ExecutionDetail />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

let server: LogServer;
let executionReads: number;
beforeEach(() => {
  FakeEventSource.all = [];
  vi.stubGlobal("EventSource", FakeEventSource);
  server = new LogServer().install();
  executionReads = 0;
  const base = vi.mocked(fetch).getMockImplementation()!;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init?: RequestInit) => {
      if (String(input).split("?")[0] === EXEC_PATH) executionReads++;
      return base(input, init);
    }),
  );
  server.others.set(EXEC_PATH, { status: 200, body: execution });
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("following the execution by EventSource", () => {
  it("opens the follow stream and shows a state it pushes", async () => {
    renderAt("logs");
    await screen.findByTestId("execution-summary");
    await waitFor(() => expect(stream(`/executions/${EXEC}/follow`)).toBeDefined());
    const before = executionReads;
    act(() => stream(`/executions/${EXEC}/follow`)!.onmessage!({ data: JSON.stringify({ ...execution, state: "WARNING", history: ["CREATED", "RUNNING", "WARNING"] }) }));
    expect(await screen.findByText(/warning/i)).toBeInTheDocument();
    // A push is the answer: no read of the execution follows it.
    expect(executionReads).toBe(before);
  });

  it("falls back to polling the execution when the stream fails", async () => {
    renderAt("logs");
    await screen.findByTestId("execution-summary");
    await waitFor(() => expect(stream(`/executions/${EXEC}/follow`)).toBeDefined());
    const before = executionReads;
    act(() => stream(`/executions/${EXEC}/follow`)!.onerror!());
    await waitFor(() => expect(executionReads).toBeGreaterThan(before));
  });
});

describe("a finished execution", () => {
  it("opens no stream and reads the execution once", async () => {
    server.others.set(EXEC_PATH, {
      status: 200,
      body: { ...execution, state: "SUCCESS", history: ["CREATED", "RUNNING", "SUCCESS"], ended_at: "2026-10-10T10:01:00.000001Z", idle: true },
    });
    server.state = "SUCCESS";
    server.complete = true;
    renderAt("logs");
    await screen.findByTestId("execution-summary");
    await new Promise((r) => setTimeout(r, 100));
    expect(FakeEventSource.all).toHaveLength(0);
    expect(executionReads).toBe(1);
  });
});

describe("following the log by EventSource", () => {
  it("opens the log stream, reads when it pushes, and keeps reading by polling once it fails", async () => {
    server.lines = [line(1, { message: "first" })];
    renderAt("logs");
    expect(await screen.findByText("first")).toBeInTheDocument();
    await waitFor(() => expect(stream(`/executions/${EXEC}/logs/follow`)).toBeDefined());

    // Live: a push makes it read the new line now, without waiting for the timer.
    server.lines = [line(1, { message: "first" }), line(2, { message: "second" })];
    act(() => stream(`/executions/${EXEC}/logs/follow`)!.onmessage!({ data: "{}" }));
    expect(await screen.findByText("second")).toBeInTheDocument();

    // Failed: the timer polling resumes (the read happens straight away, then every 2 s).
    const reads = server.queries.length;
    server.lines = [...server.lines, line(3, { message: "third" })];
    act(() => stream(`/executions/${EXEC}/logs/follow`)!.onerror!());
    expect(await screen.findByText("third")).toBeInTheDocument();
    expect(server.queries.length).toBeGreaterThan(reads);
  });
});
