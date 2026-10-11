import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RunFlow } from "@/components/workflows/run/RunFlow";

const WS = "eng";
const FLOW = "0a7c1d2e-0000-4000-8000-000000000001";
const EXEC = "5e5e5e5e-0000-4000-8000-0000000000aa";
const START = `/api/ws/${WS}/workflows/flows/${FLOW}/executions`;

type Call = { method: string; path: string; body: unknown; contentType: string | null };

/** `POST …/executions` answers `reply`; every request is recorded with its body (JSON parsed, FormData kept). */
function serve(reply: { status: number; body: unknown } = { status: 201, body: execution() }) {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
    const method = (init?.method ?? "GET").toUpperCase();
    const body = typeof init?.body === "string" ? JSON.parse(init.body) : init?.body;
    calls.push({ method, path: String(input), body, contentType: new Headers(init?.headers).get("content-type") });
    return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
  }));
  return calls;
}

function execution() {
  return {
    id: EXEC,
    flow_id: FLOW,
    namespace: "company.team",
    flow_key: "hello",
    state: "CREATED",
    history: [],
    inputs: {},
    seq: 1,
    execution_trigger: {},
    attempt_number: 1,
    origin_offset_ms: 0,
    created_at: "2026-10-10T10:00:00.000001Z",
    updated_at: "2026-10-10T10:00:00.000001Z",
    source: "",
    runs: [],
  };
}

function flowWith(inputs: string): string {
  return `id: hello\nnamespace: company.team\n${inputs ? `inputs:\n${inputs}\n` : ""}tasks:\n  - id: t\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n`;
}

function Where() {
  return <output data-testid="path">{useLocation().pathname}</output>;
}

function renderRun(source: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const onOpenChange = vi.fn();
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/flow"]}>
        <Routes>
          <Route
            path="*"
            element={
              <RunFlow
                ws={WS}
                flowId={FLOW}
                flowName="company.team.hello"
                source={source}
                open
                onOpenChange={onOpenChange}
              />
            }
          />
        </Routes>
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { client, onOpenChange };
}

const runButton = () => screen.getByRole("button", { name: "Run" });
const starts = (calls: Call[]) => calls.filter((c) => c.method === "POST" && c.path === START);

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("RunFlow: the container", () => {
  it("asks to confirm a flow with no inputs, then starts it with no inputs and opens the execution", async () => {
    const calls = serve();
    renderRun(flowWith(""));
    const confirm = await screen.findByRole("alertdialog", { name: "Run company.team.hello?" });
    expect(within(confirm).getByText(/saved draft/)).toBeInTheDocument();
    expect(starts(calls)).toHaveLength(0);
    await userEvent.click(within(confirm).getByRole("button", { name: "Run" }));
    await waitFor(() => expect(screen.getByTestId("path")).toHaveTextContent(`/explore/${WS}/workflows/executions/${EXEC}`));
    expect(starts(calls)).toHaveLength(1);
    expect(starts(calls)[0].body).toEqual({ inputs: {} });
    // Said, not left to `fetch`: a string body would go out as `text/plain`, which the route refuses (415).
    expect(starts(calls)[0].contentType).toBe("application/json");
  });

  it("opens a dialog for at most three scalar inputs", async () => {
    serve();
    renderRun(flowWith("  - id: a\n    type: STRING\n  - id: b\n    type: INT\n  - id: c\n    type: BOOL"));
    const dialog = await screen.findByRole("dialog", { name: "Run company.team.hello" });
    expect(dialog).toHaveAttribute("data-container", "dialog");
  });

  it("opens a drawer for a fourth scalar input", async () => {
    serve();
    renderRun(flowWith("  - id: a\n    type: STRING\n  - id: b\n    type: INT\n  - id: c\n    type: BOOL\n  - id: d\n    type: DATE"));
    expect(await screen.findByRole("dialog", { name: "Run company.team.hello" })).toHaveAttribute("data-container", "drawer");
  });

  it("opens a drawer for a single input that is not scalar", async () => {
    serve();
    renderRun(flowWith("  - id: j\n    type: JSON"));
    expect(await screen.findByRole("dialog", { name: "Run company.team.hello" })).toHaveAttribute("data-container", "drawer");
  });
});

