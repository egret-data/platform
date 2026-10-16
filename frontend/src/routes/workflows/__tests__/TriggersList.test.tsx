import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type {
  BulkResultDto,
  FlowListItemDto,
  SetTriggersDisabledRequest,
  TriggerEntryDto,
  TriggerRowDto,
  WorkflowNamespaceDto,
} from "@/api/generated/model";
import { TooltipProvider } from "@/components/ui/tooltip";
import { namespaceContains } from "@/components/workflows/namespaceTree";
import { otherEntry, scheduleEntry, webhookEntry } from "@/components/workflows/triggers/__tests__/triggerFixtures";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import TriggersList from "@/routes/workflows/TriggersList";

const WS = "eng";
const BASE = `/api/ws/${WS}/workflows`;
const LIST = `${BASE}/triggers`;
const BULK = `${BASE}/triggers/disabled`;
const FLOW_A = "aaaaaaaa-0000-4000-8000-00000000000a";
const FLOW_B = "bbbbbbbb-0000-4000-8000-00000000000b";
const OVERDUE = new Date(Date.now() - 10 * 60_000).toISOString();

type Reply = { status: number; body: unknown };
type Req = { method: string; path: string; query: URLSearchParams; body: unknown };

const FLOWS: Record<string, { namespace: string; flow_id: string; source: string }> = {
  [FLOW_A]: {
    namespace: "company.team",
    flow_id: "hello",
    source: `id: hello
namespace: company.team
tasks:
  - id: t
    type: io.kestra.plugin.core.log.Log
    message: hi
triggers:
  - id: daily
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "*/5 * * * *"
  - id: hook
    type: io.kestra.plugin.core.trigger.Webhook
    key: s3cr3t-key
`,
  },
  [FLOW_B]: {
    namespace: "company.data",
    flow_id: "load",
    source: `id: load
namespace: company.data
tasks:
  - id: t
    type: io.kestra.plugin.core.log.Log
    message: hi
triggers:
  - id: daily
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "0 * * * *"
  - id: poll
    type: io.kestra.plugin.fs.http.Trigger
    uri: https://example.com
`,
  },
};

function row(flow: string, entry: TriggerEntryDto, over: Partial<TriggerRowDto> = {}): TriggerRowDto {
  const f = FLOWS[flow];
  return { ...entry, flow_uuid: flow, namespace: f.namespace, flow_id: f.flow_id, flow_disabled: false, ...over };
}

/**
 * Two flows that share a trigger id (`daily`), so a page keyed by trigger id
 * alone would select, switch or open the wrong one.
 */
const ROWS = (): TriggerRowDto[] => [
  row(FLOW_A, scheduleEntry()),
  row(FLOW_A, webhookEntry()),
  row(FLOW_B, scheduleEntry()),
  row(FLOW_B, otherEntry()),
];

/**
 * A row as the list route answers it (W7e spec §3.5): the per-flow entry
 * without `webhook` and without `depends_on.conditions`. The server holds the
 * full entries; only the per-flow route (the drawer's read) returns them.
 */
function listRow(r: TriggerRowDto): TriggerRowDto {
  const flow = r.flow?.depends_on ? { ...r.flow, depends_on: { ...r.flow.depends_on, conditions: undefined } } : r.flow;
  return { ...r, webhook: null, flow };
}

function flowItem(id: string): FlowListItemDto {
  const f = FLOWS[id];
  return { id, namespace: f.namespace, flow_id: f.flow_id, state: "live", created_at: "2026-10-01T00:00:00Z", updated_at: "2026-10-02T00:00:00Z" };
}
const nsRow = (namespace: string, own = true): WorkflowNamespaceDto => ({ namespace, flows: own ? 1 : 0, files: 0, kv_keys: 0, own });

/**
 * The routes the page calls, served from memory through a stubbed `fetch`, so
 * the page runs its real orval hooks, mutator and `apiFetch` (FE-TEST-1).
 * `GET …/triggers` filters as the server does (W7e spec §3.5) and cuts the page.
 */
class Server {
  rows: TriggerRowDto[] = ROWS();
  unparsable = 0;
  flags = { scheduler_enabled: true, worker_enabled: true, webhooks_enabled: true };
  /** The page's `can_manage`: the caller's `workflow:manage`. */
  canManage = true;
  listReply: Reply | null = null;
  listGate: Promise<void> | null = null;
  /** Answers `GET …/flows/{id}/triggers` with this instead. */
  flowTriggersReply: Reply | null = null;
  /** While set, `GET …/flows/{id}/triggers` waits for it. */
  flowTriggersGate: Promise<void> | null = null;
  bulkRefusals: Record<string, { code: string; message: string }> = {};
  requests: Req[] = [];

  lists = () => this.requests.filter((r) => r.method === "GET" && r.path === LIST).map((r) => r.query);
  bulks = () => this.requests.filter((r) => r.method === "PUT" && r.path === BULK).map((r) => r.body as SetTriggersDisabledRequest);

