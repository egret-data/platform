import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Route, Routes, useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import EngineDetail from "@/routes/control-plane/EngineDetail";
import {
  ADMIN,
  CATALOGS,
  Harness,
  LocationProbe,
  MODELLED_PLUGINS,
  PROFILES,
  userWith,
} from "./externalEngineFixtures";

vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn(), warning: vi.fn() }),
}));

beforeEach(() => {
  vi.mocked(toast.success).mockClear();
  vi.mocked(toast.error).mockClear();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const T = { timeout: 3000 };
const BASE = "/api/control-plane/engines/x1/query-log";
const HIDDEN = "Hidden: needs engine:audit";

const SR = {
  id: "x1",
  name: "lake-sr",
  type: "starrocks",
  namespace: "engines",
  status: { phase: "running", message: null, pods: [] },
  spec: { starrocksConfig: {} },
  origin: "provisioned",
  notGoverned: false,
  securityUpgradeNeeded: false,
  securityUpgradeReasons: [] as string[],
};

const TRINO = { ...SR, name: "lake-trino", type: "trino", spec: { imageVersion: "latest" } };

const READY = { state: "ready", lastEventAt: "2026-11-11T08:00:00Z", retentionDays: 30 };

/** A statement that ran for bob through egret's delegated session user. */
const DELEGATED = {
  queryId: "11111111-1111-1111-1111-111111111111",
  timestamp: "2026-11-11T08:00:00Z",
  user: "egret_svc",
  authorizedUser: "bob",
  state: "EOF",
  command: "Query",
  queryTimeMs: 1500,
  returnRows: 1234,
  statement: "SELECT *\nFROM sales.orders",
  statementTruncated: false,
};

/** A statement alice ran as herself; its error quotes the statement. */
const DIRECT_FAILED = {
  queryId: "22222222-2222-2222-2222-222222222222",
  timestamp: "2026-11-11T07:00:00Z",
  user: "alice",
  authorizedUser: "alice",
  state: "ERR",
  command: "Query",
  queryTimeMs: 12,
  returnRows: 0,
  errorMessage: "Unexpected input 'xyz'",
  statement: "SELEC xyz",
  statementTruncated: false,
};

/** The same entry as a reader without engine:audit gets it: both texts absent. */
const { statement: _s, errorMessage: _e, ...DIRECT_FAILED_WITHHELD } = DIRECT_FAILED;
void _s;
void _e;

interface Recorded {
  method: string;
  path: string;
  /** The query string's parameters; the routes match on the path alone. */
  query: Record<string, string>;
}
type Reply = { status?: number; body?: unknown };
type Handler = Reply | ((req: Recorded) => Reply);

/**
 * `externalEngineFixtures`' `stubFetch`, keeping the query string: the filters
 * and the cursor travel in it. An unrouted request answers 599.
 */
function stubFetch(table: Record<string, Handler>) {
  const calls: Recorded[] = [];
  const impl = async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input), "http://localhost");
    const method = (init?.method ?? "GET").toUpperCase();
    const rec = { method, path: url.pathname, query: Object.fromEntries(url.searchParams) };
    calls.push(rec);
    const h = table[`${method} ${url.pathname}`];
    if (!h) return new Response(JSON.stringify({ error: `unrouted ${method} ${url.pathname}` }), { status: 599 });
    const r = typeof h === "function" ? h(rec) : h;
    return new Response(JSON.stringify(r.body ?? {}), {
      status: r.status ?? 200,
      headers: { "content-type": "application/json" },
    });
  };
  vi.stubGlobal("fetch", vi.fn(impl));
  return calls;
}

function routes(engine: unknown, extra: Record<string, Handler> = {}): Record<string, Handler> {
  return {
    "GET /api/control-plane/engines/x1": { body: engine },
    "GET /api/control-plane/policies/services": { body: PROFILES },
    "GET /api/control-plane/catalogs": { body: CATALOGS },
    "GET /api/control-plane/engines/modelled-ranger-plugins": { body: MODELLED_PLUGINS },
    [`GET ${BASE}/status`]: { body: READY },
    [`GET ${BASE}`]: { body: { entries: [DELEGATED, DIRECT_FAILED], nextCursor: null } },
    ...extra,
  };
}