const EVERY_TYPE = [
  "  - id: s",
  "    type: STRING",
  "    displayName: Name",
  "  - id: e",
  "    type: EMAIL",
  "  - id: u",
  "    type: URI",
  "  - id: n",
  "    type: INT",
  "  - id: fl",
  "    type: FLOAT",
  "  - id: b",
  "    type: BOOL",
  "  - id: dt",
  "    type: DATETIME",
  "  - id: d",
  "    type: DATE",
  "  - id: t",
  "    type: TIME",
  "  - id: du",
  "    type: DURATION",
  "  - id: sel",
  "    type: SELECT",
  "    values: [eu, us]",
  "  - id: ms",
  "    type: MULTISELECT",
  "    values: [x, y]",
  "  - id: arr",
  "    type: ARRAY",
  "    itemType: STRING",
  "    defaults: [one]",
  "  - id: j",
  "    type: JSON",
  "  - id: y",
  "    type: YAML",
  "  - id: sec",
  "    type: SECRET",
  "  - id: f",
  "    type: FILE",
].join("\n");

describe("RunFlow: the controls", () => {
  it("gives every control a label tied to it by htmlFor", async () => {
    serve();
    renderRun(flowWith(EVERY_TYPE));
    const dialog = await screen.findByRole("dialog");
    // Radix adds hidden native inputs inside a form for its switch, checkbox and select; they are
    // `aria-hidden` and take no focus, so they are not controls a person labels or reaches.
    const controls = [
      ...dialog.querySelectorAll<HTMLElement>("input, textarea, select, [role=switch], [role=combobox], [role=checkbox]"),
    ].filter((el) => !el.closest("[aria-hidden=true]"));
    // One per scalar input, two MULTISELECT boxes and one ARRAY row: a count that misses a type fails.
    expect(controls.length).toBe(17 - 2 + 2 + 1);
    for (const control of controls) {
      expect(control.id, control.outerHTML).not.toBe("");
      const label = dialog.querySelector(`label[for="${control.id}"]`);
      expect(label, `no label for ${control.outerHTML}`).not.toBeNull();
    }
    // The label's text is the input's displayName, or its id without one.
    expect(screen.getByLabelText(/^Name/)).toHaveProperty("type", "text");
    expect(screen.getByLabelText(/^sec/)).toHaveAttribute("type", "password");
    expect(screen.getByRole("switch", { name: /^b/ })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: /^sel/ })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "x" })).toBeInTheDocument();
    expect(screen.getByLabelText("arr item 1")).toHaveValue("one");
  });

  it("blocks a required, empty input with a message beside it and sends nothing", async () => {
    const calls = serve();
    renderRun(flowWith("  - id: who\n    type: STRING\n  - id: n\n    type: INT\n    required: false"));
    await screen.findByRole("dialog");
    await userEvent.click(runButton());
    expect(within(screen.getByTestId("run-input-who")).getByText("Enter a value for this input.")).toBeInTheDocument();
    expect(screen.getByLabelText(/^who/)).toHaveAccessibleDescription("Enter a value for this input.");
    expect(starts(calls)).toHaveLength(0);
  });

  it("sends each value in its type's shape", async () => {
    const calls = serve();
    renderRun(flowWith("  - id: n\n    type: INT\n  - id: b\n    type: BOOL\n  - id: sel\n    type: SELECT\n    values: [eu, us]"));
    await screen.findByRole("dialog");
    fireEvent.change(screen.getByLabelText(/^n/), { target: { value: "12" } });
    await userEvent.click(screen.getByRole("switch", { name: /^b/ }));
    await userEvent.click(screen.getByRole("combobox", { name: /^sel/ }));
    await userEvent.click(await screen.findByRole("option", { name: "us" }));
    await userEvent.click(runButton());
    await waitFor(() => expect(starts(calls)).toHaveLength(1));
    expect(starts(calls)[0].body).toEqual({ inputs: { n: 12, b: true, sel: "us" } });
    expect(starts(calls)[0].contentType).toBe("application/json");
  });
});