  install() {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string, init?: RequestInit) => {
        const method = (init?.method ?? "GET").toUpperCase();
        const url = new URL(String(input), "http://x");
        const path = String(input).split("?")[0];
        const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
        this.requests.push({ method, path, query: url.searchParams, body });
        if (method === "GET" && path === LIST && this.listGate) await this.listGate;
        if (method === "GET" && /\/flows\/[^/]+\/triggers$/.test(path) && this.flowTriggersGate) await this.flowTriggersGate;
        const reply = this.route(method, path, url.searchParams, body);
        return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
      }),
    );
  }

  private route(method: string, path: string, q: URLSearchParams, body: unknown): Reply {
    if (method === "GET" && path === LIST) {
      if (this.listReply) return this.listReply;
      const prefix = q.get("namespace_prefix");
      const flow = q.get("flow_id");
      const kinds = (q.get("kind") ?? "").split(",").filter(Boolean);
      const state = q.get("state");
      const needle = (q.get("q") ?? "").toLowerCase();
      const matched = this.rows.filter(
        (r) =>
          (!prefix || namespaceContains(prefix, r.namespace)) &&
          (!flow || r.flow_uuid === flow) &&
          (kinds.length === 0 || kinds.includes(r.kind)) &&
          (!state || (state === "disabled") === r.effective_disabled) &&
          (!needle || [r.trigger_id, r.flow_id, r.namespace].some((s) => s.toLowerCase().includes(needle))),
      );
      const limit = Number(q.get("limit") ?? 50);
      const offset = Number(q.get("offset") ?? 0);
      return {
        status: 200,
        body: {
          rows: matched.slice(offset, offset + limit).map(listRow),
          total: matched.length,
          limit,
          offset,
          unparsable: this.unparsable,
          ...this.flags,
          can_manage: this.canManage,
        },
      };
    }
    if (method === "PUT" && path === BULK) {
      const req = body as SetTriggersDisabledRequest;
      const results: BulkResultDto[] = req.triggers.map(({ flow_id, trigger_id }) => {
        const refusal = this.bulkRefusals[`${flow_id}/${trigger_id}`];
        if (refusal) return { flow_id, trigger_id, ok: false, code: refusal.code, message: refusal.message, trigger: null };
        const at = this.rows.findIndex((r) => r.flow_uuid === flow_id && r.trigger_id === trigger_id);
        if (at < 0) return { flow_id, trigger_id, ok: false, code: "not_found", message: "not found: trigger", trigger: null };
        const next = { ...this.rows[at], runtime_disabled: req.disabled, effective_disabled: req.disabled, disabled_by_name: req.disabled ? "Ana Lima" : null };
        this.rows = this.rows.map((r, i) => (i === at ? next : r));
        const { flow_uuid: _u, namespace: _n, flow_id: _f, flow_disabled: _d, ...entry } = next;
        return { flow_id, trigger_id, ok: true, code: null, message: null, trigger: entry };
      });
      return { status: 200, body: { results } };
    }
    if (method === "GET" && path === `${BASE}/namespaces`) {
      return { status: 200, body: [nsRow("company", false), nsRow("company.data"), nsRow("company.team")] };
    }
    if (method === "GET" && path === `${BASE}/flows`) return { status: 200, body: [flowItem(FLOW_B), flowItem(FLOW_A)] };
    const flowTriggers = new RegExp(`^${BASE}/flows/([^/]+)/triggers$`).exec(path);
    if (method === "GET" && flowTriggers) {
      if (this.flowTriggersReply) return this.flowTriggersReply;
      const id = flowTriggers[1];
      const triggers = this.rows
        .filter((r) => r.flow_uuid === id)
        .map(({ flow_uuid: _u, namespace: _n, flow_id: _f, flow_disabled: _d, ...entry }) => entry);
      return { status: 200, body: { flow_disabled: false, flow_deleted: false, ...this.flags, triggers } };
    }
    const flowPath = new RegExp(`^${BASE}/flows/([^/]+)$`).exec(path);
    if (method === "GET" && flowPath && FLOWS[flowPath[1]]) {
      const id = flowPath[1];
      const f = FLOWS[id];
      return {
        status: 200,
        body: {
          id,
          folder_id: null,
          namespace: f.namespace,
          flow_id: f.flow_id,
          source: f.source,
          plan: {},
          state: "live",
          live_version_id: null,
          created_by: null,
          created_by_name: null,
          latest_version: null,
          created_at: "2026-10-01T00:00:00.000001Z",
          updated_at: "2026-10-10T09:47:08.618779Z",
          validation: { errors: [], warnings: [] },
          can_manage: true,
        },
      };
    }
    return { status: 404, body: { error: `no route ${method} ${path}` } };
  }
}

function Where() {
  const l = useLocation();
  return <output data-testid="where">{`${l.pathname}${l.search}`}</output>;
}

let server: Server;
beforeEach(() => {
  for (const f of Object.values(toast)) f.mockReset();
  server = new Server();
});
afterEach(() => vi.unstubAllGlobals());

