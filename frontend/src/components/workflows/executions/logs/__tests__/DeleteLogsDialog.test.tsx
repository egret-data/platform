import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { DeleteLogsDialog } from "@/components/workflows/executions/logs/DeleteLogsDialog";
import { EXEC, LOGS_PATH, WS } from "./logServer";

type Reply = { status: number; body?: unknown };
type Call = { method: string; path: string; query: URLSearchParams };

/** The log delete route from memory, through a stubbed `fetch` (FE-TEST-1). */
class Server {
  calls: Call[] = [];
  reply: Reply = { status: 204 };
  constructor() {
    vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
      const method = (init?.method ?? "GET").toUpperCase();
      const [path, qs] = String(input).split("?");
      this.calls.push({ method, path, query: new URLSearchParams(qs ?? "") });
      const { status, body } = method === "DELETE" && path === LOGS_PATH ? this.reply : { status: 404, body: { error: "no route" } };
      return new Response(status === 204 ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    }));
  }
  deletes() {
    return this.calls.filter((c) => c.method === "DELETE" && c.path === LOGS_PATH);
  }
}

function renderDialog() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const onOpenChange = vi.fn();
  const onDeleted = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <DeleteLogsDialog
        ws={WS}
        executionId={EXEC}
        filters={{ taskRunId: "r-1", minLevel: "WARN" }}
        scope="the logs of extract #1"
        open
        onOpenChange={onOpenChange}
        onDeleted={onDeleted}
      />
    </QueryClientProvider>,
  );
  return { onOpenChange, onDeleted };
}

let server: Server;
beforeEach(() => {
  server = new Server();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("DeleteLogsDialog", () => {
  it("asks first, naming what it deletes, then sends the filters and reports", async () => {
    const { onOpenChange, onDeleted } = renderDialog();
    const dialog = screen.getByRole("alertdialog", { name: "Delete these logs?" });
    expect(dialog).toHaveTextContent("Deletes the logs of extract #1. Lines written after this stay. This cannot be undone.");
    expect(server.deletes()).toHaveLength(0);
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete logs" }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalledTimes(1));
    const q = server.deletes()[0].query;
    expect(q.get("task_run_id")).toBe("r-1");
    expect(q.get("min_level")).toBe("WARN");
    expect(q.has("task_id")).toBe(false);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("stays open with the server's message when the delete fails", async () => {
    server.reply = { status: 500, body: { error: "the database is unavailable" } };
    const { onOpenChange, onDeleted } = renderDialog();
    const dialog = screen.getByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete logs" }));
    expect(await within(dialog).findByText("the database is unavailable")).toBeInTheDocument();
    expect(onDeleted).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });
});