describe("RunFlow: submitting", () => {
  it("shows a server error beside the input it names, keeps every value, and stays open", async () => {
    const message = 'Invalid value for input `f`: "rows.exe": File type not allowed. Accepted extensions: .csv';
    const calls = serve({ status: 400, body: { error: message } });
    renderRun(flowWith("  - id: who\n    type: STRING\n  - id: f\n    type: FILE"));
    await screen.findByRole("dialog");
    fireEvent.change(screen.getByLabelText(/^who/), { target: { value: "ada" } });
    const file = new File(["x"], "rows.exe");
    await userEvent.upload(screen.getByLabelText(/^f/), file);
    await userEvent.click(runButton());

    expect(await within(screen.getByTestId("run-input-f")).findByText(message)).toBeInTheDocument();
    expect(within(screen.getByTestId("run-input-who")).queryByText(message)).not.toBeInTheDocument();
    // The request was multipart, the file under its input id.
    const body = starts(calls)[0].body;
    expect(body).toBeInstanceOf(FormData);
    expect(((body as FormData).get("f") as File).name).toBe("rows.exe");
    expect((body as FormData).get("who")).toBe("ada");
    // No type of ours: the browser writes `multipart/form-data; boundary=…` for a FormData body.
    expect(starts(calls)[0].contentType).toBeNull();
    // Nothing typed is cleared, and the form is still open.
    expect(screen.getByLabelText(/^who/)).toHaveValue("ada");
    expect(screen.getByText("Chosen: rows.exe")).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByTestId("path")).toHaveTextContent("/flow");
  });

  it("shows a refusal naming no input above the form, in the server's words", async () => {
    serve({ status: 409, body: { error: "Cannot execute flow: flow is disabled." } });
    renderRun(flowWith("  - id: who\n    type: STRING"));
    await screen.findByRole("dialog");
    fireEvent.change(screen.getByLabelText(/^who/), { target: { value: "ada" } });
    await userEvent.click(runButton());
    expect(await screen.findByText(/The run could not be started: Cannot execute flow: flow is disabled\./)).toBeInTheDocument();
    expect(screen.getByLabelText(/^who/)).toHaveValue("ada");
  });

  it("navigates to the execution on success", async () => {
    serve();
    renderRun(flowWith("  - id: who\n    type: STRING\n    defaults: ada"));
    await screen.findByRole("dialog");
    await userEvent.click(runButton());
    await waitFor(() => expect(screen.getByTestId("path")).toHaveTextContent(`/explore/${WS}/workflows/executions/${EXEC}`));
  });
});

describe("RunFlow: what it says", () => {
  it("says it runs the saved draft, and names no version", async () => {
    serve();
    renderRun(flowWith("  - id: who\n    type: STRING"));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Starts an execution of the saved draft.")).toBeInTheDocument();
    expect(within(dialog).queryByText(/\bv\d+\b|live|publish/i)).not.toBeInTheDocument();
  });

  it("reports a draft whose inputs cannot be read, and disables Run", async () => {
    serve();
    renderRun("id: hello\ninputs: [unclosed\n");
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText(/The flow's inputs could not be read: .*Fix the YAML, save, then run it\./)).toBeInTheDocument();
    expect(runButton()).toBeDisabled();
    expect(runButton()).toHaveAccessibleDescription("The flow's inputs could not be read");
  });

  it("disables Run for an unsupported input, naming it and its type", async () => {
    const calls = serve();
    renderRun(flowWith("  - id: who\n    type: STRING\n  - id: grp\n    type: FORM"));
    const dialog = await screen.findByRole("dialog");
    expect(runButton()).toBeDisabled();
    expect(runButton()).toHaveAccessibleDescription("grp (FORM) cannot be entered in this form");
    // The reason is visible text, not only a description (FE-ACTION-4).
    expect(within(dialog).getByText("grp (FORM) cannot be entered in this form")).toBeVisible();
    expect(within(dialog).getByRole("listitem")).toHaveTextContent("grp (FORM)");
    fireEvent.submit(dialog.querySelector("form")!);
    expect(starts(calls)).toHaveLength(0);
  });

  it("notes an input with dependsOn, whose values are not re-rendered", async () => {
    serve();
    renderRun(flowWith("  - id: a\n    type: STRING\n  - id: b\n    type: SELECT\n    values: [x]\n    dependsOn:\n      inputs: [a]"));
    await screen.findByRole("dialog");
    expect(within(screen.getByTestId("run-input-b")).getByText(/depends on other inputs/)).toBeInTheDocument();
  });
});
