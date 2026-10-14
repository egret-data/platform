import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("@/api/generated/workflow/workflow");
// The flow pages mount the editor; real Monaco cannot load in jsdom.
vi.mock("@/components/monaco/MonacoEditor", () => import("./monacoStub"));

import * as api from "@/api/generated/workflow/workflow";
import WorkflowsRoutes from "@/routes/workflows/WorkflowsRoutes";

function Where() {
  return <output data-testid="path">{useLocation().pathname}</output>;
}

function renderAt(path: string) {
  vi.mocked(api.useWorkflowListFlows).mockReturnValue({
    data: { status: 200, data: [] },
    isLoading: false,
    isError: false,
    error: null,
  } as unknown as ReturnType<typeof api.useWorkflowListFlows>);
  // The flow pages' reads, held loading: this file checks which page a URL reaches.
  const loading = { data: undefined, isLoading: true, isError: false, error: null, refetch: vi.fn() };
  vi.mocked(api.useWorkflowGetFlow).mockReturnValue(loading as unknown as ReturnType<typeof api.useWorkflowGetFlow>);
  vi.mocked(api.useWorkflowGetFlowSchema).mockReturnValue(loading as unknown as ReturnType<typeof api.useWorkflowGetFlowSchema>);
  vi.mocked(api.workflowValidateUnsavedFlow).mockReturnValue(new Promise(() => {}));
  vi.mocked(api.workflowListExecutions).mockReturnValue(new Promise(() => {}));
  vi.mocked(api.useWorkflowGetExecution).mockReturnValue(loading as unknown as ReturnType<typeof api.useWorkflowGetExecution>);
  vi.mocked(api.useWorkflowListTriggers).mockReturnValue(loading as unknown as ReturnType<typeof api.useWorkflowListTriggers>);
  vi.mocked(api.useWorkflowListNamespaces).mockReturnValue(loading as unknown as ReturnType<typeof api.useWorkflowListNamespaces>);
  render(
    <QueryClientProvider client={new QueryClient()}>
      <TooltipProvider>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route
            path="/explore/:workspace/workflows/*"
            element={
              <>
                <WorkflowsRoutes />
                <Where />
              </>
            }
          />
        </Routes>
      </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("WorkflowsRoutes", () => {
  it("sends the area's root to the flow list", () => {
    renderAt("/explore/eng/workflows");
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/flows");
    expect(screen.getByRole("heading", { level: 1, name: "Flows" })).toBeInTheDocument();
  });

  it("sends a bare flow to its edit tab", () => {
    renderAt("/explore/eng/workflows/flows/f-1");
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/flows/f-1/edit");
  });

  it("keeps a known tab, and opens the flow page there", async () => {
    renderAt("/explore/eng/workflows/flows/f-1/revisions");
    // The flow page is a lazy chunk that vitest transforms on first import, cold: measured at
    // 1.0–1.7 s on a loaded machine, at this commit and the one before it, so the default 1 s
    // wait failed on time alone. 4 s stays inside vitest's 5 s test timeout, which would
    // otherwise be the real bound.
    await waitFor(() => expect(api.useWorkflowGetFlow).toHaveBeenCalledWith("eng", "f-1", expect.anything()), { timeout: 4000 });
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/flows/f-1/revisions");
  });

  it("sends an unknown tab to edit", async () => {
    renderAt("/explore/eng/workflows/flows/f-1/nonsense");
    await waitFor(() => expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/flows/f-1/edit"));
  });

  it("matches flows/new as the create page, not as a flow id", async () => {
    renderAt("/explore/eng/workflows/flows/new");
    expect(await screen.findByRole("heading", { level: 1, name: "New flow" })).toBeInTheDocument();
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/flows/new");
    expect(api.useWorkflowGetFlow).not.toHaveBeenCalledWith("eng", "new", expect.anything());
  });

  it("opens the executions list at executions, root executions only", async () => {
    renderAt("/explore/eng/workflows/executions");
    expect(screen.getByRole("heading", { level: 1, name: "Executions" })).toBeInTheDocument();
    await waitFor(() =>
      expect(api.workflowListExecutions).toHaveBeenCalledWith("eng", expect.objectContaining({ root_only: true }), expect.anything()),
    );
    expect(screen.queryByText("The executions list is not here yet")).toBeNull();
  });

  it("sends a bare execution to its overview tab", async () => {
    renderAt("/explore/eng/workflows/executions/e-1");
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/executions/e-1/overview");
    // The execution page is a lazy chunk, cold on first import (see "keeps a known tab" above).
    await waitFor(() => expect(api.useWorkflowGetExecution).toHaveBeenCalledWith("eng", "e-1", expect.anything()), { timeout: 4000 });
  });

  it.each(["gantt", "logs", "outputs"])("keeps the execution's %s tab", async (tab) => {
    renderAt(`/explore/eng/workflows/executions/e-1/${tab}`);
    await waitFor(() => expect(api.useWorkflowGetExecution).toHaveBeenCalledWith("eng", "e-1", expect.anything()), { timeout: 4000 });
    expect(screen.getByTestId("path")).toHaveTextContent(`/explore/eng/workflows/executions/e-1/${tab}`);
  });

  it("sends an unknown execution tab to overview", async () => {
    renderAt("/explore/eng/workflows/executions/e-1/metrics");
    await waitFor(() => expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/executions/e-1/overview"), {
      timeout: 4000,
    });
  });

  it("keeps the flow's Executions tab", async () => {
    renderAt("/explore/eng/workflows/flows/f-1/executions");
    await waitFor(() => expect(api.useWorkflowGetFlow).toHaveBeenCalledWith("eng", "f-1", expect.anything()), { timeout: 4000 });
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/flows/f-1/executions");
  });

  it("opens the workspace's Triggers page at triggers (W7e)", async () => {
    renderAt("/explore/eng/workflows/triggers");
    expect(screen.getByRole("heading", { level: 1, name: "Triggers" })).toBeInTheDocument();
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/triggers");
    await waitFor(() =>
      expect(api.useWorkflowListTriggers).toHaveBeenCalledWith("eng", expect.objectContaining({ limit: 50, offset: 0 }), expect.anything()),
    );
  });
});
