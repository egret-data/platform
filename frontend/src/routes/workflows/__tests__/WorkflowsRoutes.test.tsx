import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

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
  render(
    <QueryClientProvider client={new QueryClient()}>
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
    // wait failed on time alone.
    await waitFor(() => expect(api.useWorkflowGetFlow).toHaveBeenCalledWith("eng", "f-1", expect.anything()), { timeout: 10000 });
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

  it("holds the Executions entry with a page that says the list is not here yet", () => {
    renderAt("/explore/eng/workflows/executions");
    expect(screen.getByRole("heading", { level: 1, name: "Executions" })).toBeInTheDocument();
    expect(screen.getByText("The executions list is not here yet")).toBeInTheDocument();
  });
});
