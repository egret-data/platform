import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation, useNavigationType } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

vi.mock("@/components/monaco/MonacoEditor", () => import("./monacoStub"));
vi.mock("@/components/workflows/editor/monacoYaml", () => ({ configureFlowYaml: vi.fn() }));
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import WorkflowsRoutes from "@/routes/workflows/WorkflowsRoutes";
import { NEW_FLOW_TEMPLATE } from "@/routes/workflows/FlowNew";
import { LAYOUT_KEY_PREFIX } from "@/components/workflows/panels/usePersistedLayout";
import { FakeWorkflowServer, FLOW_ID, flowSource, WS } from "./fakeWorkflowServer";
import { resetMonacoStub } from "./monacoStub";

const LONG = { timeout: 3000 };
const BASE = `/api/ws/${WS}/workflows`;
let server: FakeWorkflowServer;

beforeEach(() => {
  resetMonacoStub();
  toast.success.mockReset();
  localStorage.clear();
  server = new FakeWorkflowServer(null);
});
afterEach(() => {
  vi.unstubAllGlobals();
});

function Where() {
  return <output data-testid="path" data-nav={useNavigationType()}>{useLocation().pathname}</output>;
}

function renderNew() {
  server.install();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/explore/${WS}/workflows/flows/new`]}>
        <Routes>
          <Route path="/explore/:workspace/workflows/*" element={<WorkflowsRoutes />} />
        </Routes>
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const editorBox = () => screen.findByRole("textbox", { name: "Flow source" });

describe("FlowNew", () => {
  it("opens the editor on the template, validated through the id-less route", async () => {
    renderNew();
    expect(await screen.findByRole("heading", { level: 1, name: "New flow" })).toBeInTheDocument();
    expect(await editorBox()).toHaveValue(NEW_FLOW_TEMPLATE);
    await waitFor(() => expect(server.callsTo("POST", `${BASE}/flows/validate`)).toHaveLength(1), LONG);
    expect(server.callsTo("POST", `${BASE}/flows/validate`)[0].body).toEqual({ source: NEW_FLOW_TEMPLATE });
    expect(server.calls.some((c) => /\/flows\/[^/]+\/validate$/.test(c.path))).toBe(false);
  });

  it("creates on the first Save, replaces the URL with the flow's editor, and keeps the text and layout", async () => {
    renderNew();
    const box = await editorBox();
    fireEvent.change(box, { target: { value: flowSource("mine") } });
    // A layout change on the create page, kept in memory until the flow exists.
    screen.getByRole("button", { name: "Tab actions: Docs" }).focus();
    await userEvent.keyboard("{Enter}");
    await userEvent.click(await screen.findByRole("menuitem", { name: "Split right" }));
    // One "Tab actions" menu per panel.
    const panels = () => screen.getAllByRole("button", { name: /^Tab actions:/ });
    await waitFor(() => expect(panels()).toHaveLength(3));

    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(screen.getByTestId("path")).toHaveTextContent(`/explore/${WS}/workflows/flows/${FLOW_ID}/edit`));
    expect(screen.getByTestId("path")).toHaveAttribute("data-nav", "REPLACE");
    expect(server.callsTo("POST", `${BASE}/flows`)[0].body).toEqual({ source: flowSource("mine") });
    expect(await screen.findByRole("heading", { level: 1, name: "hello" })).toBeInTheDocument();
    expect(await editorBox()).toHaveValue(flowSource("mine"));
    expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();
    // The split survived the move to the flow's own editor.
    expect(panels()).toHaveLength(3);
    expect(localStorage.getItem(`${LAYOUT_KEY_PREFIX}${FLOW_ID}`)).toContain("\"panels\"");
    // Navigating to the result is the confirmation; no toast (FE-ACTION-2).
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("shows a 409 in the server's words beside the editor, and keeps the text", async () => {
    server.createReply = {
      status: 409,
      body: { error: "conflict: a flow already exists at company.team/hello in this workspace" },
    };
    renderNew();
    const box = await editorBox();
    fireEvent.change(box, { target: { value: flowSource("taken") } });
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByText("conflict: a flow already exists at company.team/hello in this workspace")).toBeInTheDocument();
    expect(screen.getByText("The flow was not created")).toBeInTheDocument();
    expect(screen.getByTestId("path")).toHaveTextContent(`/explore/${WS}/workflows/flows/new`);
    expect(screen.getByRole("textbox", { name: "Flow source" })).toHaveValue(flowSource("taken"));
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
  });

  it("saves on Ctrl+S", async () => {
    renderNew();
    await editorBox();
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await waitFor(() => expect(server.callsTo("POST", `${BASE}/flows`)).toHaveLength(1));
    expect(server.callsTo("POST", `${BASE}/flows`)[0].body).toEqual({ source: NEW_FLOW_TEMPLATE });
  });

  it("keeps text typed while the create request is in flight, as unsaved edits of the new flow", async () => {
    let release!: () => void;
    server.createGate = new Promise<void>((r) => { release = r; });
    renderNew();
    const box = await editorBox();
    fireEvent.change(box, { target: { value: flowSource("sent") } });
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(server.callsTo("POST", `${BASE}/flows`)).toHaveLength(1));
    // Typed after the request left.
    fireEvent.change(box, { target: { value: flowSource("typed during save") } });
    release();

    await waitFor(() => expect(screen.getByTestId("path")).toHaveTextContent(`/flows/${FLOW_ID}/edit`));
    expect(await screen.findByRole("heading", { level: 1, name: "hello" })).toBeInTheDocument();
    expect(await editorBox()).toHaveValue(flowSource("typed during save"));
    expect(server.flow!.source).toBe(flowSource("sent"));
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
  });

  it("turns read-only, with a banner and no Save, when the server says the caller cannot manage flows", async () => {
    server.validation = { status: 403, body: { error: "forbidden" } };
    renderNew();
    expect(await screen.findByText("You need permission to manage flows", {}, LONG)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(await editorBox()).toHaveAttribute("readonly");
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    // Latched: read-only stops validation, and the page stays as it is.
    await new Promise((r) => setTimeout(r, 700));
    expect(server.callsTo("POST", `${BASE}/flows`)).toHaveLength(0);
    expect(server.callsTo("POST", `${BASE}/flows/validate`)).toHaveLength(1);
    expect(screen.getByText("You need permission to manage flows")).toBeInTheDocument();
  });

  it("turns read-only on a 403 from Save too", async () => {
    server.createReply = { status: 403, body: { error: "forbidden" } };
    renderNew();
    await editorBox();
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("You need permission to manage flows")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Flow source" })).toHaveValue(NEW_FLOW_TEMPLATE);
  });
});
