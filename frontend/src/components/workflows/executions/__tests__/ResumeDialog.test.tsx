import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider, focusManager } from "@tanstack/react-query";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import type { ExecutionDto } from "@/api/generated/model";
import { resumeRefusal } from "@/components/workflows/executions/actions";
import { RESUME_BY_HAND, RESUME_NO_INPUTS, ResumeDialog } from "@/components/workflows/executions/ResumeDialog";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

const WS = "eng";
const EXEC = "5e5e5e5e-0000-4000-8000-0000000000aa";
const BASE = `/api/ws/${WS}/workflows`;
const RESUME = `${BASE}/executions/${EXEC}/resume`;
const VALIDATE = `${RESUME}/validate`;

function execution(extra: Partial<ExecutionDto> = {}): ExecutionDto {
  return { id: EXEC, state: "PAUSED", runs: [], paused_by_hand: false, ...extra } as ExecutionDto;
}

const outcome = (input: Record<string, unknown>, errors: { message: string; renderError: boolean }[] = []) => ({
  input,
  value: null,
  enabled: true,
  errors,
});
const answer = (inputs: unknown[]) => ({ id: "f", namespace: "n", inputs, checks: [] });

type Reply = { status: number; body: unknown };
type Call = { method: string; path: string; body: unknown };

class Server {
  calls: Call[] = [];
  validate: Reply = { status: 200, body: answer([]) };
  /** Replies for the validate calls after the first (the one the dialog makes on opening), in turn. */
  checks: Reply[] = [];
  resume: Reply = { status: 200, body: { id: EXEC, state: "RUNNING" } };
  /** When set, the first validate answer waits for it. */
  gate: Promise<void> | null = null;
  constructor() {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string, init?: RequestInit) => {
        const method = (init?.method ?? "GET").toUpperCase();
        const path = String(input).split("?")[0];
        const body = typeof init?.body === "string" ? JSON.parse(init.body) : init?.body;
        const nth = this.posts(VALIDATE).length;
        this.calls.push({ method, path, body });
        let reply: Reply = { status: 404, body: { error: `no route ${method} ${path}` } };
        if (method === "POST" && path === VALIDATE) {
          if (nth === 0 && this.gate) await this.gate;
          reply = nth === 0 ? this.validate : (this.checks[nth - 1] ?? this.validate);
        }
        if (method === "POST" && path === RESUME) reply = this.resume;
        return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
      }),
    );
  }
  posts(path: string) {
    return this.calls.filter((c) => c.method === "POST" && c.path === path);
  }
}

function renderDialog(exec: ExecutionDto = execution()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const onOpenChange = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <ResumeDialog ws={WS} execution={exec} open onOpenChange={onOpenChange} />
    </QueryClientProvider>,
  );
  return { onOpenChange, client };
}

const dialog = () => screen.findByRole("alertdialog");
const resumeButton = (d: HTMLElement) => within(d).getByRole("button", { name: "Resume" });
const INT_N = outcome({ id: "n", type: "INT" });

