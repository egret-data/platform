import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

vi.mock("@/components/monaco/MonacoEditor", () => import("./monacoStub"));
vi.mock("@/components/workflows/editor/monacoYaml", () => ({ configureFlowYaml: vi.fn() }));
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import FlowDetail from "@/routes/workflows/FlowDetail";
import { FakeWorkflowServer, FLOW_ID, flowSource, WS } from "./fakeWorkflowServer";
import { pressSaveInEditor, resetMonacoStub } from "./monacoStub";

const FLOW_PATH = `/api/ws/${WS}/workflows/flows/${FLOW_ID}`;
const LONG = { timeout: 3000 };

let server: FakeWorkflowServer;

beforeEach(() => {
  resetMonacoStub();
  toast.success.mockReset();
  toast.error.mockReset();
  toast.warning.mockReset();
  localStorage.clear();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

function Where() {
  return <output data-testid="path">{useLocation().pathname}</output>;
}

function renderAt(tab = "edit", id = FLOW_ID) {
  server.install();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/explore/${WS}/workflows/flows/${id}/${tab}`]}>
        <Routes>
          <Route path="/explore/:workspace/workflows/flows/:id/:tab" element={<FlowDetail />} />
          <Route path="/explore/:workspace/workflows/flows" element={<h1>The flow list</h1>} />
        </Routes>
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const editorBox = () => screen.findByRole("textbox", { name: "Flow source" });
const type = (box: HTMLElement, text: string) => fireEvent.change(box, { target: { value: text } });
const button = (name: string) => screen.getByRole("button", { name });
const puts = () => server.callsTo("PUT", FLOW_PATH);

/** A flow whose live v1 is `live`, and whose saved draft is `draft`. */
function liveFlow(live: string, draft = live) {
  server = new FakeWorkflowServer({ source: live });
  server.publishNow();
  server.flow = { ...server.flow!, source: draft };
}

describe("FlowDetail: the draft indicator", () => {
  it("lights after a saved change, and goes out when the draft is saved back to the live text", async () => {
    const live = flowSource("live");
    liveFlow(live);
    renderAt();
    const box = await editorBox();
    expect(await screen.findByRole("heading", { level: 1, name: "hello" })).toBeInTheDocument();
    await waitFor(() => expect(server.callsTo("GET", `${FLOW_PATH}/versions`).length).toBeGreaterThan(0));
    expect(screen.queryByText(/Draft differs from live/)).not.toBeInTheDocument();

    // An unsaved edit is its own indicator, and does not light the draft pill.
    type(box, flowSource("changed"));
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
    expect(screen.queryByText(/Draft differs from live/)).not.toBeInTheDocument();

    await userEvent.click(button("Save"));
    expect(await screen.findByText("Draft differs from live v1")).toBeInTheDocument();
    expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();

    // Back to the live text, byte for byte, and saved: the content matches again.
    type(box, flowSource("live"));
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
    // Still lit: what is SAVED still differs until the save lands.
    expect(screen.getByText("Draft differs from live v1")).toBeInTheDocument();
    await userEvent.click(button("Save"));
    await waitFor(() => expect(screen.queryByText(/Draft differs from live/)).not.toBeInTheDocument());
    expect(puts()).toHaveLength(2);
  });

  it("is lit on load when the saved draft already differs, and names the live version", async () => {
    liveFlow(flowSource("live"), flowSource("draft"));
    renderAt();
    expect(await screen.findByText("Draft differs from live v1")).toBeInTheDocument();
    expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();
  });
});

describe("FlowDetail: Save", () => {
  it("sends if_updated_at exactly as the server sent it, and confirms with a toast", async () => {
    server = new FakeWorkflowServer({ updated_at: "2026-10-10T09:47:08.618779Z" });
    renderAt();
    type(await editorBox(), flowSource("two"));
    await userEvent.click(button("Save"));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(puts()[0].body).toEqual({ source: flowSource("two"), folder_id: null, if_updated_at: "2026-10-10T09:47:08.618779Z" });
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Flow saved"));
  });

  it("chains: the second save sends the updated_at the first one returned", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    const box = await editorBox();
    type(box, flowSource("two"));
    await userEvent.click(button("Save"));
    await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(1));
    const afterFirst = server.flow!.updated_at;
    type(box, flowSource("three"));
    await userEvent.click(button("Save"));
    await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(2));
    expect((puts()[1].body as { if_updated_at: string }).if_updated_at).toBe(afterFirst);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("is disabled, saying why, while nothing has changed", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    await editorBox();
    expect(button("Save")).toBeDisabled();
    expect(button("Save")).toHaveAccessibleDescription("No unsaved changes");
  });

  it("saves on Ctrl+S and on Cmd+S anywhere on the page, and on the editor's own binding, once per press", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    const box = await editorBox();
    type(box, flowSource("two"));
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await waitFor(() => expect(puts()).toHaveLength(1));
    await waitFor(() => expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument());

    type(box, flowSource("three"));
    fireEvent.keyDown(window, { key: "s", metaKey: true });
    await waitFor(() => expect(puts()).toHaveLength(2));
    await waitFor(() => expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument());

    // Monaco's binding and the page's both fire on one press in a browser: still one save.
    type(box, flowSource("four"));
    act(() => {
      pressSaveInEditor();
      fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    });
    await waitFor(() => expect(puts()).toHaveLength(3));
    await waitFor(() => expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument());
    expect(puts()).toHaveLength(3);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("on a stale save offers Overwrite, which saves again without if_updated_at", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    const box = await editorBox();
    server.saveElsewhere(flowSource("theirs"));
    type(box, flowSource("mine"));
    await userEvent.click(button("Save"));

    const dialog = await screen.findByRole("alertdialog", { name: "The draft changed since you opened it" });
    expect(box).toHaveValue(flowSource("mine"));
    await userEvent.click(within(dialog).getByRole("button", { name: "Overwrite" }));

    await waitFor(() => expect(puts()).toHaveLength(2));
    expect(puts()[1].body).toEqual({ source: flowSource("mine"), folder_id: null });
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(server.flow!.source).toBe(flowSource("mine"));
    expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();
  });

  it("on a stale save offers Reload, which asks before discarding the edits", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    const box = await editorBox();
    server.saveElsewhere(flowSource("theirs"));
    type(box, flowSource("mine"));
    await userEvent.click(button("Save"));
    const dialog = await screen.findByRole("alertdialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Reload" }));

    // The confirm: keeping the edits goes back, and nothing was replaced.
    await userEvent.click(await screen.findByRole("button", { name: "Keep my edits" }));
    expect(box).toHaveValue(flowSource("mine"));
    await userEvent.click(screen.getByRole("button", { name: "Reload" }));
    await userEvent.click(await screen.findByRole("button", { name: "Discard and reload" }));

    await waitFor(() => expect(screen.getByRole("textbox", { name: "Flow source" })).toHaveValue(flowSource("theirs")));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();

    // The reloaded draft is the new baseline: the next save is not refused.
    type(screen.getByRole("textbox", { name: "Flow source" }), flowSource("after reload"));
    await userEvent.click(button("Save"));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Flow saved"));
    expect(server.flow!.source).toBe(flowSource("after reload"));
  });

  it("shows any other refusal in the server's words and keeps the text", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    const box = await editorBox();
    type(box, flowSource("two"));
    server.flow = null; // the PUT now answers 404
    await userEvent.click(button("Save"));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("not found: flow"));
    expect(box).toHaveValue(flowSource("two"));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });
});

describe("FlowDetail: Publish", () => {
  it("is disabled with \"Save first\" while there are unsaved edits", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    await waitFor(() => expect(button("Publish")).toBeEnabled(), LONG);
    type(await editorBox(), flowSource("two"));
    expect(button("Publish")).toBeDisabled();
    expect(button("Publish")).toHaveAccessibleDescription("Save first");
  });

  it("is disabled with the error count while validation reports errors", async () => {
    server = new FakeWorkflowServer();
    server.validation = {
      status: 200,
      body: {
        errors: [
          { code: "a", message: "first", path: "tasks" },
          { code: "b", message: "second", path: "id" },
        ],
        warnings: [{ code: "w", message: "a warning", path: "namespace" }],
      },
    };
    renderAt();
    await waitFor(() => expect(button("Publish")).toHaveAccessibleDescription("Fix 2 errors to publish"), LONG);
    expect(button("Publish")).toBeDisabled();
    // The same count the Problems tab carries.
    expect(screen.getByRole("tab", { name: /Problems, 2 errors/ })).toBeInTheDocument();
  });

  it("is disabled with \"Validation unavailable\" when validation fails, never read as no errors", async () => {
    server = new FakeWorkflowServer();
    server.validation = { status: 500, body: { error: "internal error" } };
    renderAt();
    await waitFor(() => expect(button("Publish")).toHaveAccessibleDescription("Validation unavailable"), LONG);
    expect(button("Publish")).toBeDisabled();
  });

  it("sends the note, refreshes the versions, and re-reads the draft so the next save is not refused", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    await waitFor(() => expect(button("Publish")).toBeEnabled(), LONG);
    const versionReads = server.callsTo("GET", `${FLOW_PATH}/versions`).length;
    await userEvent.click(button("Publish"));

    const dialog = await screen.findByRole("dialog", { name: "Publish company.team.hello" });
    await userEvent.type(within(dialog).getByLabelText("Note (optional)"), "First release");
    await userEvent.click(within(dialog).getByRole("button", { name: "Publish" }));

    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Published v1"));
    expect(server.callsTo("POST", `${FLOW_PATH}/publish`)[0].body).toEqual({ note: "First release" });
    await waitFor(() => expect(server.callsTo("GET", `${FLOW_PATH}/versions`).length).toBeGreaterThan(versionReads));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(await screen.findByText("Live")).toBeInTheDocument();
    const movedByPublish = server.flow!.updated_at;
    expect(movedByPublish).not.toBe("2026-10-10T09:47:08.618779Z");

    // Publishing moved `updated_at`; the editor re-read it, so this save goes through.
    type(screen.getByRole("textbox", { name: "Flow source" }), flowSource("after publish"));
    await userEvent.click(button("Save"));
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Flow saved"));
    expect((puts()[0].body as { if_updated_at: string }).if_updated_at).toBe(movedByPublish);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(server.flow!.source).toBe(flowSource("after publish"));
  });

  it("keeps the note and shows the server's message when publishing fails", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    await waitFor(() => expect(button("Publish")).toBeEnabled(), LONG);
    await userEvent.click(button("Publish"));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Note (optional)"), "Keep me");
    server.flow = null; // publish now answers 404
    await userEvent.click(within(dialog).getByRole("button", { name: "Publish" }));
    expect(await within(dialog).findByText("not found: flow")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Note (optional)")).toHaveValue("Keep me");
  });
});

describe("FlowDetail: Run", () => {
  it("is disabled until a version is live", async () => {
    server = new FakeWorkflowServer({ live_version_id: null });
    renderAt();
    await editorBox();
    expect(button("Run")).toBeDisabled();
    expect(button("Run")).toHaveAccessibleDescription("Publish the flow before running it");
  });

  it("is enabled once a version is live", async () => {
    liveFlow(flowSource("live"));
    renderAt();
    await editorBox();
    expect(button("Run")).toBeEnabled();
  });
});

describe("FlowDetail: Delete", () => {
  it("asks through the typed confirm, deletes, and goes to the list", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    await editorBox();
    await userEvent.click(button("Delete"));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete company.team.hello?" });
    expect(server.callsTo("DELETE", FLOW_PATH)).toHaveLength(0);
    await userEvent.type(within(dialog).getByRole("textbox"), "delete");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.getByTestId("path")).toHaveTextContent(`/explore/${WS}/workflows/flows`));
    expect(screen.getByTestId("path").textContent).toBe(`/explore/${WS}/workflows/flows`);
    expect(server.callsTo("DELETE", FLOW_PATH)).toHaveLength(1);
    expect(toast.success).not.toHaveBeenCalled();
  });
});

describe("FlowDetail: a read-only viewer", () => {
  it("sees no Save, Publish, Run or Delete, and the banner saying why", async () => {
    server = new FakeWorkflowServer({ can_manage: false });
    renderAt();
    const box = await editorBox();
    expect(box).toHaveAttribute("readonly");
    expect(screen.getByRole("note")).toHaveTextContent("Read only. Editing a flow needs the workflow:manage permission.");
    for (const name of ["Save", "Publish", "Run", "Delete"]) {
      expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
    }
    // Ctrl+S does nothing, and the server is never asked to validate.
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await new Promise((r) => setTimeout(r, 700));
    expect(puts()).toHaveLength(0);
    expect(server.callsTo("POST", `${FLOW_PATH}/validate`)).toHaveLength(0);
  });
});

describe("FlowDetail: routing and states", () => {
  it("says the flow was not found, with a link back to the list", async () => {
    server = new FakeWorkflowServer(null);
    renderAt();
    expect(await screen.findByRole("heading", { level: 1, name: "Flow not found" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to flows" })).toHaveAttribute("href", `/explore/${WS}/workflows/flows`);
    expect(screen.queryByRole("textbox", { name: "Flow source" })).not.toBeInTheDocument();
  });

  it("shows a failed load with its message and a retry, never as not found", async () => {
    server = new FakeWorkflowServer();
    server.getFlowReply = { status: 503, body: { error: "the database is unavailable" } };
    renderAt();
    expect(await screen.findByText(/the database is unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("Flow not found")).not.toBeInTheDocument();
    server.getFlowReply = null;
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await editorBox()).toHaveValue(flowSource("one"));
  });

  it("sends an unknown tab to edit", async () => {
    server = new FakeWorkflowServer();
    renderAt("nonsense");
    await waitFor(() => expect(screen.getByTestId("path")).toHaveTextContent(`/flows/${FLOW_ID}/edit`));
    expect(await editorBox()).toBeInTheDocument();
  });

  it("keeps the tab in the URL, and keeps unsaved edits across a tab switch", async () => {
    server = new FakeWorkflowServer();
    renderAt();
    type(await editorBox(), flowSource("unsaved"));
    // A press, not userEvent's click: in jsdom react-resizable-panels cancels every pointer
    // press while the panels are mounted (zero rects put it on a gutter); Radix tabs select on press.
    fireEvent.mouseDown(screen.getByRole("tab", { name: "Revisions" }), { button: 0 });
    expect(screen.getByTestId("path")).toHaveTextContent(`/flows/${FLOW_ID}/revisions`);
    expect(screen.queryByRole("textbox", { name: "Flow source" })).not.toBeInTheDocument();
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: "Edit" }));
    expect(screen.getByTestId("path")).toHaveTextContent(`/flows/${FLOW_ID}/edit`);
    expect(await editorBox()).toHaveValue(flowSource("unsaved"));
  });
});
