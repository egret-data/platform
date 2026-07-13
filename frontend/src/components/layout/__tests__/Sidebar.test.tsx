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
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";
import { Sidebar } from "@/components/layout/Sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

// Default to a platform_admin `me` so existing assertions (which expect every
// item to render) keep passing without threading permissions through every
// call site; tests that care about permission filtering pass their own `me`.
const ADMIN_ME: Me = { user_id: "1", email: "admin@example.com", platform_roles: ["platform_admin"], permissions: [] };

function renderSidebar(initialPath: string, me: Me | null = ADMIN_ME) {
  // Mount Sidebar under routes that expose the :workspace param (mirroring the
  // app), so its `base` resolves to /explore/:ws or /control-plane.
  const sidebar = <Sidebar collapsed={false} onToggle={vi.fn()} />;
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[initialPath]}>
          <Routes>
            <Route path="/explore/:workspace/*" element={sidebar} />
            <Route path="*" element={sidebar} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </AuthContext.Provider>,
  );
}

beforeEach(() => {
  localStorage.clear(); // groups default to expanded
});

describe("Sidebar – workspace zones", () => {
  test("renders the five zone headings", () => {
    renderSidebar("/explore/eng/sql-lab");
    expect(screen.getByText("Explore")).toBeInTheDocument();
    expect(screen.getByText("Data Engineering")).toBeInTheDocument();
    expect(screen.getByText("Automate")).toBeInTheDocument();
    expect(screen.getByText("Data Governance")).toBeInTheDocument();
    expect(screen.getByText("Manage")).toBeInTheDocument();
  });

  test("renders representative items from each zone", () => {
    renderSidebar("/explore/eng/sql-lab");
    for (const label of [
      "SQL Lab",
      "Data API",
      "Pipelines",
      "Workflows",
      "Alerts",
      "Data Lineage",
      "Engines",
    ]) {
      expect(screen.getByRole("link", { name: label })).toBeInTheDocument();
    }
    // 'Databases' was renamed to 'Engines' in Manage.
    expect(screen.queryByRole("link", { name: "Databases" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Engines" })).toHaveAttribute(
      "href",
      "/explore/eng/engines",
    );
  });

  test("dropped items are gone", () => {
    renderSidebar("/explore/eng/sql-lab");
    for (const label of ["Code IDE", "Query Audit", "Data Quality"]) {
      expect(screen.queryByRole("link", { name: label })).not.toBeInTheDocument();
    }
  });

  test("Policies moved out of Explore's Data Governance group", () => {
    renderSidebar("/explore/eng/sql-lab");
    expect(screen.queryByRole("link", { name: "Policies" })).not.toBeInTheDocument();
  });

  test("items link to /explore/:ws/<slug>", () => {
    renderSidebar("/explore/eng/sql-lab");
    expect(screen.getByRole("link", { name: "Data API" })).toHaveAttribute(
      "href",
      "/explore/eng/data-api",
    );
    expect(screen.getByRole("link", { name: "Workflows" })).toHaveAttribute(
      "href",
      "/explore/eng/workflows",
    );
    expect(screen.getByRole("link", { name: "Data Lineage" })).toHaveAttribute(
      "href",
      "/explore/eng/data-lineage",
    );
  });

  test("a group heading collapses/expands its items", async () => {
    const user = userEvent.setup();
    renderSidebar("/explore/eng/sql-lab");
    // 'Data Engineering' starts expanded → Data API visible.
    expect(screen.getByRole("link", { name: "Data API" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Data Engineering/i }));
    expect(screen.queryByRole("link", { name: "Data API" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Data Engineering/i }));
    expect(screen.getByRole("link", { name: "Data API" })).toBeInTheDocument();
  });
});

describe("Sidebar – Control Plane group", () => {
  test("renders the Control Plane items (Workspaces, Engines, Warehouses, Storage, Secrets)", () => {
    renderSidebar("/control-plane/workspaces");
    expect(screen.getByText("Control Plane")).toBeInTheDocument();

    expect(screen.getByRole("link", { name: "Workspaces" })).toHaveAttribute(
      "href",
      "/control-plane/workspaces",
    );
    expect(screen.getByRole("link", { name: "Engines" })).toHaveAttribute(
      "href",
      "/control-plane/engines",
    );
    expect(screen.getByRole("link", { name: "Warehouses" })).toHaveAttribute(
      "href",
      "/control-plane/warehouses",
    );
    expect(screen.getByRole("link", { name: "Storage" })).toHaveAttribute(
      "href",
      "/control-plane/storage",
    );
    expect(screen.getByRole("link", { name: "Secrets" })).toHaveAttribute(
      "href",
      "/control-plane/secrets",
    );
    // Dropped from the Control Plane.
    for (const gone of ["Resources", "Monitoring", "Platform Policies"]) {
      expect(screen.queryByRole("link", { name: gone })).not.toBeInTheDocument();
    }
  });
});

describe("Sidebar – Access control group", () => {
  test("renders Users, Groups, Roles, Permissions at their /control-plane routes", () => {
    renderSidebar("/control-plane/workspaces");
    expect(screen.getByText("Access control")).toBeInTheDocument();

    expect(screen.getByRole("link", { name: "Users" })).toHaveAttribute(
      "href",
      "/control-plane/users",
    );
    expect(screen.getByRole("link", { name: "Overview" })).toHaveAttribute(
      "href",
      "/control-plane/access-overview",
    );
    expect(screen.getByRole("link", { name: "Who can access" })).toHaveAttribute(
      "href",
      "/control-plane/who-can-access",
    );
    expect(screen.getByRole("link", { name: "Groups" })).toHaveAttribute(
      "href",
      "/control-plane/groups",
    );
    expect(screen.getByRole("link", { name: "Roles" })).toHaveAttribute(
      "href",
      "/control-plane/roles",
    );
    expect(screen.getByRole("link", { name: "Permissions" })).toHaveAttribute(
      "href",
      "/control-plane/permissions",
    );
  });

  test("Overview is the first item in the Access control group", () => {
    renderSidebar("/control-plane/workspaces");
    const overview = screen.getByRole("link", { name: "Overview" });
    const users = screen.getByRole("link", { name: "Users" });
    // Overview must precede Users (which was previously the first item).
    expect(overview.compareDocumentPosition(users) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe("Sidebar – Control Plane Data Governance group", () => {
  test("renders the Policies item at /control-plane/policies", () => {
    renderSidebar("/control-plane/workspaces");
    expect(screen.getByText("Data Governance")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Policies" })).toHaveAttribute(
      "href",
      "/control-plane/policies",
    );
    // Audit log has no `perm` → platform_admin-only, slugified to `audit-log`.
    expect(screen.getByRole("link", { name: "Audit log" })).toHaveAttribute(
      "href",
      "/control-plane/audit-log",
    );
  });
});

describe("Sidebar – permission-based filtering", () => {
  test("a scoped (non platform_admin) user with only policy:read sees Policies but not Engines/Users/Roles/Workspaces", () => {
    renderSidebar("/control-plane/workspaces", {
      user_id: "2",
      email: "scoped@example.com",
      platform_roles: [],
      permissions: ["policy:read"],
    });

    expect(screen.getByRole("link", { name: "Policies" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Engines" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Users" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Roles" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Workspaces" })).not.toBeInTheDocument();

    // Groups with no visible items are dropped entirely, heading included.
    expect(screen.queryByText("Access control")).not.toBeInTheDocument();
    // The "Control Plane" heading (Workspaces/Engines/Warehouses/Storage/Secrets)
    // has nothing visible for this user either.
    expect(screen.queryByText("Control Plane")).not.toBeInTheDocument();
    expect(screen.getByText("Data Governance")).toBeInTheDocument();
  });

  test("a platform_admin sees every Control Plane item regardless of permissions", () => {
    renderSidebar("/control-plane/workspaces", {
      user_id: "3",
      email: "admin2@example.com",
      platform_roles: ["platform_admin"],
      permissions: [],
    });

    for (const label of [
      "Workspaces",
      "Engines",
      "Warehouses",
      "Storage",
      "Secrets",
      "Users",
      "Groups",
      "Roles",
      "Permissions",
      "Policies",
      "Audit log",
    ]) {
      expect(screen.getByRole("link", { name: label })).toBeInTheDocument();
    }
  });
});