/** The browser's Back button, for the history FE-PAGE-2 asks the filters and the panel to write. */
function BackProbe() {
  const navigate = useNavigate();
  return (
    <button type="button" data-testid="history-back" onClick={() => navigate(-1)}>
      history back
    </button>
  );
}

function renderTab(me = ADMIN, search = "?tab=query-log") {
  return render(
    <Harness me={me} entry={`/control-plane/engines/x1${search}`}>
      <Routes>
        <Route
          path="/control-plane/engines/:id"
          element={
            <>
              <EngineDetail />
              <LocationProbe />
              <BackProbe />
            </>
          }
        />
      </Routes>
    </Harness>,
  );
}

const listCalls = (calls: Recorded[]) => calls.filter((c) => c.method === "GET" && c.path === BASE);
const statusCalls = (calls: Recorded[]) => calls.filter((c) => c.method === "GET" && c.path === `${BASE}/status`);
const urlParams = () => new URLSearchParams(screen.getByTestId("location").textContent!.split("?")[1] ?? "");

/** Wait for the list to show its rows. */
async function rowsShown() {
  return within(await screen.findByRole("table", {}, T)).getAllByRole("row").slice(1);
}

describe("the tab", () => {
  test("a StarRocks engine has a Query log tab after Monitor, and ?tab=query-log opens it", async () => {
    stubFetch(routes(SR));
    renderTab();
    const tab = await screen.findByRole("tab", { name: "Query log" }, T);
    expect(tab).toHaveAttribute("aria-selected", "true");
    const names = screen.getAllByRole("tab").map((t) => t.textContent);
    expect(names.indexOf("Query log")).toBe(names.indexOf("Monitor") + 1);
  });

  test("a Trino engine has none, and ?tab=query-log falls back to Overview without reading the log", async () => {
    const calls = stubFetch(routes(TRINO));
    renderTab();
    expect(await screen.findByRole("tab", { name: "Overview" }, T)).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("tab", { name: "Query log" })).not.toBeInTheDocument();
    await new Promise((r) => setTimeout(r, 50));
    expect(calls.filter((c) => c.path.startsWith(BASE))).toEqual([]);
  });
});

describe("the status line", () => {
  test("ready with an event: the retention and the latest event, with its UTC instant", async () => {
    stubFetch(routes(SR));
    renderTab();
    const line = await screen.findByTestId("query-log-status", {}, T);
    await waitFor(() => expect(line).toHaveTextContent(/^Kept 30 days\. Latest event: .+\.$/), T);
    expect(within(line).getByTitle("2026-11-11T08:00:00.000Z")).toBeInTheDocument();
  });

  test("ready with no event says when events arrive", async () => {
    stubFetch(routes(SR, { [`GET ${BASE}/status`]: { body: { state: "ready", lastEventAt: null, retentionDays: 30 } } }));
    renderTab();
    expect(await screen.findByTestId("query-log-status", {}, T)).toHaveTextContent(
      "No events yet. Events reach the log about a minute after a statement runs.",
    );
  });

  test("pluginNotShipped names Upgrade security when the engine needs it", async () => {
    stubFetch(
      routes(
        { ...SR, securityUpgradeNeeded: true, securityUpgradeReasons: ["unpatchedImage"] },
        { [`GET ${BASE}/status`]: { body: { state: "pluginNotShipped", retentionDays: 30 } } },
      ),
    );
    renderTab();
    const line = await screen.findByTestId("query-log-status", {}, T);
    expect(line).toHaveTextContent("This engine's image has no audit plugin.");
    expect(line).toHaveTextContent("Upgrade security");
    expect(line).toHaveTextContent("and the Query log starts after that.");
    // The button it names is B's, on the same page.
    expect(within(screen.getByTestId("starrocks-security-upgrade")).getByRole("button", { name: "Upgrade security" })).toBeInTheDocument();
  });

  test("pluginNotShipped without B's flag names no button", async () => {
    stubFetch(routes(SR, { [`GET ${BASE}/status`]: { body: { state: "pluginNotShipped", retentionDays: 30 } } }));
    renderTab();
    const line = await screen.findByTestId("query-log-status", {}, T);
    expect(line).toHaveTextContent("This engine's image has no audit plugin.");
    expect(line).not.toHaveTextContent("Upgrade security");
  });

  test("unreachable shows the message, and Retry reads the status again", async () => {
    const calls = stubFetch(
      routes(SR, { [`GET ${BASE}/status`]: { body: { state: "unreachable", message: "FE fe-0 refused the connection", retentionDays: 30 } } }),
    );
    renderTab();
    const line = await screen.findByTestId("query-log-status", {}, T);
    expect(line).toHaveTextContent("FE fe-0 refused the connection");
    expect(statusCalls(calls)).toHaveLength(1);
    await userEvent.click(within(line).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(statusCalls(calls)).toHaveLength(2), T);
  });
});