beforeEach(() => {
  toast.success.mockReset();
  toast.error.mockReset();
  toast.warning.mockReset();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ResumeDialog", () => {
  it("with no inputs says what resume does, and resumes with an empty body", async () => {
    const server = new Server();
    const { onOpenChange } = renderDialog();
    const d = await dialog();
    expect(within(d).getByRole("heading", { name: "Resume this execution?" })).toBeInTheDocument();
    expect(d).toHaveTextContent(RESUME_NO_INPUTS);
    await waitFor(() => expect(resumeButton(d)).toBeEnabled());
    await userEvent.click(resumeButton(d));
    await waitFor(() => expect(server.posts(RESUME)).toHaveLength(1));
    expect(server.posts(RESUME)[0].body).toEqual({});
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Resumed the execution"));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("for a pause by hand says the waiting tasks start now", async () => {
    new Server();
    renderDialog(execution({ paused_by_hand: true }));
    const d = await dialog();
    expect(d).toHaveTextContent(RESUME_BY_HAND);
    expect(d).not.toHaveTextContent(RESUME_NO_INPUTS);
  });

  it("shows the onResume inputs, checks the value client-side, then validates and resumes with it", async () => {
    const server = new Server();
    server.validate = { status: 200, body: answer([INT_N]) };
    renderDialog();
    const d = await dialog();
    const field = await within(d).findByLabelText(/^n/);
    await userEvent.type(field, "x");
    await userEvent.click(resumeButton(d));
    expect(await within(d).findByText(/./, { selector: "[id$='-n-error']" })).toBeInTheDocument();
    expect(server.posts(RESUME)).toHaveLength(0);
    await userEvent.clear(field);
    await userEvent.type(field, "3");
    await userEvent.click(resumeButton(d));
    await waitFor(() => expect(server.posts(RESUME)).toHaveLength(1));
    expect(server.posts(VALIDATE)[1].body).toEqual({ inputs: { n: 3 } });
    expect(server.posts(RESUME)[0].body).toEqual({ inputs: { n: 3 } });
  });

  it("shows the server's error beside the field and does not resume", async () => {
    const server = new Server();
    const message = "Invalid value for input `n`. Cause: it must be more than `5`";
    server.validate = { status: 200, body: answer([INT_N]) };
    server.checks = [{ status: 200, body: answer([outcome({ id: "n", type: "INT" }, [{ message, renderError: false }])]) }];
    renderDialog();
    const d = await dialog();
    await userEvent.type(await within(d).findByLabelText(/^n/), "3");
    await userEvent.click(resumeButton(d));
    const shown = await within(d).findByText(message);
    expect(shown.id).toMatch(/-n-error$/);
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();
    expect(server.posts(RESUME)).toHaveLength(0);
  });

  it("keeps the dialog open with the sentence for a coded refusal", async () => {
    const server = new Server();
    server.resume = { status: 400, body: { code: "resume_inputs_unexpected", error: "no onResume" } };
    const { onOpenChange } = renderDialog();
    const d = await dialog();
    await waitFor(() => expect(resumeButton(d)).toBeEnabled());
    await userEvent.click(resumeButton(d));
    expect(await within(d).findByText(resumeRefusal("resume_inputs_unexpected")!)).toBeInTheDocument();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("does not post validate again when a refusal refreshes the execution or the window regains focus", async () => {
    // Arrange
    const server = new Server();
    server.validate = { status: 200, body: answer([INT_N]) };
    server.checks = [{ status: 200, body: answer([INT_N]) }];
    server.resume = { status: 409, body: { code: "not_paused", error: "not paused" } };
    renderDialog();
    const d = await dialog();
    await userEvent.type(await within(d).findByLabelText(/^n/), "3");

    // Act
    await userEvent.click(resumeButton(d));
    await within(d).findByText(resumeRefusal("not_paused")!);
    focusManager.setFocused(false);
    focusManager.setFocused(true);
    focusManager.setFocused(undefined);

    // Assert: the opening read and the check before the resume, nothing after.
    await waitFor(() => expect(server.posts(RESUME)).toHaveLength(1));
    expect(server.posts(VALIDATE)).toHaveLength(2);
    expect(within(d).getAllByText(resumeRefusal("not_paused")!)).toHaveLength(1);
    expect(within(d).getByLabelText(/^n/)).toBeInTheDocument();
  });

  it("keeps the form when a later read of the inputs is refused", async () => {
    // Arrange
    const server = new Server();
    server.validate = { status: 200, body: answer([INT_N]) };
    server.checks = [{ status: 409, body: { code: "not_paused", error: "not paused" } }];
    const { client } = renderDialog();
    const d = await dialog();
    await within(d).findByLabelText(/^n/);

    // Act
    await client.invalidateQueries({ queryKey: workflowsKeys.resumeInputs(EXEC) });

    // Assert
    await waitFor(() => expect(server.posts(VALIDATE)).toHaveLength(2));
    expect(within(d).getByLabelText(/^n/)).toBeInTheDocument();
    expect(within(d).queryByText(resumeRefusal("not_paused")!)).toBeNull();
  });

  it("shows a skeleton and disables Resume while the inputs load", async () => {
    const server = new Server();
    let release = () => {};
    server.gate = new Promise<void>((r) => {
      release = r;
    });
    renderDialog();
    const d = await dialog();
    expect(within(d).getByText("Reading the inputs")).toBeInTheDocument();
    expect(resumeButton(d)).toBeDisabled();
    release();
    await waitFor(() => expect(resumeButton(d)).toBeEnabled());
    expect(within(d).queryByText("Reading the inputs")).toBeNull();
  });

  it("posts a FormData body when a file is chosen", async () => {
    const server = new Server();
    server.validate = { status: 200, body: answer([outcome({ id: "doc", type: "FILE" })]) };
    renderDialog();
    const d = await dialog();
    const field = await within(d).findByLabelText(/^doc/);
    await userEvent.upload(field, new File(["x"], "a.txt", { type: "text/plain" }));
    await userEvent.click(resumeButton(d));
    await waitFor(() => expect(server.posts(RESUME)).toHaveLength(1));
    expect(server.posts(RESUME)[0].body).toBeInstanceOf(FormData);
  });
});
