import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import type { ExecutionDto, TaskRunDto } from "@/api/generated/model";
import { ReplayDialog } from "@/components/workflows/executions/ReplayDialog";

const WS = "eng";
const EXEC = "5e5e5e5e-0000-4000-8000-0000000000aa";
const NEW = "5e5e5e5e-0000-4000-8000-0000000000bb";
const FLOW = "0a7c1d2e-0000-4000-8000-000000000001";
const BASE = `/api/ws/${WS}/workflows`;
const REPLAY = `${BASE}/executions/${EXEC}/replay`;
const RESTART = `${BASE}/executions/${EXEC}/restart`;

const sourceWith = (inputs: string, message = "hi") =>
  `id: hello\nnamespace: company.team\n${inputs ? `inputs:\n${inputs}\n` : ""}tasks:\n  - id: extract\n    type: io.kestra.plugin.core.log.Log\n    message: ${message}\n`;
const INPUTS = "  - id: who\n    type: STRING\n  - id: token\n    type: SECRET\n    required: false";

function execution(extra: Partial<ExecutionDto> = {}): ExecutionDto {
  return {
    id: EXEC,
    flow_id: FLOW,
    namespace: "company.team",
    flow_key: "hello",
    state: "FAILED",
    history: [],
    inputs: {},
    seq: 1,
    execution_trigger: null,
    effective_labels: [],
    attempt_number: 1,
    origin_offset_ms: 0,
    created_at: "2026-10-10T09:59:00.000001Z",
    updated_at: "2026-10-10T10:06:05.000001Z",
    source: sourceWith(""),
    runs: [],
    can_manage: true,
    worker_enabled: true,
    paused_by_hand: false,
    ...extra,
  } as ExecutionDto;
}

const taskRun = { id: "r-0", task_id: "extract", run_ix: 0 } as TaskRunDto;

const version = (n: number, source: string) => ({
  id: `v${n}`,
  flow_id: FLOW,
  version: n,
  snapshot: { source },
  note: `note ${n}`,
  created_by_name: "Ada",
  created_at: "2026-10-0" + n + "T09:00:00.000001Z",
});

type Reply = { status: number; body: unknown };
type Call = { method: string; path: string; query: URLSearchParams; body: unknown };

class Server {
  calls: Call[] = [];
  versions: unknown[] = [version(2, sourceWith(INPUTS, "two")), version(1, sourceWith("", "one"))];
  replay: Reply = { status: 201, body: { ...execution({ state: "RUNNING" }), id: NEW } };
  restart: Reply = { status: 201, body: { ...execution({ state: "RUNNING" }), id: NEW } };
  /** When set, the flow's current source is answered only once this resolves. */
  flowGate: Promise<void> | null = null;
  constructor() {
    vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
      const method = (init?.method ?? "GET").toUpperCase();
      const [path, qs] = String(input).split("?");
      if (method === "GET" && path === `${BASE}/flows/${FLOW}` && this.flowGate) await this.flowGate;
      const body = typeof init?.body === "string" ? JSON.parse(init.body) : init?.body;
      this.calls.push({ method, path, query: new URLSearchParams(qs ?? ""), body });
      const { status, body: out } = this.route(method, path);
      return new Response(JSON.stringify(out), { status, headers: { "content-type": "application/json" } });
    }));
  }
  private route(method: string, path: string): Reply {
    if (method === "POST" && path === REPLAY) return this.replay;
    if (method === "POST" && path === RESTART) return this.restart;
    if (method === "GET" && path === `${BASE}/flows/${FLOW}/versions`) return { status: 200, body: this.versions };
    if (method === "GET" && path === `${BASE}/flows/${FLOW}`) return { status: 200, body: { id: FLOW, source: sourceWith(INPUTS, "current") } };
    const m = path.match(/\/versions\/(\d+)$/);
    if (method === "GET" && m) return { status: 200, body: this.versions.find((v) => (v as { version: number }).version === Number(m[1])) };
    if (method === "GET" && path === `${BASE}/executions/${NEW}`) return { status: 200, body: { ...execution({ state: "RUNNING" }), id: NEW } };
    return { status: 404, body: { error: `no route ${method} ${path}` } };
  }
  posts(path: string) {
    return this.calls.filter((c) => c.method === "POST" && c.path === path);
  }
}