function renderPage(search = "") {
  server.install();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[`/explore/${WS}/workflows/triggers${search}`]}>
          <Routes>
            <Route path="/explore/:workspace/workflows/triggers" element={<TriggersList />} />
            <Route path="*" element={null} />
          </Routes>
          <Where />
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
  return { client };
}

const where = () => screen.getByTestId("where").textContent ?? "";
const urlParams = () => new URLSearchParams(where().split("?")[1] ?? "");
const table = () => screen.findByTestId("triggers-table");
const bodyRows = () => within(screen.getByTestId("triggers-table")).getAllByRole("row").slice(1);
/** A row by its flow's YAML id and the trigger's id: the global labels name both. */
const who = (flow: string, trigger: string) => `${trigger} of ${FLOWS[flow].namespace}.${FLOWS[flow].flow_id}`;
const lastList = () => server.lists().at(-1)!;
const TOO_LARGE: Reply = {
  status: 422,
  body: { error: "more than 2000 flows match; narrow by namespace", code: "triggers_too_large" },
};

describe("TriggersList: the page", () => {
  it("is a list page with the flow tab's columns plus one Flow column, namespace and flow linked", async () => {
    renderPage();
    expect(screen.getByRole("heading", { level: 1, name: "Triggers" })).toBeInTheDocument();
    await table();
    expect(screen.getAllByRole("columnheader").map((h) => h.textContent?.trim())).toEqual([
      "",
      "ID",
      "Flow",
      "Type",
      "Next run",
      "Last evaluated",
      "Last execution",
      "Status",
      "Enabled",
      "Backfill",
      "Details",
    ]);
    const second = bodyRows()[2];
    expect(within(second).getByRole("link", { name: "company.data" })).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/namespaces/company.data/overview`,
    );
    expect(within(second).getByRole("link", { name: "load" })).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/flows/${FLOW_B}/triggers`,
    );
    // A list row carries no webhook (§3.5), and the page never shows a key.
    expect(document.body.textContent).not.toContain("s3cr3t-key");
    // Type: the short name only, with no long name on hover.
    const typeCell = within(second).getAllByRole("cell")[3];
    expect(typeCell.textContent).toBe("Schedule: Schedule");
    expect(typeCell.querySelector("[title]")).toBeNull();
  });

  it("asks for the first page of everything when nothing is filtered", async () => {
    renderPage();
    await table();
    const q = server.lists()[0];
    expect([...q.keys()].sort()).toEqual(["limit", "offset"]);
    expect(q.get("limit")).toBe("50");
    expect(q.get("offset")).toBe("0");
  });

  it("reads every filter back from a shared URL and sends it", async () => {
    renderPage(`?namespace=company&flow=${FLOW_B}&kind=schedule,webhook&state=enabled&q=dai&page=2`);
    await waitFor(() => expect(server.lists().length).toBeGreaterThan(0));
    const q = server.lists()[0];
    expect(q.get("namespace_prefix")).toBe("company");
    expect(q.get("flow_id")).toBe(FLOW_B);
    expect(q.get("kind")).toBe("schedule,webhook");
    expect(q.get("state")).toBe("enabled");
    expect(q.get("q")).toBe("dai");
    expect(q.get("limit")).toBe("50");
    expect(q.get("offset")).toBe("50");
    // The pickers show the names once the namespaces and flows are read.
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Namespace" })).toHaveTextContent("company"));
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Flow" })).toHaveTextContent("company.data.load"));
    expect(screen.getByRole("combobox", { name: "State" })).toHaveTextContent("Enabled");
    expect(screen.getByRole("button", { name: /^Kind/ })).toHaveTextContent("Schedule, Webhook");
    expect(screen.getByLabelText("Search triggers")).toHaveValue("dai");
  });

  it("drops an unknown state from the URL rather than sending it", async () => {
    renderPage("?state=paused&kind=schedule,bogus");
    await table();
    expect(server.lists()[0].get("state")).toBeNull();
    expect(server.lists()[0].get("kind")).toBe("schedule");
  });
});

