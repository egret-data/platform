import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { EvalPanel } from "@/components/workflows/executions/EvalPanel";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function setup(taskRunId?: string) {
  const qc = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <EvalPanel ws="acme" executionId="e1" taskRunId={taskRunId} />
    </QueryClientProvider>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe("EvalPanel", () => {
  it("posts the raw expression as text/plain to the execution's eval and shows the result", async () => {
    const fetchMock = vi.fn().mockResolvedValue(json({ result: "42", error: null }));
    vi.stubGlobal("fetch", fetchMock);
    setup();
    await userEvent.type(screen.getByLabelText("Expression"), "{{ 6 * 7 }}".replace(/[{]/g, "{{"));
    await userEvent.click(screen.getByRole("button", { name: "Evaluate" }));
    expect(await screen.findByTestId("eval-result")).toHaveTextContent("42");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/ws/acme/workflows/executions/e1/eval");
    expect(init.method).toBe("POST");
    expect(new Headers(init.headers).get("Content-Type")).toBe("text/plain");
    expect(init.body).toBe("{{ 6 * 7 }}");
  });

  it("posts to the task run's eval when given a task run", async () => {
    const fetchMock = vi.fn().mockResolvedValue(json({ result: "x", error: null }));
    vi.stubGlobal("fetch", fetchMock);
    setup("r9");
    await userEvent.type(screen.getByLabelText("Expression"), "abc");
    await userEvent.click(screen.getByRole("button", { name: "Evaluate" }));
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(fetchMock.mock.calls[0][0]).toBe("/api/ws/acme/workflows/executions/e1/task-runs/r9/eval");
  });

  it("shows the evaluation error, not a result", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(json({ result: null, error: "unknown variable" })));
    setup();
    await userEvent.type(screen.getByLabelText("Expression"), "abc");
    await userEvent.click(screen.getByRole("button", { name: "Evaluate" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("unknown variable");
    expect(screen.queryByTestId("eval-result")).toBeNull();
  });

  it("disables Evaluate while the expression is empty, and says why", () => {
    setup();
    expect(screen.getByRole("button", { name: "Evaluate" })).toBeDisabled();
    expect(screen.getByText("Type an expression to evaluate.")).toBeInTheDocument();
  });

  it("shows the server's message when the request fails", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(json({ error: "forbidden here" }, 403)));
    setup();
    await userEvent.type(screen.getByLabelText("Expression"), "abc");
    await userEvent.click(screen.getByRole("button", { name: "Evaluate" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("forbidden here");
  });
});
