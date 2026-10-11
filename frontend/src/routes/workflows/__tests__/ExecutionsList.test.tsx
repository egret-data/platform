import { afterEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ExecutionListItemDto, FlowListItemDto } from "@/api/generated/model";
import { TooltipProvider } from "@/components/ui/tooltip";
import ExecutionsList from "@/routes/workflows/ExecutionsList";

const WS = "eng";
const FLOW = "0a7c1d2e-0000-4000-8000-000000000001";
const OTHER_FLOW = "0a7c1d2e-0000-4000-8000-000000000002";
const LIST = `/api/ws/${WS}/workflows/executions`;

function exec(id: string, over: Partial<ExecutionListItemDto> = {}): ExecutionListItemDto {
  return {
    id,
    flow_id: FLOW,
    namespace: "company.team",
    flow_key: "hello",
    state: "SUCCESS",
    history: ["CREATED", "RUNNING", "SUCCESS"],
    inputs: {},
    seq: 1,
    execution_trigger: null as unknown as ExecutionListItemDto["execution_trigger"],
    attempt_number: 1,
    origin_offset_ms: 0,
    started_at: "2026-10-11T10:00:05.000001Z",
    ended_at: "2026-10-11T10:01:10.000001Z",
    created_at: "2026-10-11T10:00:00.000001Z",
    updated_at: "2026-10-11T10:01:10.000001Z",
    ...over,
  };
}

const ROWS: ExecutionListItemDto[] = [
  exec("aaaaaaaa-0000-4000-8000-000000000001"),
  exec("bbbbbbbb-0000-4000-8000-000000000002", {
    flow_id: OTHER_FLOW,
    namespace: "company",
    flow_key: "nightly",
    state: "FAILED",
    history: ["CREATED", "RUNNING", "FAILED"],
    execution_trigger: { id: "every-night", type: "io.kestra.plugin.core.trigger.Schedule", variables: {} },
    // Recorded before migration 0180: no start, no end.
    started_at: null,
    ended_at: null,
    updated_at: "2026-10-11T10:00:45.000001Z",
  }),
  exec("cccccccc-0000-4000-8000-000000000003", {
    flow_id: null,
    flow_key: "deleted-flow",
    state: "QUEUED",
    history: ["CREATED", "QUEUED"],
    started_at: null,
    ended_at: null,
  }),
];

const FLOWS: FlowListItemDto[] = [
  { id: OTHER_FLOW, namespace: "company", flow_id: "nightly", state: "live", created_at: "", updated_at: "" } as FlowListItemDto,
  { id: FLOW, namespace: "company.team", flow_id: "hello", state: "draft", created_at: "", updated_at: "" } as FlowListItemDto,
];

type Reply = { status: number; body: unknown };
const page = (rows: ExecutionListItemDto[], total = rows.length, offset = 0): Reply => ({
  status: 200,
  body: { rows, total, limit: 50, offset },
});

/** The server's page of a `total`-row match at `offset`: as many rows as it really returns. */
function slice(total: number, offset: number): Reply {
  const n = Math.max(0, Math.min(50, total - offset));
  const rows = Array.from({ length: n }, (_, i) => {
    const k = String(offset + i).padStart(12, "0");
    return exec(`eeeeeeee-0000-4000-8000-${k}`);
  });
  return page(rows, total, offset);
}

/**
 * Serves `GET …/executions` from `answer` and `GET …/flows` from `FLOWS`,
 * through the real hooks, mutator and `apiFetch` (FE-TEST-1). Returns every
 * executions request's query, in order.
 */
function serve(answer: (q: URLSearchParams, n: number) => Reply | Promise<Reply>, flows: Reply = { status: 200, body: FLOWS }) {
  const lists: URLSearchParams[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) => {
      const url = new URL(String(input), "http://x");
      let reply: Reply;
      if (url.pathname === LIST) {
        lists.push(url.searchParams);
        reply = await answer(url.searchParams, lists.length);
      } else if (url.pathname === `/api/ws/${WS}/workflows/flows`) {
        reply = flows;
      } else {
        reply = { status: 404, body: { error: `unexpected ${url.pathname}` } };
      }
      return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
    }),
  );
  return lists;
}

function Where() {
  const l = useLocation();
  return <output data-testid="where">{`${l.pathname}${l.search}`}</output>;
}
const search = () => new URLSearchParams(screen.getByTestId("where").textContent!.split("?")[1] ?? "");