describe("TriggersList: the filters", () => {
  it("writes a picked namespace into the URL and the request, and goes back to the first page", async () => {
    const user = userEvent.setup();
    renderPage(`?flow=${FLOW_B}&page=2`);
    await waitFor(() => expect(server.lists().length).toBeGreaterThan(0));
    await user.click(screen.getByRole("combobox", { name: "Namespace" }));
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options).toEqual(["All", "company", "company.data", "company.team"]);
    await user.click(screen.getByRole("option", { name: "company.team" }));
    await waitFor(() => expect(lastList().get("namespace_prefix")).toBe("company.team"));
    expect(lastList().get("offset")).toBe("0");
    expect(urlParams().get("namespace")).toBe("company.team");
    expect(urlParams().get("page")).toBeNull();
    // A flow outside the new namespace would match nothing, so it goes with the change.
    expect(urlParams().get("flow")).toBeNull();
    expect(lastList().get("flow_id")).toBeNull();
  });

  it("offers the flows inside the picked namespace, and sends the flow's uuid", async () => {
    const user = userEvent.setup();
    renderPage("?namespace=company.data&page=3");
    await waitFor(() => expect(server.lists().length).toBeGreaterThan(0));
    await user.click(screen.getByRole("combobox", { name: "Flow" }));
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options).toEqual(["All", "company.data.load"]);
    await user.click(screen.getByRole("option", { name: "company.data.load" }));
    await waitFor(() => expect(lastList().get("flow_id")).toBe(FLOW_B));
    expect(urlParams().get("flow")).toBe(FLOW_B);
    expect(urlParams().get("page")).toBeNull();
  });

  it("sends picked kinds as one comma list, and resets the page", async () => {
    const user = userEvent.setup();
    renderPage("?page=2");
    await waitFor(() => expect(server.lists().length).toBeGreaterThan(0));
    await user.click(screen.getByRole("button", { name: /^Kind/ }));
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Webhook" }));
    await waitFor(() => expect(urlParams().get("kind")).toBe("webhook"));
    expect(urlParams().get("page")).toBeNull();
    await user.click(screen.getByRole("menuitemcheckbox", { name: "Schedule" }));
    await waitFor(() => expect(urlParams().get("kind")).toBe("webhook,schedule"));
    await waitFor(() => expect(lastList().get("kind")).toBe("webhook,schedule"));
  });

  it("sends the Enabled / Disabled pick as state", async () => {
    const user = userEvent.setup();
    server.rows = [row(FLOW_A, scheduleEntry({ runtime_disabled: true, effective_disabled: true })), row(FLOW_A, webhookEntry())];
    renderPage("?page=2");
    await waitFor(() => expect(server.lists().length).toBeGreaterThan(0));
    await user.click(screen.getByRole("combobox", { name: "State" }));
    await user.click(await screen.findByRole("option", { name: "Disabled" }));
    await waitFor(() => expect(lastList().get("state")).toBe("disabled"));
    expect(urlParams().get("state")).toBe("disabled");
    expect(urlParams().get("page")).toBeNull();
    await waitFor(() => expect(bodyRows()).toHaveLength(1));
    expect(within(bodyRows()[0]).getByRole("link", { name: "daily" })).toBeInTheDocument();
  });

  it("searches as typed, through the URL, and resets the page", async () => {
    const user = userEvent.setup();
    renderPage("?page=2");
    await waitFor(() => expect(server.lists().length).toBeGreaterThan(0));
    await user.type(screen.getByLabelText("Search triggers"), "hoo");
    await waitFor(() => expect(lastList().get("q")).toBe("hoo"));
    expect(urlParams().get("q")).toBe("hoo");
    expect(urlParams().get("page")).toBeNull();
    await waitFor(() => expect(bodyRows()).toHaveLength(1));
  });

  it("asks once per pause in the typing, not once per key", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      // 50 ms between keys: well inside the pause, so the three keys are one search.
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime, delay: 50 });
      renderPage();
      await table();
      const before = server.lists().length;
      const box = screen.getByLabelText("Search triggers");
      await user.type(box, "ho");
      await user.type(box, "o");
      // Typed, but the pause has not come: nothing asked yet.
      expect(server.lists().filter((q) => q.get("q") !== null)).toHaveLength(0);
      await vi.advanceTimersByTimeAsync(300);
      await waitFor(() => expect(server.lists().length).toBe(before + 1));
      expect(lastList().get("q")).toBe("hoo");
      await vi.advanceTimersByTimeAsync(1000);
      expect(server.lists().filter((q) => q.get("q") !== null).map((q) => q.get("q"))).toEqual(["hoo"]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not let a search still waiting to be written undo Clear filters", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      renderPage("?q=nomatch");
      expect(await screen.findByText("No triggers match these filters")).toBeInTheDocument();
      await user.type(screen.getByLabelText("Search triggers"), "x");
      // Clear within the pause: the pending write of "nomatchx" must not land afterwards.
      await user.click(screen.getByRole("button", { name: "Clear filters" }));
      await vi.advanceTimersByTimeAsync(1000);
      await waitFor(() => expect(bodyRows()).toHaveLength(4));
      expect(where()).toBe(`/explore/${WS}/workflows/triggers`);
      expect(screen.getByLabelText("Search triggers")).toHaveValue("");
      expect(server.lists().some((q) => q.get("q") === "nomatchx")).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not let a first search, not yet in the URL, come back after Clear filters", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      // No `q` in the URL before or after the clear, so only Clear itself can drop the pending write.
      renderPage("?kind=flow");
      expect(await screen.findByText("No triggers match these filters")).toBeInTheDocument();
      await user.type(screen.getByLabelText("Search triggers"), "x");
      await user.click(screen.getByRole("button", { name: "Clear filters" }));
      await vi.advanceTimersByTimeAsync(1000);
      await waitFor(() => expect(bodyRows()).toHaveLength(4));
      expect(where()).toBe(`/explore/${WS}/workflows/triggers`);
      expect(screen.getByLabelText("Search triggers")).toHaveValue("");
      expect(server.lists().some((q) => q.get("q") === "x")).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("says when the filters match nothing, and Clear filters brings every trigger back", async () => {
    const user = userEvent.setup();
    renderPage(`?q=nomatch&kind=flow&state=disabled&namespace=company&flow=${FLOW_A}&page=1`);
    expect(await screen.findByText("No triggers match these filters")).toBeInTheDocument();
    expect(screen.queryByText("No triggers yet")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(bodyRows()).toHaveLength(4));
    expect(where()).toBe(`/explore/${WS}/workflows/triggers`);
    expect(screen.getByLabelText("Search triggers")).toHaveValue("");
    const q = lastList();
    expect([...q.keys()].sort()).toEqual(["limit", "offset"]);
  });

  it("shows the empty state, not the filter miss, when the workspace has no triggers", async () => {
    server.rows = [];
    renderPage();
    expect(await screen.findByText("No triggers yet")).toBeInTheDocument();
    expect(screen.queryByText("No triggers match these filters")).toBeNull();
    expect(screen.queryByRole("button", { name: "Clear filters" })).toBeNull();
  });
});

