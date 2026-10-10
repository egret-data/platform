import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import ExecutionMinimal from "@/routes/workflows/ExecutionMinimal";

const WS = "eng";
const EXEC = "5e5e5e5e-0000-4000-8000-0000000000aa";
const FLOW = "0a7c1d2e-0000-4000-8000-000000000001";
const PATH = `/api/ws/${WS}/workflows/executions/${EXEC}`;

function run(task_id: string, state: string, run_ix: number) {
  return {
    id: `r-${run_ix}`,
    execution_id: EXEC,
    run_ix,
    task_ix: run_ix,
    task_id,
    state,
    history: [],
    attempts: [],
    outputs: {},
    created_at: "2026-10-10T10:00:00.000001Z",
    updated_at: "2026-10-10T10:00:01.000001Z",
  };
}

function execution(state: string, runs = [run("extract", "SUCCESS", 0), run("load", "RUNNING", 1)]) {
  return {
    id: EXEC,
    flow_id: FLOW,
    namespace: "company.team",
    flow_key: "hello",
    state,
    history: [],
    inputs: {},
    seq: 1,
    execution_trigger: {},
    attempt_number: 1,
    origin_offset_ms: 0,
    created_at: "2026-10-10T10:00:00.000001Z",
    updated_at: "2026-10-10T10:01:05.000001Z",
    source: "",
    runs,
  };
}

/** Serves `replies` in turn, the last one for good; records each GET. */
function serve(...replies: { status: number; body: unknown }[]) {
  const gets: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (input: string) => {
    gets.push(String(input));
    const reply = replies[Math.min(gets.length - 1, replies.length - 1)];
    return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
  }));
  return gets;
}

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/explore/${WS}/workflows/executions/${EXEC}`]}>
        <Routes>
          <Route path="/explore/:workspace/workflows/executions/:id" element={<ExecutionMinimal />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("ExecutionMinimal", () => {
  it("shows the id, the flow with a link back, the namespace, the state, start and duration, and the runs", async () => {
    serve({ status: 200, body: execution("SUCCESS") });
    renderPage();
    expect(await screen.findByText(EXEC)).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "Execution" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "company.team.hello" })).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/flows/${FLOW}/edit`,
    );
    expect(screen.getByText("company.team")).toBeInTheDocument();
    const summary = screen.getByTestId("execution-summary");
    expect(within(summary).getByText("Success")).toBeInTheDocument();
    expect(within(summary).getByText(new Date("2026-10-10T10:00:00.000Z").toLocaleString())).toBeInTheDocument();
    // Terminal: created to the last update, 1 min 5 s.
    expect(within(summary).getByText("1 min 5 s")).toBeInTheDocument();
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows.map((r) => r.textContent)).toEqual(["extractSuccess", "loadRunning"]);
  });

  it("polls every 2 s while the execution is not terminal, and stops once it is", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const gets = serve(
      { status: 200, body: execution("RUNNING") },
      { status: 200, body: execution("RUNNING") },
      { status: 200, body: execution("SUCCESS") },
    );
    renderPage();
    await screen.findByText("Running", { selector: "[data-testid=execution-summary] *" });
    expect(gets).toHaveLength(1);
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(gets).toHaveLength(2));
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await waitFor(() => expect(gets).toHaveLength(3));
    expect(await within(screen.getByTestId("execution-summary")).findByText("Success")).toBeInTheDocument();
    // Terminal: no further request, however long the page stays open.
    await act(() => vi.advanceTimersByTimeAsync(10_000));
    expect(gets).toHaveLength(3);
    expect(gets.every((g) => g === PATH)).toBe(true);
  });

  it("shows a skeleton while loading", () => {
    vi.stubGlobal("fetch", vi.fn(() => new Promise(() => {})));
    renderPage();
    expect(screen.getByTestId("execution-loading")).toBeInTheDocument();
    expect(screen.queryByText(/Loading/)).not.toBeInTheDocument();
  });

  it("says when the execution has no task runs yet", async () => {
    serve({ status: 200, body: execution("SUCCESS", []) });
    renderPage();
    expect(await screen.findByText("No task runs yet")).toBeInTheDocument();
  });

  it("says the execution was not found on a 404, with a way back", async () => {
    serve({ status: 404, body: { error: "not found: execution" } });
    renderPage();
    expect(await screen.findByText("Execution not found")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to flows" })).toHaveAttribute("href", `/explore/${WS}/workflows/flows`);
  });

  it("shows any other error with the server's message and a retry that reads again", async () => {
    const gets = serve(
      { status: 503, body: { error: "the database is unavailable" } },
      { status: 200, body: execution("SUCCESS") },
    );
    renderPage();
    expect(await screen.findByText(/the database is unavailable/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText(EXEC)).toBeInTheDocument();
    expect(gets).toHaveLength(2);
  });
});