describe("Set up query log", () => {
  const NOT_SET_UP = { [`GET ${BASE}/status`]: { body: { state: "notSetUp", retentionDays: 30 } } };

  test("with engine:update it is enabled; it calls the route, reads the status and the list again, and toasts", async () => {
    const calls = stubFetch(routes(SR, { ...NOT_SET_UP, [`POST ${BASE}/setup`]: { body: { state: "ready", retentionDays: 30 } } }));
    renderTab(userWith("engine:read", "engine:update"));
    const button = await screen.findByRole("button", { name: "Set up query log" }, T);
    expect(button).toBeEnabled();
    await rowsShown();
    const listBefore = listCalls(calls).length;
    await userEvent.click(button);
    await waitFor(() => expect(calls.filter((c) => c.method === "POST" && c.path === `${BASE}/setup`)).toHaveLength(1), T);
    await waitFor(() => expect(statusCalls(calls)).toHaveLength(2), T);
    await waitFor(() => expect(listCalls(calls).length).toBeGreaterThan(listBefore), T);
    expect(toast.success).toHaveBeenCalledWith("Query log set up.");
  });

  test("a failed setup shows the server's message", async () => {
    stubFetch(routes(SR, { ...NOT_SET_UP, [`POST ${BASE}/setup`]: { status: 502, body: { error: "INSTALL PLUGIN failed: no such file" } } }));
    renderTab(userWith("engine:read", "engine:update"));
    await userEvent.click(await screen.findByRole("button", { name: "Set up query log" }, T));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("INSTALL PLUGIN failed: no such file"), T);
  });

  test("without engine:update it is disabled and says why", async () => {
    const calls = stubFetch(routes(SR, NOT_SET_UP));
    renderTab(userWith("engine:read"));
    const button = await screen.findByRole("button", { name: "Set up query log" }, T);
    expect(button).toBeDisabled();
    expect(screen.getByTestId("query-log-status")).toHaveTextContent("Setting it up needs the engine:update permission.");
    expect(calls.filter((c) => c.method === "POST")).toEqual([]);
  });
});