describe("TriggersList: paging", () => {
  it("pages by 50 through the URL", async () => {
    const user = userEvent.setup();
    server.rows = Array.from({ length: 120 }, (_, i) => row(FLOW_A, webhookEntry({ trigger_id: `t${String(i).padStart(3, "0")}` })));
    renderPage();
    await table();
    expect(bodyRows()).toHaveLength(50);
    expect(screen.getByText("Showing 1–50 of 120")).toBeInTheDocument();
    expect(screen.getByText("Page 1 of 3")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() => expect(lastList().get("offset")).toBe("50"));
    expect(urlParams().get("page")).toBe("2");
    expect(await screen.findByText("Showing 51–100 of 120")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("Showing 101–120 of 120")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Previous" }));
    await waitFor(() => expect(urlParams().get("page")).toBe("2"));
  });

  it("starts each page and filter with nothing selected, so a bulk action acts only on what is seen", async () => {
    const user = userEvent.setup();
    server.rows = Array.from({ length: 60 }, (_, i) => row(FLOW_A, webhookEntry({ trigger_id: `t${String(i).padStart(3, "0")}` })));
    renderPage();
    await table();
    await user.click(screen.getByRole("checkbox", { name: `Select ${who(FLOW_A, "t000")}` }));
    expect(screen.getByText("1 trigger selected")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("Showing 51–60 of 60")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Previous" }));
    expect(await screen.findByText("Showing 1–50 of 60")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: `Select ${who(FLOW_A, "t000")}` })).not.toBeChecked();
    expect(screen.queryByText("1 trigger selected")).toBeNull();
  });

  it("offers the first page when a shared link is past the end", async () => {
    const user = userEvent.setup();
    renderPage("?page=9");
    expect(await screen.findByText("This page is past the end of the list.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Go to the first page" }));
    await waitFor(() => expect(bodyRows()).toHaveLength(4));
    expect(urlParams().get("page")).toBeNull();
  });
});