function Where() {
  const l = useLocation();
  return <output data-testid="where">{l.pathname}</output>;
}

function renderDialog(props: Partial<Parameters<typeof ReplayDialog>[0]> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const onOpenChange = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/start"]}>
        <Routes>
          <Route
            path="*"
            element={<ReplayDialog ws={WS} execution={execution()} mode="replay" open onOpenChange={onOpenChange} {...props} />}
          />
        </Routes>
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { onOpenChange };
}

const dialog = () => screen.findByRole("alertdialog");

beforeEach(() => {
  toast.success.mockReset();
  toast.error.mockReset();
  toast.warning.mockReset();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ReplayDialog: replay", () => {
  it("sends the frozen source and an empty body by default, then opens the new execution", async () => {
    const server = new Server();
    renderDialog();
    const d = await dialog();
    expect(within(d).getByRole("heading", { name: "Replay this execution?" })).toBeInTheDocument();
    expect(within(d).getByRole("radio", { name: "This run's version" })).toBeChecked();
    await userEvent.click(within(d).getByRole("button", { name: "Replay" }));
    await waitFor(() => expect(server.posts(REPLAY)).toHaveLength(1));
    const call = server.posts(REPLAY)[0];
    expect(call.query.get("source")).toBe("frozen");
    expect(call.query.get("taskRunId")).toBeNull();
    expect(call.body).toEqual({});
    await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent(`/explore/${WS}/workflows/executions/${NEW}/overview`));
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("titles and sends a replay from a task", async () => {
    const server = new Server();
    renderDialog({ taskRun });
    const d = await dialog();
    expect(within(d).getByRole("heading", { name: "Replay from extract?" })).toBeInTheDocument();
    await userEvent.click(within(d).getByRole("button", { name: "Replay" }));
    await waitFor(() => expect(server.posts(REPLAY)).toHaveLength(1));
    expect(server.posts(REPLAY)[0].query.get("taskRunId")).toBe("r-0");
  });

  it("sends a chosen published version", async () => {
    const server = new Server();
    renderDialog();
    const d = await dialog();
    await userEvent.click(within(d).getByRole("radio", { name: "A published version" }));
    await userEvent.click(within(d).getByRole("combobox", { name: "Published version" }));
    expect(await screen.findByRole("option", { name: /Version 2/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("option", { name: /Version 2/ }));
    await userEvent.click(within(d).getByRole("button", { name: "Replay" }));
    await waitFor(() => expect(server.posts(REPLAY)).toHaveLength(1));
    expect(server.posts(REPLAY)[0].query.get("source")).toBe("version:2");
  });

  it("disables the published versions, saying why, when the flow has none", async () => {
    const server = new Server();
    server.versions = [];
    renderDialog();
    const d = await dialog();
    expect(await within(d).findByText("This flow has no published versions")).toBeInTheDocument();
    expect(within(d).getByRole("radio", { name: "A published version" })).toBeDisabled();
    expect(within(d).getByRole("combobox", { name: "Published version" })).toBeDisabled();
  });

  it("prefills Change inputs from the execution's inputs, but never a SECRET", async () => {
    new Server();
    renderDialog({ execution: execution({ source: sourceWith(INPUTS), inputs: { who: "world", token: "******" } }) });
    const d = await dialog();
    await userEvent.click(within(d).getByRole("button", { name: "Change inputs" }));
    expect(await within(d).findByLabelText(/who/)).toHaveValue("world");
    const token = within(d).getByLabelText(/token/);
    expect(token).toHaveValue("");
    expect(within(d).getByText(/Enter it again/)).toBeInTheDocument();
  });

  it("sends the changed inputs as JSON", async () => {
    const server = new Server();
    renderDialog({ execution: execution({ source: sourceWith(INPUTS), inputs: { who: "world", token: "******" } }) });
    const d = await dialog();
    await userEvent.click(within(d).getByRole("button", { name: "Change inputs" }));
    const who = await within(d).findByLabelText(/who/);
    await userEvent.clear(who);
    await userEvent.type(who, "moon");
    await userEvent.click(within(d).getByRole("button", { name: "Replay" }));
    await waitFor(() => expect(server.posts(REPLAY)).toHaveLength(1));
    expect(server.posts(REPLAY)[0].body).toEqual({ inputs: { who: "moon" } });
  });

  it("shows the sentence of a replay_task_not_in_source refusal with the server's message, and stays open", async () => {
    const server = new Server();
    server.replay = { status: 400, body: { error: "task extract is not in version 2", code: "replay_task_not_in_source" } };
    const { onOpenChange } = renderDialog({ taskRun });
    const d = await dialog();
    await userEvent.click(within(d).getByRole("button", { name: "Replay" }));
    expect(await within(d).findByText(/no task to start from\. task extract is not in version 2/)).toBeInTheDocument();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(screen.getByTestId("where")).toHaveTextContent("/start");
  });
});

describe("ReplayDialog: restart", () => {
  it("restarts in place by default: 200 refreshes and toasts", async () => {
    const server = new Server();
    server.restart = { status: 200, body: execution({ state: "RUNNING" }) };
    const { onOpenChange } = renderDialog({ mode: "restart" });
    const d = await dialog();
    expect(within(d).getByRole("heading", { name: "Restart this execution?" })).toBeInTheDocument();
    expect(d).toHaveTextContent("The failed tasks run again in this same execution");
    expect(within(d).queryByRole("button", { name: "Change inputs" })).toBeNull();
    await userEvent.click(within(d).getByRole("button", { name: "Restart" }));
    await waitFor(() => expect(server.posts(RESTART)).toHaveLength(1));
    expect(server.posts(RESTART)[0].query.get("source")).toBe("frozen");
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Restarted the execution"));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("on the current version says it creates a new execution and opens it on 201", async () => {
    const server = new Server();
    renderDialog({ mode: "restart" });
    const d = await dialog();
    await userEvent.click(within(d).getByRole("radio", { name: "The flow's current version" }));
    expect(d).toHaveTextContent("Restarting on another version creates a new execution");
    await userEvent.click(within(d).getByRole("button", { name: "Restart" }));
    await waitFor(() => expect(server.posts(RESTART)).toHaveLength(1));
    expect(server.posts(RESTART)[0].query.get("source")).toBe("current");
    await waitFor(() => expect(screen.getByTestId("where")).toHaveTextContent(`/executions/${NEW}/overview`));
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("explains a replay_task_not_in_source refusal in restart terms, with the server's message", async () => {
    const server = new Server();
    server.restart = { status: 400, body: { error: "task extract is not in version 2", code: "replay_task_not_in_source" } };
    const { onOpenChange } = renderDialog({ mode: "restart" });
    const d = await dialog();
    await userEvent.click(within(d).getByRole("radio", { name: "The flow's current version" }));
    await userEvent.click(within(d).getByRole("button", { name: "Restart" }));
    const alert = await within(d).findByText(/cannot be restarted on it/);
    expect(alert).toHaveTextContent(/task extract is not in version 2$/);
    expect(within(d).queryByText(/no task to start from/)).toBeNull();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });
});

describe("ReplayDialog: the review's fixes (C1 round 1)", () => {
  const FILES = "  - id: who\n    type: STRING\n  - id: doc\n    type: FILE\n  - id: extra\n    type: FILE\n    required: false";
  const STORED = "kestra:///company/team/hello/old/doc.txt";

  it("says why the other versions are unavailable when the flow was deleted for good (restart)", async () => {
    new Server();
    renderDialog({ mode: "restart", execution: execution({ flow_id: null }) });
    const d = await dialog();
    expect(within(d).getByRole("radio", { name: "The flow's current version" })).toBeDisabled();
    expect(within(d).getByRole("radio", { name: "A published version" })).toBeDisabled();
    expect(within(d).getByText("The flow was deleted permanently, so only this run's version is available")).toBeInTheDocument();
  });

  it("disables the published versions, saying it is loading, until the list arrives", async () => {
    new Server();
    renderDialog();
    const d = await dialog();
    const radio = within(d).getByRole("radio", { name: "A published version" });
    expect(radio).toBeDisabled();
    expect(radio).toHaveAccessibleDescription("Loading the published versions");
    await waitFor(() => expect(radio).toBeEnabled());
  });

  it("opens Change inputs and shows the error when a required input is missing, instead of doing nothing", async () => {
    const server = new Server();
    const src = sourceWith("  - id: who\n    type: STRING\n  - id: token\n    type: SECRET");
    renderDialog({ execution: execution({ source: src, inputs: { who: "world", token: "******" } }) });
    const d = await dialog();
    await userEvent.click(within(d).getByRole("button", { name: "Change inputs" }));
    const who = await within(d).findByLabelText(/who/);
    await userEvent.type(who, "!");
    // Collapse the section, then send: the secret was not entered again.
    await userEvent.click(within(d).getByRole("button", { name: "Change inputs" }));
    expect(within(d).queryByLabelText(/who/)).toBeNull();
    await userEvent.click(within(d).getByRole("button", { name: "Replay" }));
    expect(await within(d).findByLabelText(/token/)).toBeInTheDocument();
    expect(within(d).getAllByText(/required|Enter/i).length).toBeGreaterThan(0);
    expect(server.posts(REPLAY)).toHaveLength(0);
  });

  it("keeps the edited inputs that the other source also declares when the source is switched", async () => {
    new Server();
    renderDialog({ execution: execution({ source: sourceWith(INPUTS), inputs: { who: "world" } }) });
    const d = await dialog();
    await userEvent.click(within(d).getByRole("button", { name: "Change inputs" }));
    const who = await within(d).findByLabelText(/who/);
    await userEvent.clear(who);
    await userEvent.type(who, "moon");
    await userEvent.click(within(d).getByRole("radio", { name: "The flow's current version" }));
    await waitFor(() => expect(within(d).getByRole("button", { name: "Change inputs" })).toBeEnabled());
    expect(await within(d).findByLabelText(/who/)).toHaveValue("moon");
  });

  it("does not drop edited inputs while the chosen source loads: Replay waits, saying why (M5)", async () => {
    const server = new Server();
    let release = () => {};
    server.flowGate = new Promise<void>((r) => {
      release = r;
    });
    renderDialog({ execution: execution({ source: sourceWith(INPUTS), inputs: { who: "world" } }) });
    const d = await dialog();
    await userEvent.click(within(d).getByRole("button", { name: "Change inputs" }));
    const who = await within(d).findByLabelText(/who/);
    await userEvent.clear(who);
    await userEvent.type(who, "moon");
    await userEvent.click(within(d).getByRole("radio", { name: "The flow's current version" }));
    const replay = within(d).getByRole("button", { name: "Replay" });
    expect(replay).toBeDisabled();
    expect(replay).toHaveAccessibleDescription(/changed inputs/);
    release();
    // The body remounts once the text arrives, so the button is found again.
    await waitFor(() => expect(within(d).getByRole("button", { name: "Replay" })).toBeEnabled());
    await userEvent.click(within(d).getByRole("button", { name: "Replay" }));
    await waitFor(() => expect(server.posts(REPLAY)).toHaveLength(1));
    expect(server.posts(REPLAY)[0].query.get("source")).toBe("current");
    expect(server.posts(REPLAY)[0].body).toEqual({ inputs: { who: "moon" } });
  });

  it("sends a FILE input left alone as its old kestra:// value, in JSON", async () => {
    const server = new Server();
    renderDialog({ execution: execution({ source: sourceWith(FILES), inputs: { who: "world", doc: STORED } }) });
    const d = await dialog();
    await userEvent.click(within(d).getByRole("button", { name: "Change inputs" }));
    const who = await within(d).findByLabelText(/who/);
    await userEvent.type(who, "!");
    await userEvent.click(within(d).getByRole("button", { name: "Replay" }));
    await waitFor(() => expect(server.posts(REPLAY)).toHaveLength(1));
    expect(server.posts(REPLAY)[0].body).toEqual({ inputs: { who: "world!", doc: STORED } });
  });

  it("sends the old value of an unchanged FILE input beside a new file, as multipart", async () => {
    const server = new Server();
    renderDialog({ execution: execution({ source: sourceWith(FILES), inputs: { who: "world", doc: STORED } }) });
    const d = await dialog();
    await userEvent.click(within(d).getByRole("button", { name: "Change inputs" }));
    const extra = await within(d).findByLabelText(/extra/);
    await userEvent.upload(extra, new File(["x"], "extra.txt", { type: "text/plain" }));
    await userEvent.click(within(d).getByRole("button", { name: "Replay" }));
    await waitFor(() => expect(server.posts(REPLAY)).toHaveLength(1));
    const body = server.posts(REPLAY)[0].body as FormData;
    expect(body).toBeInstanceOf(FormData);
    expect(body.get("doc")).toBe(STORED);
    expect((body.get("extra") as File).name).toBe("extra.txt");
  });
});
