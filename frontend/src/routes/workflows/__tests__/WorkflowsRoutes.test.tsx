import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";

vi.mock("@/api/generated/workflow/workflow");
vi.mock("@/api/members");

import * as api from "@/api/generated/workflow/workflow";
import * as members from "@/api/members";
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
  vi.mocked(members.useWorkspaceMembers).mockReturnValue({ data: [] } as unknown as ReturnType<
    typeof members.useWorkspaceMembers
  >);
  render(
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
    </MemoryRouter>,
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

  it("keeps a known tab", () => {
    renderAt("/explore/eng/workflows/flows/f-1/revisions");
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/flows/f-1/revisions");
  });

  it("sends an unknown tab to edit", () => {
    renderAt("/explore/eng/workflows/flows/f-1/nonsense");
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/flows/f-1/edit");
  });

  it("matches flows/new as the create page, not as a flow id", () => {
    renderAt("/explore/eng/workflows/flows/new");
    expect(screen.getByTestId("path")).toHaveTextContent("/explore/eng/workflows/flows/new");
    expect(screen.getByRole("heading", { level: 1, name: "New flow" })).toBeInTheDocument();
  });

  it("holds the Executions entry with a page that says the list is not here yet", () => {
    renderAt("/explore/eng/workflows/executions");
    expect(screen.getByRole("heading", { level: 1, name: "Executions" })).toBeInTheDocument();
    expect(screen.getByText("The executions list is not here yet")).toBeInTheDocument();
  });
});
