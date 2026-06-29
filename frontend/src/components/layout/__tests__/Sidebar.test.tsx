/**
 * Sidebar zone tests
 *
 * Verifies the workspace sidebar renders the four IA zones (Explore /
 * Data Engineering / Data Governance / Manage) with their items at the right
 * `/explore/:ws/...` hrefs, and the Control Plane group at `/control-plane/...`.
 *
 * Sidebar requires:
 *   - MemoryRouter (uses useLocation + useParams)
 *   - TooltipProvider (renders Radix Tooltip in collapsed mode)
 *   - collapsed: boolean + onToggle: () => void props
 */
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, test, vi } from "vitest";
import { Sidebar } from "@/components/layout/Sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";

function renderSidebar(initialPath: string) {
  // Mount Sidebar under routes that expose the :workspace param (mirroring the
  // app), so its `base` resolves to /explore/:ws or /control-plane.
  const sidebar = <Sidebar collapsed={false} onToggle={vi.fn()} />;
  return render(
    <TooltipProvider>
      <MemoryRouter initialEntries={[initialPath]}>
        <Routes>
          <Route path="/explore/:workspace/*" element={sidebar} />
          <Route path="*" element={sidebar} />
        </Routes>
      </MemoryRouter>
    </TooltipProvider>,
  );
}

describe("Sidebar – workspace zones", () => {
  test("renders the four zone headings", () => {
    renderSidebar("/explore/eng/sql-lab");
    expect(screen.getByText("Explore")).toBeInTheDocument();
    expect(screen.getByText("Data Engineering")).toBeInTheDocument();
    expect(screen.getByText("Data Governance")).toBeInTheDocument();
    expect(screen.getByText("Manage")).toBeInTheDocument();
  });

  test("renders representative items from each new zone", () => {
    renderSidebar("/explore/eng/sql-lab");
    for (const label of [
      "Alerts",
      "Data API",
      "Workflows",
      "Pipelines",
      "Code IDE",
      "Policies",
      "Data Lineage",
      "Query Audit",
      "Data Quality",
    ]) {
      expect(screen.getByRole("link", { name: label })).toBeInTheDocument();
    }
  });

  test("items link to /explore/:ws/<slug>", () => {
    renderSidebar("/explore/eng/sql-lab");
    expect(screen.getByRole("link", { name: "Data API" })).toHaveAttribute(
      "href",
      "/explore/eng/data-api",
    );
    expect(screen.getByRole("link", { name: "Code IDE" })).toHaveAttribute(
      "href",
      "/explore/eng/code-ide",
    );
    expect(screen.getByRole("link", { name: "Data Lineage" })).toHaveAttribute(
      "href",
      "/explore/eng/data-lineage",
    );
    expect(screen.getByRole("link", { name: "Query Audit" })).toHaveAttribute(
      "href",
      "/explore/eng/query-audit",
    );
    expect(screen.getByRole("link", { name: "Data Quality" })).toHaveAttribute(
      "href",
      "/explore/eng/data-quality",
    );
  });
});

describe("Sidebar – Control Plane group", () => {
  test("renders the Control Plane heading and items at /control-plane hrefs", () => {
    renderSidebar("/control-plane/workspaces");
    expect(screen.getByText("Control Plane")).toBeInTheDocument();

    expect(screen.getByRole("link", { name: "Workspaces" })).toHaveAttribute(
      "href",
      "/control-plane/workspaces",
    );
    expect(screen.getByRole("link", { name: "Resources" })).toHaveAttribute(
      "href",
      "/control-plane/resources",
    );
    expect(
      screen.getByRole("link", { name: "Platform Policies" }),
    ).toHaveAttribute("href", "/control-plane/platform-policies");
  });
});