describe("TriggersList: states", () => {
  it("shows a skeleton while the first page loads", async () => {
    let release!: () => void;
    server.listGate = new Promise((r) => (release = r));
    renderPage();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
    release();
    await table();
    expect(screen.queryByTestId("list-skeleton")).toBeNull();
  });

  it("shows a failed read with the server's sentence and Retry, never as empty", async () => {
    const user = userEvent.setup();
    server.listReply = { status: 503, body: { error: "the database is unavailable" } };
    renderPage();
    expect(await screen.findByText("The triggers could not be loaded: the database is unavailable.")).toBeInTheDocument();
    expect(screen.queryByText("No triggers yet")).toBeNull();
    expect(screen.getByRole("combobox", { name: "Namespace" })).not.toHaveFocus();
    server.listReply = null;
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(bodyRows()).toHaveLength(4));
  });

  it("shows the 422 triggers_too_large sentence, and leaves the focus alone on the first load", async () => {
    server.listReply = TOO_LARGE;
    renderPage();
    expect(await screen.findByText("The triggers could not be listed: more than 2000 flows match; narrow by namespace.")).toBeInTheDocument();
    // FE-A11Y-6: the user has done nothing yet, so nothing moves the focus.
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.getByRole("combobox", { name: "Namespace" })).not.toHaveFocus();
    expect(document.body).toHaveFocus();
    // Narrowing is the way out, not Retry.
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    expect(screen.queryByText("No triggers yet")).toBeNull();
  });

  it("moves the focus to the namespace filter when a filter the user picked answers 422", async () => {
    const user = userEvent.setup();
    renderPage();
    await table();
    server.listReply = TOO_LARGE;
    await user.click(screen.getByRole("combobox", { name: "State" }));
    await user.click(await screen.findByRole("option", { name: "Disabled" }));
    expect(await screen.findByText("The triggers could not be listed: more than 2000 flows match; narrow by namespace.")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("combobox", { name: "Namespace" })).toHaveFocus());
  });

  it("offers Clear filters, not Retry, when the server refuses a shared link's filters (400)", async () => {
    const user = userEvent.setup();
    server.listReply = { status: 400, body: { error: "bad request: flow_id is not a uuid", code: "invalid_query" } };
    renderPage("?flow=not-a-uuid&namespace=company");
    expect(await screen.findByText("The triggers could not be loaded: bad request: flow_id is not a uuid.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
    server.listReply = null;
    await user.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(bodyRows()).toHaveLength(4));
    expect(where()).toBe(`/explore/${WS}/workflows/triggers`);
  });

  it.each([
    [1, "1 flow could not be read; its triggers are not listed"],
    [3, "3 flows could not be read; their triggers are not listed"],
  ])("says when %i unreadable flows are left out", async (n, sentence) => {
    server.unparsable = n;
    renderPage();
    await table();
    expect(screen.getByText(sentence)).toBeInTheDocument();
  });

  it("says nothing about unreadable flows when every flow was read", async () => {
    renderPage();
    await table();
    expect(screen.queryByText(/could not be read/)).toBeNull();
  });

  it("says so even when the unreadable flows leave nothing to list", async () => {
    server.rows = [];
    server.unparsable = 2;
    renderPage();
    expect(await screen.findByText("2 flows could not be read; their triggers are not listed")).toBeInTheDocument();
  });

  it("shows the server-flag banners from the page", async () => {
    server.flags = { scheduler_enabled: false, worker_enabled: false, webhooks_enabled: false };
    renderPage();
    await table();
    const banners = within(screen.getByTestId("trigger-banners"));
    expect(banners.getByText(/The scheduler is off on this server/)).toBeInTheDocument();
    expect(banners.getByText("The worker is off, so triggered executions are created but do not run.")).toBeInTheDocument();
    expect(banners.getByText("Webhooks are off on this server; their URLs answer 404.")).toBeInTheDocument();
  });

  it("says a trigger of a disabled flow is on but does not fire (D18)", async () => {
    server.rows = [row(FLOW_A, webhookEntry({ effective_disabled: true }), { flow_disabled: true })];
    renderPage();
    await table();
    expect(within(bodyRows()[0]).getByTestId("trigger-status")).toHaveTextContent("On, but the flow is disabled, so it does not fire");
  });
});

