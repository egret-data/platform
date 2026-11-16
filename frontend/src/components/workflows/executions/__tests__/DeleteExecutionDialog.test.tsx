import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import type { ExecutionDto } from "@/api/generated/model";
import { deleteRefusal } from "@/components/workflows/executions/actions";
import { DeleteExecutionDialog } from "@/components/workflows/executions/DeleteExecutionDialog";

const WS = "eng";
const EXEC = "5e5e5e5e-0000-4000-8000-0000000000aa";
const PATH = `/api/ws/${WS}/workflows/executions/${EXEC}`;

const execution = { id: EXEC, state: "SUCCESS", idle: true, parent_execution_id: null } as unknown as ExecutionDto;

type Reply = { status: number; body?: unknown };
type Call = { method: string; path: string; query: URLSearchParams };

/** The delete route from memory, through a stubbed `fetch` (FE-TEST-1). */
class Server {
  calls: Call[] = [];
  reply: Reply = { status: 204 };
  constructor() {
    vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
      const method = (init?.method ?? "GET").toUpperCase();
      const [path, qs] = String(input).split("?");
      this.calls.push({ method, path, query: new URLSearchParams(qs ?? "") });
      const { status, body } = method === "DELETE" && path === PATH ? this.reply : { status: 404, body: { error: "no route" } };
      return new Response(status === 204 ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    }));
  }
  deletes() {
    return this.calls.filter((c) => c.method === "DELETE" && c.path === PATH);
  }
}

function Where() {
  const l = useLocation();
  return <output data-testid="where">{l.pathname}</output>;
}

function renderDialog() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const onOpenChange = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/explore/${WS}/workflows/executions/${EXEC}/overview`]}>
        <Routes>
          <Route path="*" element={<DeleteExecutionDialog ws={WS} execution={execution} open onOpenChange={onOpenChange} />} />
        </Routes>
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { onOpenChange };
}

let server: Server;
beforeEach(() => {
  toast.success.mockReset();
  server = new Server();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("DeleteExecutionDialog", () => {
  it("asks first, sends deleteStorage=true, then opens the executions list and says so", async () => {
    renderDialog();
    const dialog = screen.getByRole("alertdialog", { name: "Delete this execution?" });
    expect(dialog).toHaveTextContent(`Deletes execution ${EXEC} with its task runs, logs, loop iterations and retries. This cannot be undone.`);
    expect(within(dialog).getByRole("checkbox", { name: "Delete stored files" })).toBeChecked();
    expect(server.deletes()).toHaveLength(0);
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    // Exactly the list: the execution's own page starts with the same path.
    await waitFor(() => expect(screen.getByTestId("where").textContent).toBe(`/explore/${WS}/workflows/executions`));
    expect(server.deletes()).toHaveLength(1);
    expect(server.deletes()[0].query.get("deleteStorage")).toBe("true");
    expect(toast.success).toHaveBeenCalledWith(`Deleted execution ${EXEC}`);
  });

  it("sends deleteStorage=false when Delete stored files is cleared", async () => {
    renderDialog();
    const dialog = screen.getByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("checkbox", { name: "Delete stored files" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(server.deletes()).toHaveLength(1));
    expect(server.deletes()[0].query.get("deleteStorage")).toBe("false");
  });

  it("on has_dependents, stays open with the sentence, a link to each dependent, and how many more", async () => {
    server.reply = {
      status: 409,
      body: { error: "3 other executions point at this one", code: "has_dependents", details: { executions: ["dep-a", "dep-b"], total: 3 } },
    };
    const { onOpenChange } = renderDialog();
    const dialog = screen.getByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByText(deleteRefusal("has_dependents")!)).toBeInTheDocument();
    expect(within(dialog).getByRole("link", { name: "dep-a" })).toHaveAttribute("href", `/explore/${WS}/workflows/executions/dep-a/overview`);
    expect(within(dialog).getByRole("link", { name: "dep-b" })).toHaveAttribute("href", `/explore/${WS}/workflows/executions/dep-b/overview`);
    expect(within(dialog).getByText("and 1 more")).toBeInTheDocument();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(screen.getByTestId("where").textContent).toBe(`/explore/${WS}/workflows/executions/${EXEC}/overview`);
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("shows the sentence of a coded refusal, and the server's message otherwise", async () => {
    server.reply = { status: 409, body: { error: "not terminated", code: "not_terminated" } };
    renderDialog();
    const dialog = screen.getByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByText(deleteRefusal("not_terminated")!)).toBeInTheDocument();

    server.reply = { status: 500, body: { error: "the store is unavailable" } };
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByText("the store is unavailable")).toBeInTheDocument();
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
  });
});