function renderList(at = `/explore/${WS}/workflows/executions`, pinnedFlowId?: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[at]}>
          <Routes>
            <Route
              path="/explore/:workspace/workflows/executions"
              element={
                <>
                  <ExecutionsList />
                  <Where />
                </>
              }
            />
            <Route
              path="/explore/:workspace/workflows/flows/:id/executions"
              element={
                <>
                  <ExecutionsList pinnedFlowId={pinnedFlowId} />
                  <Where />
                </>
              }
            />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const bodyRows = () => within(screen.getByTestId("executions-table")).getAllByRole("row").slice(1);
const cell = (row: HTMLElement, column: string) => {
  const headers = within(screen.getByTestId("executions-table")).getAllByRole("columnheader").map((h) => h.textContent);
  return within(row).getAllByRole("cell")[headers.indexOf(column)];
};

let visibility: DocumentVisibilityState = "visible";
function setVisibility(v: DocumentVisibilityState) {
  visibility = v;
  Object.defineProperty(document, "visibilityState", { configurable: true, get: () => visibility });
  document.dispatchEvent(new Event("visibilitychange", { bubbles: true }));
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  setVisibility("visible");
});

describe("ExecutionsList: columns", () => {
  it("shows id, flow, namespace, state, start, duration and trigger, in that order", async () => {
    serve(() => page(ROWS));
    renderList();
    await screen.findByTestId("executions-table");
    expect(screen.getByRole("heading", { level: 1, name: "Executions" })).toBeInTheDocument();
    const headers = within(screen.getByTestId("executions-table")).getAllByRole("columnheader").map((h) => h.textContent);
    expect(headers).toEqual(["ID", "Flow", "Namespace", "State", "Start", "Duration", "Trigger"]);
    expect(bodyRows()).toHaveLength(3);
  });

  it("links the id to the execution and the flow to its editor; a deleted flow is its name without a link", async () => {
    serve(() => page(ROWS));
    renderList();
    await screen.findByTestId("executions-table");
    const [first, , third] = bodyRows();
    const idLink = within(cell(first, "ID")).getByRole("link");
    expect(idLink).toHaveAttribute("href", `/explore/${WS}/workflows/executions/${ROWS[0].id}`);
    expect(idLink).toHaveTextContent("aaaaaaaa");
    expect(idLink).toHaveAttribute("title", ROWS[0].id);
    expect(within(cell(first, "Flow")).getByRole("link", { name: "hello" })).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/flows/${FLOW}/edit`,
    );
    expect(within(cell(third, "Flow")).queryByRole("link")).toBeNull();
    expect(cell(third, "Flow")).toHaveTextContent("deleted-flow");
    expect(cell(first, "Namespace")).toHaveTextContent("company.team");
  });

  it("shows the state chip, and the trigger as a word beside its icon", async () => {
    serve(() => page(ROWS));
    renderList();
    await screen.findByTestId("executions-table");
    const [first, second] = bodyRows();
    expect(within(cell(first, "State")).getByText("Success")).toBeInTheDocument();
    expect(within(cell(second, "State")).getByText("Failed")).toBeInTheDocument();
    expect(cell(first, "Trigger")).toHaveTextContent("API");
    expect(cell(second, "Trigger")).toHaveTextContent("Schedule");
    expect(cell(second, "Trigger").querySelector("svg")).toHaveAttribute("aria-hidden", "true");
  });

  it("measures duration from started_at to ended_at, estimates a legacy row from created_at, and shows a dash for one that never ran", async () => {
    serve(() => page(ROWS));
    renderList();
    await screen.findByTestId("executions-table");
    const [exact, legacy, queued] = bodyRows();
    // 10:00:05 → 10:01:10, not created_at → updated_at (1 min 10 s).
    expect(cell(exact, "Duration")).toHaveTextContent("1 min 5 s");
    expect(cell(exact, "Duration").querySelector("[title]")).toBeNull();
    expect(cell(legacy, "Duration")).toHaveTextContent("45 s");
    expect(within(cell(legacy, "Duration")).getByTitle(/estimated/i)).toBeInTheDocument();
    expect(cell(queued, "Duration")).toHaveTextContent("—");
    // Start: the recorded start on hover; the queued row says it has not started.
    expect(within(cell(queued, "Start")).getByTitle(/not started/i)).toBeInTheDocument();
    expect(within(cell(legacy, "Start")).getByTitle(/estimated/i)).toBeInTheDocument();
  });

  it("ticks a running row's duration", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true, now: Date.parse("2026-10-11T10:00:15.000Z") });
    serve(() => page([exec("dddddddd-0000-4000-8000-000000000004", { state: "RUNNING", history: ["CREATED", "RUNNING"], ended_at: null })]));
    renderList();
    await screen.findByTestId("executions-table");
    expect(cell(bodyRows()[0], "Duration")).toHaveTextContent(/^1[0-1] s$/);
    await act(() => vi.advanceTimersByTimeAsync(3000));
    expect(cell(bodyRows()[0], "Duration")).toHaveTextContent(/^1[3-4] s$/);
  });
});

describe("ExecutionsList: the flow's Executions tab", () => {
  it("pins the flow, hides the flow column and the flow and namespace filters, and has no page heading", async () => {
    const lists = serve(() => page([ROWS[0]]));
    renderList(`/explore/${WS}/workflows/flows/${FLOW}/executions?flow=${OTHER_FLOW}`, FLOW);
    await screen.findByTestId("executions-table");
    expect(lists[0].get("flow_id")).toBe(FLOW);
    expect(lists[0].get("namespace")).toBeNull();
    const headers = within(screen.getByTestId("executions-table")).getAllByRole("columnheader").map((h) => h.textContent);
    expect(headers).toEqual(["ID", "Namespace", "State", "Start", "Duration", "Trigger"]);
    expect(screen.queryByRole("heading", { level: 1 })).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Flow" })).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Namespace" })).toBeNull();
  });

  it("ignores a flow or namespace left in the URL: an empty flow says it has not run, not that filters match nothing", async () => {
    const lists = serve(() => page([]));
    renderList(`/explore/${WS}/workflows/flows/${FLOW}/executions?flow=${OTHER_FLOW}&namespace=company&subns=1`, FLOW);
    expect(await screen.findByText("This flow has not run yet")).toBeInTheDocument();
    expect(screen.queryByText("No executions match the current filters.")).toBeNull();
    expect(screen.queryByRole("button", { name: "Clear filters" })).toBeNull();
    expect(lists[0].get("flow_id")).toBe(FLOW);
    expect(lists[0].get("namespace")).toBeNull();
    expect(lists[0].get("namespace_prefix")).toBeNull();
  });

  it("says the flow has not run yet when it has no executions, without a link away", async () => {
    serve(() => page([]));
    renderList(`/explore/${WS}/workflows/flows/${FLOW}/executions`, FLOW);
    expect(await screen.findByText("This flow has not run yet")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Go to flows" })).toBeNull();
  });
});

describe("ExecutionsList: filters in the URL", () => {
  it("sends root_only=true by default, and drops it when Show child executions is switched on", async () => {
    const lists = serve(() => page(ROWS));
    renderList();
    await screen.findByTestId("executions-table");
    expect(lists[0].get("root_only")).toBe("true");
    const sw = screen.getByRole("switch", { name: "Show child executions" });
    expect(sw).toHaveAttribute("aria-checked", "false");
    await userEvent.click(sw);
    await waitFor(() => expect(lists.length).toBe(2));
    expect(lists[1].get("root_only")).toBeNull();
    expect(search().get("child")).toBe("1");
    expect(screen.getByRole("switch", { name: "Show child executions" })).toHaveAttribute("aria-checked", "true");
  });

  it("writes a picked state to the URL and the request, and adds a second as a comma list", async () => {
    const lists = serve(() => page(ROWS));
    renderList();
    await screen.findByTestId("executions-table");
    await userEvent.click(screen.getByRole("button", { name: /^State/ }));
    await userEvent.click(await screen.findByRole("menuitemcheckbox", { name: "Failed" }));
    await waitFor(() => expect(search().get("state")).toBe("FAILED"));
    await userEvent.click(screen.getByRole("menuitemcheckbox", { name: "Running" }));
    await waitFor(() => expect(search().get("state")).toBe("FAILED,RUNNING"));
    await waitFor(() => expect(lists.at(-1)!.get("state")).toBe("FAILED,RUNNING"));
  });

  it("reads every filter back from a shared URL and sends it", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true, now: Date.parse("2026-10-11T12:00:00.000Z") });
    const lists = serve(() => slice(120, 50));
    renderList(`/explore/${WS}/workflows/executions?state=RUNNING,FAILED&namespace=company&flow=${OTHER_FLOW}&time=24h&child=1&page=2`);
    await screen.findByTestId("executions-table");
    const q = lists[0];
    expect(q.get("state")).toBe("RUNNING,FAILED");
    expect(q.get("namespace")).toBe("company");
    expect(q.get("flow_id")).toBe(OTHER_FLOW);
    expect(Date.parse(q.get("since")!)).toBeGreaterThanOrEqual(Date.parse("2026-10-10T12:00:00.000Z"));
    expect(Date.parse(q.get("since")!)).toBeLessThan(Date.parse("2026-10-10T12:00:05.000Z"));
    expect(q.get("until")).toBeNull();
    expect(q.get("root_only")).toBeNull();
    expect(q.get("offset")).toBe("50");
    expect(q.get("limit")).toBe("50");
    // The controls show what the URL says.
    expect(screen.getByRole("button", { name: /^State/ })).toHaveTextContent("Running, Failed");
    expect(screen.getByRole("combobox", { name: "Namespace" })).toHaveTextContent("company");
    expect(screen.getByRole("combobox", { name: "Flow" })).toHaveTextContent("company.nightly");
    expect(screen.getByRole("combobox", { name: "Time" })).toHaveTextContent("Last 24 hours");
    expect(screen.getByText(/Page 2 of 3/)).toBeInTheDocument();
  });

  it("writes a custom window to the URL as UTC instants", async () => {
    const lists = serve(() => page(ROWS));
    renderList(`/explore/${WS}/workflows/executions?time=custom`);
    await screen.findByTestId("executions-table");
    // A change event: jsdom drops a datetime-local value typed one key at a time.
    fireEvent.change(screen.getByLabelText("From"), { target: { value: "2026-10-01T08:00" } });
    const expected = new Date("2026-10-01T08:00").toISOString();
    await waitFor(() => expect(search().get("since")).toBe(expected));
    await waitFor(() => expect(lists.at(-1)!.get("since")).toBe(expected));
  });

  it("sends the namespace as namespace_prefix when child namespaces are included", async () => {
    const lists = serve(() => page(ROWS));
    renderList(`/explore/${WS}/workflows/executions?namespace=company`);
    await screen.findByTestId("executions-table");
    expect(lists[0].get("namespace")).toBe("company");
    expect(lists[0].get("namespace_prefix")).toBeNull();
    await userEvent.click(screen.getByRole("checkbox", { name: "Include child namespaces" }));
    await waitFor(() => expect(lists.at(-1)!.get("namespace_prefix")).toBe("company"));
    expect(lists.at(-1)!.get("namespace")).toBeNull();
    expect(search().get("subns")).toBe("1");
  });

  it("still sends namespace_prefix when the flow list (the pickers' source) fails to load", async () => {
    const lists = serve(() => page(ROWS), { status: 503, body: { error: "the database is unavailable" } });
    renderList(`/explore/${WS}/workflows/executions?namespace=company&subns=1`);
    await screen.findByTestId("executions-table");
    expect(lists).toHaveLength(1);
    expect(lists[0].get("namespace_prefix")).toBe("company");
    expect(screen.getByRole("checkbox", { name: "Include child namespaces" })).toBeChecked();
  });
});

describe("ExecutionsList: states", () => {
  it("shows a skeleton while the first page loads", () => {
    serve(() => new Promise<Reply>(() => {}));
    renderList();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
  });

  it("says there are no executions yet, with a way to the flows, when nothing is filtered", async () => {
    serve(() => page([]));
    renderList();
    expect(await screen.findByText("No executions yet")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to flows" })).toHaveAttribute("href", `/explore/${WS}/workflows/flows`);
    expect(screen.queryByText(/match the current filters/)).toBeNull();
  });

  it("says a filter matched nothing, not that there are none, and clears the filters on request", async () => {
    const lists = serve((q) => (q.get("state") ? page([]) : page(ROWS)));
    renderList(`/explore/${WS}/workflows/executions?state=KILLED&child=1&page=2`);
    expect(await screen.findByText("No executions match the current filters.")).toBeInTheDocument();
    expect(screen.queryByText("No executions yet")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    await screen.findByTestId("executions-table");
    expect(search().get("state")).toBeNull();
    expect(search().get("page")).toBeNull();
    // The child switch is a view, not a filter: clearing keeps it.
    expect(search().get("child")).toBe("1");
    expect(lists.at(-1)!.get("state")).toBeNull();
  });

  it("shows the server's error, never the empty state, when the first load fails, and retries", async () => {
    const lists = serve((_, n) => (n === 1 ? { status: 503, body: { error: "the database is unavailable" } } : page(ROWS)));
    renderList();
    expect(await screen.findByText(/the database is unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("No executions yet")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByTestId("executions-table");
    expect(lists).toHaveLength(2);
  });

  it("keeps the last rows when a refresh fails, and says so", async () => {
    serve((_, n) => (n === 1 ? page(ROWS) : { status: 503, body: { error: "the database is unavailable" } }));
    renderList();
    await screen.findByTestId("executions-table");
    await userEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText(/could not be refreshed: the database is unavailable/)).toBeInTheDocument();
    expect(bodyRows()).toHaveLength(3);
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});

describe("ExecutionsList: paging", () => {
  it("pages by total: range, page count, Next writes the page to the URL and asks for the next offset", async () => {
    const lists = serve((q) => slice(120, Number(q.get("offset"))));
    renderList();
    await screen.findByTestId("executions-table");
    expect(screen.getByText(/Showing 1–50 of 120/)).toBeInTheDocument();
    expect(screen.getByText(/Page 1 of 3/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Previous/ })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: /Next/ }));
    await waitFor(() => expect(search().get("page")).toBe("2"));
    await waitFor(() => expect(lists.at(-1)!.get("offset")).toBe("50"));
    expect(await screen.findByText(/Showing 51–100 of 120/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Next/ }));
    expect(await screen.findByText(/Page 3 of 3/)).toBeInTheDocument();
    expect(screen.getByText(/Showing 101–120 of 120/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Next/ })).toBeDisabled();
  });

  it("shows no pager when one page holds everything", async () => {
    serve(() => page(ROWS));
    renderList();
    await screen.findByTestId("executions-table");
    expect(screen.queryByRole("button", { name: /Next/ })).toBeNull();
  });

  it("counts the matching total and the states on this page", async () => {
    serve(() => page(ROWS, 120));
    renderList();
    await screen.findByTestId("executions-table");
    // The strip spaces its parts with CSS gaps, not text: read each number with its word.
    const strip = screen.getByTestId("executions-summary");
    expect(strip).toHaveTextContent(/^120\s*executions·3\s*on this page·1\s*queued·1\s*success·1\s*failed$/);
  });
});

describe("ExecutionsList: polling and refresh", () => {
  it("polls every 5 s while a row is not terminal, and stops once every row is", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const running = exec("dddddddd-0000-4000-8000-000000000004", { state: "RUNNING", history: ["CREATED", "RUNNING"], ended_at: null });
    const lists = serve((_, n) => (n < 3 ? page([running, ROWS[0]]) : page([{ ...running, state: "SUCCESS" }, ROWS[0]])));
    renderList();
    await screen.findByTestId("executions-table");
    expect(lists).toHaveLength(1);
    await act(() => vi.advanceTimersByTimeAsync(5000));
    await waitFor(() => expect(lists).toHaveLength(2));
    await act(() => vi.advanceTimersByTimeAsync(5000));
    await waitFor(() => expect(lists).toHaveLength(3));
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(lists).toHaveLength(3);
  });

  it("does not poll a page whose rows are all terminal", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const lists = serve(() => page(ROWS.slice(0, 2)));
    renderList();
    await screen.findByTestId("executions-table");
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(lists).toHaveLength(1);
  });

  it("stops polling while the browser tab is hidden, and resumes when it is shown", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const running = exec("dddddddd-0000-4000-8000-000000000004", { state: "RUNNING", history: ["CREATED", "RUNNING"], ended_at: null });
    const lists = serve(() => page([running]));
    renderList();
    await screen.findByTestId("executions-table");
    act(() => setVisibility("hidden"));
    await act(() => vi.advanceTimersByTimeAsync(20_000));
    expect(lists).toHaveLength(1);
    act(() => setVisibility("visible"));
    await act(() => vi.advanceTimersByTimeAsync(5000));
    await waitFor(() => expect(lists.length).toBeGreaterThan(1));
  });

  it("says how long ago it was updated, and Refresh reads again and resets it", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const lists = serve(() => page(ROWS.slice(0, 2)));
    renderList();
    await screen.findByTestId("executions-table");
    expect(screen.getByTestId("executions-updated")).toHaveTextContent(/^Updated [0-1] s ago$/);
    await act(() => vi.advanceTimersByTimeAsync(12_000));
    expect(screen.getByTestId("executions-updated")).toHaveTextContent(/^Updated 1[2-3] s ago$/);
    await userEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(lists).toHaveLength(2));
    await waitFor(() => expect(screen.getByTestId("executions-updated")).toHaveTextContent(/^Updated [0-1] s ago$/));
  });
});