describe("TriggersList: switching", () => {
  it("switches a selection across two flows off in one bulk request, keyed by flow and trigger", async () => {
    const user = userEvent.setup();
    renderPage();
    await table();
    const readsBefore = server.lists().length;
    await user.click(screen.getByRole("checkbox", { name: `Select ${who(FLOW_A, "daily")}` }));
    await user.click(screen.getByRole("checkbox", { name: `Select ${who(FLOW_B, "daily")}` }));
    // The same trigger id in another flow is a different row: selecting one does not select the other.
    expect(screen.getByRole("checkbox", { name: `Select ${who(FLOW_A, "hook")}` })).not.toBeChecked();
    expect(screen.getByText("2 triggers selected")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Disable" }));
    await waitFor(() => expect(server.bulks()).toHaveLength(1));
    expect(server.bulks()[0]).toEqual({
      triggers: [
        { flow_id: FLOW_A, trigger_id: "daily" },
        { flow_id: FLOW_B, trigger_id: "daily" },
      ],
      disabled: true,
      recover_missed: false,
    });
    // The answer clears the selection and reads the page again (FE-ACTION-2).
    await waitFor(() => expect(screen.queryByText("2 triggers selected")).toBeNull());
    await waitFor(() => expect(server.lists().length).toBeGreaterThan(readsBefore));
    await waitFor(() => expect(screen.getByRole("switch", { name: `Trigger ${who(FLOW_B, "daily")} enabled` })).not.toBeChecked());
    expect(screen.getByRole("switch", { name: `Trigger ${who(FLOW_A, "hook")} enabled` })).toBeChecked();
    expect(toast.success).toHaveBeenCalledWith("Switched 2 triggers off.");
  });

  it("names a refused entry by its flow and trigger, in the server's words", async () => {
    const user = userEvent.setup();
    server.bulkRefusals[`${FLOW_B}/daily`] = { code: "not_found", message: "not found: trigger" };
    renderPage();
    await table();
    await user.click(screen.getByRole("checkbox", { name: `Select ${who(FLOW_A, "daily")}` }));
    await user.click(screen.getByRole("checkbox", { name: `Select ${who(FLOW_B, "daily")}` }));
    await user.click(screen.getByRole("button", { name: "Disable" }));
    const refusals = await screen.findByTestId("bulk-refusals");
    expect(within(refusals).getByText("1 trigger was not switched:")).toBeInTheDocument();
    expect(within(refusals).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["company.data.load · daily: not found: trigger"]);
  });

  it("asks about missed runs before enabling an overdue schedule in the selection", async () => {
    const user = userEvent.setup();
    server.rows = [
      row(FLOW_A, scheduleEntry({ runtime_disabled: true, effective_disabled: true }, { next_evaluation_date: OVERDUE })),
      row(FLOW_B, webhookEntry({ runtime_disabled: true, effective_disabled: true })),
    ];
    renderPage();
    await table();
    await user.click(screen.getByRole("checkbox", { name: "Select all triggers" }));
    await user.click(screen.getByRole("button", { name: "Enable" }));
    const dialog = await screen.findByRole("dialog", { name: "Enable 2 triggers" }, { timeout: 4000 });
    expect(server.bulks()).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: "Enable" }));
    await waitFor(() => expect(server.bulks()).toHaveLength(1));
    expect(server.bulks()[0]).toEqual({
      triggers: [
        { flow_id: FLOW_A, trigger_id: "daily" },
        { flow_id: FLOW_B, trigger_id: "hook" },
      ],
      disabled: false,
      recover_missed: false,
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("says why bulk is off with nothing selected", async () => {
    renderPage();
    await table();
    const disable = screen.getByRole("button", { name: "Disable" });
    expect(disable).toBeDisabled();
    const describedBy = disable.getAttribute("aria-describedby")!;
    expect(document.getElementById(describedBy)).toHaveTextContent("Select triggers first");
  });

  it("is read-only for a viewer without workflow:manage, and says why", async () => {
    server.canManage = false;
    server.rows = ROWS().map((r) => (r.kind === "other" ? r : { ...r, can_toggle: false, toggle_refusal: "forbidden" as const }));
    renderPage();
    await table();
    for (const name of ["Enable", "Disable"]) {
      const button = screen.getByRole("button", { name });
      expect(button).toBeDisabled();
      // The reason is visible text the button names, not only its `title`.
      expect(document.getElementById(button.getAttribute("aria-describedby")!)).toHaveTextContent("You need workflow:manage");
    }
    const all = screen.getByRole("checkbox", { name: "Select all triggers" });
    expect(all).toBeDisabled();
    expect(document.getElementById(all.getAttribute("aria-describedby")!)).toHaveTextContent("You need workflow:manage");
    expect(screen.getByRole("checkbox", { name: `Select ${who(FLOW_A, "daily")}` })).toBeDisabled();
    const sw = screen.getByRole("switch", { name: `Trigger ${who(FLOW_A, "hook")} enabled` });
    expect(sw).toBeDisabled();
    expect(document.getElementById(sw.getAttribute("aria-describedby")!)).toHaveTextContent("You need workflow:manage");
  });
});

describe("TriggersList: switching, continued", () => {
  it("takes workflow:manage from the page's can_manage, even when no row says forbidden", async () => {
    // A viewer's page of Leaf triggers only: each row is refused `not_supported`, which comes
    // before `forbidden` (§3.5), so the rows alone cannot tell a viewer from a manager.
    server.canManage = false;
    server.rows = [row(FLOW_B, otherEntry())];
    renderPage();
    await table();
    const all = screen.getByRole("checkbox", { name: "Select all triggers" });
    expect(all).toBeDisabled();
    expect(document.getElementById(all.getAttribute("aria-describedby")!)).toHaveTextContent("You need workflow:manage");
    const disable = screen.getByRole("button", { name: "Disable" });
    expect(document.getElementById(disable.getAttribute("aria-describedby")!)).toHaveTextContent("You need workflow:manage");
  });

  it("lets a manager select, whatever the rows say", async () => {
    const user = userEvent.setup();
    server.rows = [row(FLOW_B, otherEntry())];
    renderPage();
    await table();
    const all = screen.getByRole("checkbox", { name: "Select all triggers" });
    expect(all).toBeEnabled();
    await user.click(all);
    expect(screen.getByText("1 trigger selected")).toBeInTheDocument();
  });

  it("ignores selection and holds bulk while the next page loads, keeping the focus", async () => {
    const user = userEvent.setup();
    server.rows = Array.from({ length: 60 }, (_, i) => row(FLOW_A, webhookEntry({ trigger_id: `t${String(i).padStart(3, "0")}` })));
    renderPage();
    await table();
    let release!: () => void;
    server.listGate = new Promise((r) => (release = r));
    await user.click(screen.getByRole("button", { name: "Next" }));
    // The rows on screen are page 1's, kept while page 2 is read.
    const box = screen.getByRole("checkbox", { name: `Select ${who(FLOW_A, "t000")}` });
    await waitFor(() => expect(box).toHaveAttribute("aria-disabled", "true"));
    // Not `disabled`: a focused box keeps the focus (jsdom keeps it either way, so the
    // attribute is what is checked).
    expect(box).toBeEnabled();
    box.focus();
    await user.click(box);
    expect(box).not.toBeChecked();
    expect(box).toHaveFocus();
    const all = screen.getByRole("checkbox", { name: "Select all triggers" });
    expect(all).toHaveAttribute("aria-disabled", "true");
    await user.click(all);
    expect(screen.queryByText(/triggers? selected/)).toBeNull();
    // Bulk cannot act: the new page's selection is empty, and the bar is held while it loads.
    for (const name of ["Enable", "Disable"]) {
      const b = screen.getByRole("button", { name });
      expect(b.hasAttribute("disabled") || b.getAttribute("aria-disabled") === "true").toBe(true);
    }
    release();
    expect(await screen.findByText("Showing 51–60 of 60")).toBeInTheDocument();
    const fresh = screen.getByRole("checkbox", { name: `Select ${who(FLOW_A, "t050")}` });
    expect(fresh).not.toHaveAttribute("aria-disabled");
    await user.click(fresh);
    expect(screen.getByText("1 trigger selected")).toBeInTheDocument();
  });

  it("drops the bulk refusals when the filters or the page change", async () => {
    const user = userEvent.setup();
    server.bulkRefusals[`${FLOW_B}/daily`] = { code: "not_found", message: "not found: trigger" };
    renderPage();
    await table();
    await user.click(screen.getByRole("checkbox", { name: `Select ${who(FLOW_B, "daily")}` }));
    await user.click(screen.getByRole("button", { name: "Disable" }));
    expect(await screen.findByTestId("bulk-refusals")).toBeInTheDocument();
    await user.click(screen.getByRole("combobox", { name: "State" }));
    await user.click(await screen.findByRole("option", { name: "Enabled" }));
    await waitFor(() => expect(lastList().get("state")).toBe("enabled"));
    await waitFor(() => expect(screen.queryByTestId("bulk-refusals")).toBeNull());
  });
});

describe("TriggersList: the details drawer", () => {
  it("opens from Details with the flow and the trigger in the URL, and reads that flow's trigger", async () => {
    const user = userEvent.setup();
    renderPage("?kind=webhook,schedule");
    await table();
    await user.click(screen.getByRole("link", { name: `Details of ${who(FLOW_B, "daily")}` }));
    expect(urlParams().get("trigger")).toBe(`${FLOW_B}/daily`);
    // The filters stay as they were.
    expect(urlParams().get("kind")).toBe("webhook,schedule");
    const dialog = await screen.findByRole("dialog", { name: "Trigger daily in company.data.load" }, { timeout: 4000 });
    // FLOW_B's `daily`, not FLOW_A's: the cron of its own YAML.
    expect(await within(dialog).findByText(/cron: "0 \* \* \* \*"/)).toBeInTheDocument();
    expect(server.requests.some((r) => r.method === "GET" && r.path === `${BASE}/flows/${FLOW_B}/triggers`)).toBe(true);
    expect(server.requests.some((r) => r.method === "GET" && r.path === `${BASE}/flows/${FLOW_A}/triggers`)).toBe(false);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(urlParams().get("trigger")).toBeNull());
    expect(urlParams().get("kind")).toBe("webhook,schedule");
  });

  it("opens from a shared link, with the webhook's key hidden", async () => {
    renderPage(`?trigger=${FLOW_A}/hook`);
    const dialog = await screen.findByRole("dialog", { name: "Trigger hook in company.team.hello" }, { timeout: 4000 });
    expect(await within(dialog).findByText("The key is hidden; Reveal under Webhook shows it.")).toBeInTheDocument();
    expect(dialog.textContent).not.toContain("s3cr3t-key");
  });

  it("says the trigger is gone when the flow no longer declares it", async () => {
    renderPage(`?trigger=${FLOW_A}/vanished`);
    const dialog = await screen.findByRole("dialog", { name: "Trigger vanished in company.team.hello" }, { timeout: 4000 });
    expect(await within(dialog).findByText("This trigger no longer exists in the flow; reload.")).toBeInTheDocument();
  });

  it("shows a skeleton while it reads, then the details in the same drawer", async () => {
    let release!: () => void;
    server.flowTriggersGate = new Promise((r) => (release = r));
    renderPage(`?trigger=${FLOW_A}/daily`);
    const dialog = await screen.findByRole("dialog", { name: "Trigger daily in company.team.hello" }, { timeout: 4000 });
    expect(within(dialog).getByTestId("trigger-drawer-loading")).toBeInTheDocument();
    // Not "no longer exists" while the answer is on its way.
    expect(within(dialog).queryByText("This trigger no longer exists in the flow; reload.")).toBeNull();
    release();
    // The drawer that opened is the one that fills: it does not close and open again.
    expect(await within(dialog).findByText(/cron: "\*\/5 \* \* \* \*"/)).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Trigger daily in company.team.hello" })).toBe(dialog);
  });

  it("says why when the flow's triggers cannot be read", async () => {
    server.flowTriggersReply = { status: 503, body: { error: "the database is unavailable" } };
    renderPage(`?trigger=${FLOW_A}/daily`);
    const dialog = await screen.findByRole("dialog", { name: "Trigger daily in company.team.hello" }, { timeout: 4000 });
    expect(
      await within(dialog).findByText("The trigger's details could not be loaded: the database is unavailable."),
    ).toBeInTheDocument();
  });
});
