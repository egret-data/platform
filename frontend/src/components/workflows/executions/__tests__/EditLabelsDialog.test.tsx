import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import type { ExecutionDto } from "@/api/generated/model";
import { EditLabelsDialog } from "@/components/workflows/executions/EditLabelsDialog";

const WS = "eng";
const EXEC = "5e5e5e5e-0000-4000-8000-0000000000aa";
const PATH = `/api/ws/${WS}/workflows/executions/${EXEC}/labels`;

const execution = {
  id: EXEC,
  state: "SUCCESS",
  idle: true,
  effective_labels: [
    { key: "team", value: "data" },
    { key: "system.restarted", value: "true" },
  ],
} as unknown as ExecutionDto;

type Reply = { status: number; body: unknown };
type Call = { method: string; path: string; body: unknown };

/** The labels route from memory, through a stubbed `fetch` (FE-TEST-1). */
class Server {
  calls: Call[] = [];
  reply: Reply = { status: 200, body: execution };
  /** When set, a write waits for it before answering. */
  gate: Promise<void> | null = null;
  constructor() {
    vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
      const method = (init?.method ?? "GET").toUpperCase();
      const [path] = String(input).split("?");
      const body = typeof init?.body === "string" ? JSON.parse(init.body) : init?.body;
      this.calls.push({ method, path, body });
      if (method !== "GET" && this.gate) await this.gate;
      const { status, body: out } = path === PATH ? this.reply : { status: 404, body: { error: "no route" } };
      return new Response(JSON.stringify(out), { status, headers: { "content-type": "application/json" } });
    }));
  }
  writes() {
    return this.calls.filter((c) => c.path === PATH && c.method !== "GET");
  }
}

function renderDialog() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const onOpenChange = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <EditLabelsDialog ws={WS} execution={execution} open onOpenChange={onOpenChange} />
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

describe("EditLabelsDialog", () => {
  it("opens with the effective labels; a system label is read only and cannot be removed", async () => {
    renderDialog();
    const dialog = screen.getByRole("dialog", { name: "Edit labels" });
    expect(dialog).toHaveTextContent("These labels replace the execution's labels, the flow's included. Labels set by egret (system.*) are kept.");
    expect(within(dialog).getByRole("textbox", { name: "Key of label 1" })).toHaveValue("team");
    expect(within(dialog).getByRole("textbox", { name: "Value of label 1" })).toHaveValue("data");
    expect(within(dialog).getByRole("textbox", { name: "Key of label 2" })).toHaveValue("system.restarted");
    expect(within(dialog).getByRole("textbox", { name: "Key of label 2" })).toHaveAttribute("readonly");
    expect(within(dialog).getByRole("textbox", { name: "Value of label 2" })).toHaveAttribute("readonly");
    expect(within(dialog).getByText("Set by egret; kept as it is")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Remove label team" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Remove label system.restarted" })).toBeNull();
  });

  it("removes one, adds one, saves the whole list in order, toasts and closes", async () => {
    const { onOpenChange } = renderDialog();
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove label team" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Add label" }));
    // The new row's key takes focus: the user asked for the row.
    const key = within(dialog).getByRole("textbox", { name: "Key of label 2" });
    expect(key).toHaveFocus();
    await userEvent.type(key, "env");
    await userEvent.type(within(dialog).getByRole("textbox", { name: "Value of label 2" }), "dev");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(server.writes()).toHaveLength(1));
    expect(server.writes()[0].method).toBe("POST");
    expect(JSON.stringify(server.writes()[0].body)).toBe('[{"key":"system.restarted","value":"true"},{"key":"env","value":"dev"}]');
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(toast.success).toHaveBeenCalledWith("Saved the labels");
  });

  it("shows Enter a value under the empty field and sends nothing", async () => {
    renderDialog();
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add label" }));
    await userEvent.type(within(dialog).getByRole("textbox", { name: "Key of label 3" }), "env");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    const value = within(dialog).getByRole("textbox", { name: "Value of label 3" });
    expect(value).toHaveAccessibleDescription("Enter a value");
    expect(value).toHaveAttribute("aria-invalid", "true");
    expect(server.writes()).toHaveLength(0);
  });

  it("keeps the rows and shows the server's words on invalid_labels", async () => {
    server.reply = { status: 400, body: { error: "label key `Env` must start with a lower-case letter", code: "invalid_labels" } };
    const { onOpenChange } = renderDialog();
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add label" }));
    await userEvent.type(within(dialog).getByRole("textbox", { name: "Key of label 3" }), "Env");
    await userEvent.type(within(dialog).getByRole("textbox", { name: "Value of label 3" }), "x");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await within(dialog).findByText("label key `Env` must start with a lower-case letter")).toBeInTheDocument();
    expect(within(dialog).getByRole("textbox", { name: "Key of label 3" })).toHaveValue("Env");
    expect(within(dialog).getByRole("textbox", { name: "Key of label 1" })).toHaveValue("team");
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("ignores Escape and Cancel while the save is in flight, then closes when it lands", async () => {
    let release!: () => void;
    server.gate = new Promise<void>((r) => (release = r));
    const { onOpenChange } = renderDialog();
    const dialog = screen.getByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(server.writes()).toHaveLength(1));
    await userEvent.keyboard("{Escape}");
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled();
    expect(onOpenChange).not.toHaveBeenCalled();
    release();
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });
});