describe("the list's states", () => {
  test("an error shows the server's message, not an empty list", async () => {
    stubFetch(routes(SR, { [`GET ${BASE}`]: { status: 502, body: { error: "FE fe-0 rejected the read" } } }));
    renderTab();
    expect(await screen.findByText("FE fe-0 rejected the read", {}, T)).toBeInTheDocument();
    expect(screen.queryByText(/No statements/)).not.toBeInTheDocument();
  });

  test("a table that does not exist yet (409 query_log_not_ready) says so", async () => {
    stubFetch(routes(SR, { [`GET ${BASE}`]: { status: 409, body: { error: "not ready", code: "query_log_not_ready" } } }));
    renderTab();
    expect(await screen.findByText("The query log table does not exist yet", {}, T)).toBeInTheDocument();
  });

  test("loading is a skeleton", async () => {
    stubFetch(routes(SR));
    const f = vi.mocked(globalThis.fetch);
    const real = f.getMockImplementation()!;
    f.mockImplementation((input, init) =>
      new URL(String(input), "http://localhost").pathname === BASE ? new Promise<Response>(() => {}) : real(input, init),
    );
    const { container } = renderTab();
    await screen.findByTestId("query-log-status", {}, T);
    expect(container.querySelector('[data-slot="skeleton"], .animate-pulse')).not.toBeNull();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  test("empty, unfiltered: EmptyState", async () => {
    stubFetch(routes(SR, { [`GET ${BASE}`]: { body: { entries: [], nextCursor: null } } }));
    renderTab();
    expect(await screen.findByText("No statements in the last 24 hours", {}, T)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Clear filters" })).not.toBeInTheDocument();
  });

  test("a filter that matches nothing says so, and Clear filters empties the URL's filters", async () => {
    stubFetch(routes(SR, { [`GET ${BASE}`]: { body: { entries: [], nextCursor: null } } }));
    renderTab(ADMIN, "?tab=query-log&user=nobody&state=error");
    expect(await screen.findByText("No statements match these filters", {}, T)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(urlParams().has("user")).toBe(false), T);
    expect(urlParams().has("state")).toBe(false);
    expect(urlParams().get("tab")).toBe("query-log");
  });
});

describe("filters", () => {
  test("State writes the URL and reaches the request", async () => {
    const calls = stubFetch(routes(SR));
    renderTab();
    await rowsShown();
    expect(listCalls(calls).at(-1)!.query.state).toBeUndefined();
    await userEvent.click(screen.getByRole("combobox", { name: "State" }));
    await userEvent.click(await screen.findByRole("option", { name: "Failed" }));
    await waitFor(() => expect(urlParams().get("state")).toBe("error"), T);
    await waitFor(() => expect(listCalls(calls).at(-1)!.query.state).toBe("error"), T);
  });

  test("User writes the URL on Enter and reaches the request", async () => {
    const calls = stubFetch(routes(SR));
    renderTab();
    await rowsShown();
    await userEvent.type(screen.getByLabelText("User"), "bob{Enter}");
    await waitFor(() => expect(urlParams().get("user")).toBe("bob"), T);
    await waitFor(() => expect(listCalls(calls).at(-1)!.query.user).toBe("bob"), T);
  });

  test("the range writes the URL, and the request asks for that window", async () => {
    const calls = stubFetch(routes(SR));
    renderTab();
    await rowsShown();
    const first = listCalls(calls).at(-1)!.query;
    expect(Date.parse(first.to) - Date.parse(first.from)).toBe(24 * 3600 * 1000);
    await userEvent.click(screen.getByRole("combobox", { name: "Time range" }));
    await userEvent.click(await screen.findByRole("option", { name: "Last hour" }));
    await waitFor(() => expect(urlParams().get("from")).toBe("1h"), T);
    await waitFor(() => {
      const q = listCalls(calls).at(-1)!.query;
      expect(Date.parse(q.to) - Date.parse(q.from)).toBe(3600 * 1000);
    }, T);
  });

  test("a custom range in the URL reaches the request as written", async () => {
    const calls = stubFetch(routes(SR));
    renderTab(ADMIN, "?tab=query-log&from=2026-11-01T00:00:00.000Z&to=2026-11-02T00:00:00.000Z");
    await rowsShown();
    expect(screen.getByRole("combobox", { name: "Time range" })).toHaveTextContent("Custom");
    expect(screen.getByLabelText("From")).toBeInTheDocument();
    expect(listCalls(calls).at(-1)!.query).toMatchObject({ from: "2026-11-01T00:00:00.000Z", to: "2026-11-02T00:00:00.000Z" });
  });
});

describe("the table", () => {
  test("Load more follows nextCursor and appends the next page", async () => {
    const calls = stubFetch(
      routes(SR, {
        [`GET ${BASE}`]: (req) =>
          req.query.cursor === "c2"
            ? { body: { entries: [DIRECT_FAILED], nextCursor: null } }
            : { body: { entries: [DELEGATED], nextCursor: "c2" } },
      }),
    );
    renderTab();
    expect(await rowsShown()).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(async () => expect(await rowsShown()).toHaveLength(2), T);
    const [first, second] = listCalls(calls);
    expect(first.query.cursor).toBeUndefined();
    expect(second.query.cursor).toBe("c2");
    expect(second.query.from).toBe(first.query.from);
    expect(second.query.to).toBe(first.query.to);
    expect(screen.queryByRole("button", { name: "Load more" })).not.toBeInTheDocument();
  });

  test("a filter change after Load more starts again at the first page, with no cursor", async () => {
    const calls = stubFetch(
      routes(SR, {
        [`GET ${BASE}`]: (req) =>
          req.query.cursor === "c2"
            ? { body: { entries: [DIRECT_FAILED], nextCursor: null } }
            : { body: { entries: [DELEGATED], nextCursor: "c2" } },
      }),
    );
    renderTab();
    await rowsShown();
    await userEvent.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(async () => expect(await rowsShown()).toHaveLength(2), T);
    const before = listCalls(calls).length;

    await userEvent.click(screen.getByRole("combobox", { name: "State" }));
    await userEvent.click(await screen.findByRole("option", { name: "Failed" }));

    await waitFor(() => expect(listCalls(calls).length).toBe(before + 1), T);
    const next = listCalls(calls).at(-1)!.query;
    expect(next.state).toBe("error");
    expect(next.cursor).toBeUndefined();
    await waitFor(async () => expect(await rowsShown()).toHaveLength(1), T);
    expect(screen.getByRole("button", { name: "Load more" })).toBeInTheDocument();
  });

  test("a row's open button names what it opens", async () => {
    stubFetch(routes(SR));
    renderTab();
    const [delegated] = await rowsShown();
    const button = within(delegated).getByRole("button");
    expect(button).toHaveAccessibleName(/^Open statement at \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} by bob$/);
    expect(button).toHaveAttribute("aria-haspopup", "dialog");
  });

  test("User is the person; Connected as only when the session user differs", async () => {
    stubFetch(routes(SR));
    renderTab();
    const [delegated, direct] = await rowsShown();
    const cells = (row: HTMLElement) => within(row).getAllByRole("cell").map((c) => c.textContent);
    expect(cells(delegated).slice(1, 3)).toEqual(["bob", "egret_svc"]);
    expect(cells(direct).slice(1, 3)).toEqual(["alice", ""]);
  });

  test("a row shows its state, duration, rows and the statement's first line cut with an ellipsis", async () => {
    stubFetch(routes(SR));
    renderTab();
    const [delegated, direct] = await rowsShown();
    expect(within(delegated).getByText("Succeeded")).toBeInTheDocument();
    expect(within(direct).getByText("Failed")).toBeInTheDocument();
    expect(delegated).toHaveTextContent("1.5 s");
    expect(delegated).toHaveTextContent("1,234");
    expect(within(delegated).getByText("SELECT * …")).toBeInTheDocument();
    expect(within(delegated).getByTitle("2026-11-11T08:00:00.000Z")).toBeInTheDocument();
  });

  test("without engine:audit the statement cell reads the hidden text", async () => {
    stubFetch(routes(SR, { [`GET ${BASE}`]: { body: { entries: [DIRECT_FAILED_WITHHELD], nextCursor: null } } }));
    renderTab(userWith("engine:read"));
    const [row] = await rowsShown();
    expect(row).toHaveTextContent(HIDDEN);
    // The state still tells a failure.
    expect(within(row).getByText("Failed")).toBeInTheDocument();
  });
});

describe("the side panel", () => {
  test("a row opens the whole statement and the error from the detail route, kept in the URL", async () => {
    const calls = stubFetch(
      routes(SR, {
        [`GET ${BASE}/${DIRECT_FAILED.queryId}`]: {
          body: { ...DIRECT_FAILED, querySource: "MySQL", clientIp: "10.0.0.7", statement: "SELEC xyz\nFROM t" },
        },
      }),
    );
    renderTab();
    const [, direct] = await rowsShown();
    await userEvent.click(within(direct).getByRole("button"));
    const panel = await screen.findByRole("dialog", {}, T);
    expect(await within(panel).findByTestId("query-log-statement", {}, T)).toHaveTextContent("SELEC xyz FROM t");
    expect(within(panel).getByTestId("query-log-error")).toHaveTextContent("Unexpected input 'xyz'");
    expect(panel).toHaveTextContent("10.0.0.7");
    expect(panel).toHaveTextContent("MySQL");
    expect(urlParams().get("entry")).toBe(DIRECT_FAILED.queryId);
    expect(calls.some((c) => c.path === `${BASE}/${DIRECT_FAILED.queryId}`)).toBe(true);
  });

  test("without engine:audit the statement and the error both read the hidden text, not 'no error'", async () => {
    stubFetch(
      routes(SR, {
        [`GET ${BASE}`]: { body: { entries: [DIRECT_FAILED_WITHHELD], nextCursor: null } },
        [`GET ${BASE}/${DIRECT_FAILED.queryId}`]: { body: DIRECT_FAILED_WITHHELD },
      }),
    );
    renderTab(userWith("engine:read"), `?tab=query-log&entry=${DIRECT_FAILED.queryId}`);
    const panel = await screen.findByRole("dialog", {}, T);
    await waitFor(() => expect(within(panel).getAllByText(HIDDEN)).toHaveLength(2), T);
    expect(within(panel).queryByText("None")).not.toBeInTheDocument();
  });

  test("a success seen with engine:audit has no error", async () => {
    stubFetch(routes(SR, { [`GET ${BASE}/${DELEGATED.queryId}`]: { body: DELEGATED } }));
    renderTab(ADMIN, `?tab=query-log&entry=${DELEGATED.queryId}`);
    const panel = await screen.findByRole("dialog", {}, T);
    const error = await within(panel).findByRole("region", { name: "Error" }, T);
    expect(error).toHaveTextContent("None");
  });
});

describe("URL values the server would refuse", () => {
  test("a bad from, to, state and an over-long user fall back to the defaults", async () => {
    const calls = stubFetch(routes(SR, { [`GET ${BASE}`]: { body: { entries: [], nextCursor: null } } }));
    renderTab(ADMIN, `?tab=query-log&from=garbage&to=nope&state=bogus&user=${"u".repeat(129)}`);
    // Nothing the reader can see narrows the view, so the empty state is the unfiltered one.
    expect(await screen.findByText("No statements in the last 24 hours", {}, T)).toBeInTheDocument();
    const q = listCalls(calls).at(-1)!.query;
    expect(Date.parse(q.to) - Date.parse(q.from)).toBe(24 * 3600 * 1000);
    expect(q.state).toBeUndefined();
    expect(q.user).toBeUndefined();
    expect(screen.getByRole("combobox", { name: "Time range" })).toHaveTextContent("Last 24 hours");
    expect(screen.getByRole("combobox", { name: "State" })).toHaveTextContent("All");
    expect(screen.getByLabelText("User")).toHaveValue("");
  });

  test("a preset from ignores a to beside it", async () => {
    const calls = stubFetch(routes(SR));
    renderTab(ADMIN, "?tab=query-log&from=1h&to=2026-11-02T00:00:00.000Z");
    await rowsShown();
    const q = listCalls(calls).at(-1)!.query;
    expect(Date.parse(q.to) - Date.parse(q.from)).toBe(3600 * 1000);
    expect(q.to).not.toBe("2026-11-02T00:00:00.000Z");
  });
});

describe("history (FE-PAGE-2)", () => {
  test("Back undoes a filter change", async () => {
    stubFetch(routes(SR));
    renderTab();
    await rowsShown();
    await userEvent.click(screen.getByRole("combobox", { name: "State" }));
    await userEvent.click(await screen.findByRole("option", { name: "Failed" }));
    await waitFor(() => expect(urlParams().get("state")).toBe("error"), T);
    await userEvent.click(screen.getByTestId("history-back"));
    await waitFor(() => expect(urlParams().has("state")).toBe(false), T);
    expect(urlParams().get("tab")).toBe("query-log");
    expect(screen.getByRole("combobox", { name: "State" })).toHaveTextContent("All");
  });

  test("Back puts the typed User back to what the URL held", async () => {
    stubFetch(routes(SR));
    renderTab();
    await rowsShown();
    await userEvent.type(screen.getByLabelText("User"), "bob{Enter}");
    await waitFor(() => expect(urlParams().get("user")).toBe("bob"), T);
    await userEvent.click(screen.getByTestId("history-back"));
    await waitFor(() => expect(screen.getByLabelText("User")).toHaveValue(""), T);
  });

  test("Back closes an opened statement", async () => {
    stubFetch(routes(SR, { [`GET ${BASE}/${DELEGATED.queryId}`]: { body: DELEGATED } }));
    renderTab();
    const [delegated] = await rowsShown();
    await userEvent.click(within(delegated).getByRole("button"));
    expect(await screen.findByRole("dialog", {}, T)).toBeInTheDocument();
    // The panel is modal; the probe sits under it, so it is fired directly.
    screen.getByTestId("history-back").click();
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument(), T);
    expect(urlParams().has("entry")).toBe(false);
  });
});

describe("partial text and the panel's facts", () => {
  /** An audit holder's row whose `stmt` column was NULL: the error is delivered, the statement is not. */
  const ERROR_ONLY = { ...DIRECT_FAILED_WITHHELD, errorMessage: "Unexpected input 'xyz'" };

  test("a delivered error is shown even when the statement is absent, and nothing reads Hidden", async () => {
    stubFetch(
      routes(SR, {
        [`GET ${BASE}`]: { body: { entries: [ERROR_ONLY], nextCursor: null } },
        [`GET ${BASE}/${DIRECT_FAILED.queryId}`]: { body: ERROR_ONLY },
      }),
    );
    renderTab(ADMIN, `?tab=query-log&entry=${DIRECT_FAILED.queryId}`);
    const panel = await screen.findByRole("dialog", {}, T);
    expect(await within(panel).findByTestId("query-log-error", {}, T)).toHaveTextContent("Unexpected input 'xyz'");
    expect(within(panel).getByRole("region", { name: "Statement" })).toHaveTextContent("Not recorded");
    expect(within(panel).queryByText(HIDDEN)).not.toBeInTheDocument();
    expect(screen.queryByText(HIDDEN)).not.toBeInTheDocument();
  });

  test("Connected as appears in the panel only when it differs", async () => {
    stubFetch(
      routes(SR, {
        [`GET ${BASE}/${DIRECT_FAILED.queryId}`]: { body: DIRECT_FAILED },
        [`GET ${BASE}/${DELEGATED.queryId}`]: { body: DELEGATED },
      }),
    );
    renderTab(ADMIN, `?tab=query-log&entry=${DIRECT_FAILED.queryId}`);
    let panel = await screen.findByRole("dialog", {}, T);
    await within(panel).findByTestId("query-log-statement", {}, T);
    expect(within(panel).queryByText("Connected as")).not.toBeInTheDocument();
    cleanup();

    renderTab(ADMIN, `?tab=query-log&entry=${DELEGATED.queryId}`);
    panel = await screen.findByRole("dialog", {}, T);
    await within(panel).findByTestId("query-log-statement", {}, T);
    expect(within(panel).getByText("Connected as").nextElementSibling).toHaveTextContent("egret_svc");
  });

  test("a state this client does not know gets a neutral line, not 'did not answer'", async () => {
    stubFetch(routes(SR, { [`GET ${BASE}/status`]: { body: { state: "somethingNew", retentionDays: 30 } } }));
    renderTab();
    const line = await screen.findByTestId("query-log-status", {}, T);
    expect(line).toHaveTextContent("The query log reports a state this page does not know (somethingNew).");
    expect(line).not.toHaveTextContent("did not answer");
  });
});
