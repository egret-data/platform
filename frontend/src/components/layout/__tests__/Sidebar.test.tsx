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
 *   - QueryClientProvider (useWorkspaceFeatures reads useMyWorkspaces)
 *   - collapsed: boolean + onToggle: () => void props
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { Sidebar } from "@/components/layout/Sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

// Default to a platform_admin `me` so existing assertions (which expect every
// item to render) keep passing without threading permissions through every
// call site; tests that care about permission filtering pass their own `me`.
const ADMIN_ME: Me = { user_id: "1", email: "admin@example.com", platform_roles: ["platform_admin"], permissions: [] };

/**
 * `/me/workspaces` never resolves synchronously in these tests, so
 * `useWorkspaceFeatures` sees it as "still loading" (`data === undefined`) and
 * returns every feature key — matching the pre-entitlements behaviour the
 * existing assertions below assume. Tests that care about feature filtering
 * pass their own `fetchImpl` and `await waitFor(...)` for it to resolve.
 */
function renderSidebar(
  initialPath: string,
  me: Me | null = ADMIN_ME,
  fetchImpl: () => Promise<Response> = () => new Promise(() => {}),
) {
  vi.spyOn(globalThis, "fetch").mockImplementation(fetchImpl);
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // Mount Sidebar under routes that expose the :workspace param (mirroring the
  // app), so its `base` resolves to /explore/:ws or /control-plane.
  const sidebar = <Sidebar collapsed={false} onToggle={vi.fn()} />;
  return render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <TooltipProvider>
          <MemoryRouter initialEntries={[initialPath]}>
            <Routes>
              <Route path="/explore/:workspace/*" element={sidebar} />
              <Route path="*" element={sidebar} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  localStorage.clear(); // groups default to expanded
});

afterEach(() => vi.restoreAllMocks());

describe("Sidebar – workspace zones", () => {
  test("renders the four zone headings", () => {
    renderSidebar("/explore/eng/sql-lab");
    expect(screen.getByText("Explore")).toBeInTheDocument();
    expect(screen.getByText("Data Engineering")).toBeInTheDocument();
    expect(screen.getByText("Automate")).toBeInTheDocument();
    expect(screen.getByText("Manage")).toBeInTheDocument();
    // Data Lineage moved to Control Plane → Data governance, so Explore no
    // longer has a Data Governance group.
    expect(screen.queryByText("Data Governance")).not.toBeInTheDocument();
  });

  test("renders representative items from each zone", () => {
    renderSidebar("/explore/eng/sql-lab");
    for (const label of [
      "SQL Lab",
      "Data API",
      "Pipelines",
      "Workflows",
      "Workspace settings",
    ]) {
      expect(screen.getByRole("link", { name: label })).toBeInTheDocument();
    }
    // Engine connections moved into Workspace settings → Connections, so they're
    // no longer a top-level Manage item; the Manage entry is Workspace settings.
    expect(screen.queryByRole("link", { name: "Databases" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Engines" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Workspace settings" })).toHaveAttribute(
      "href",
      "/explore/eng/settings/members",
    );
  });

  /**
   * The <div> holding a group's heading toggle and its items. Asserted
   * structurally rather than by mere presence: "Alerts & Reports is somewhere
   * in the sidebar" was already true before the move, so only containment
   * inside the right group can tell the two states apart.
   */
  const groupOf = (heading: string) =>
    screen.getByRole("button", { name: new RegExp(`^${heading}$`, "i") }).parentElement;

  test("Alerts & Reports is an Explore module, not an Automate item", () => {
    renderSidebar("/explore/eng/sql-lab");
    const link = screen.getByRole("link", { name: "Alerts & Reports" });
    expect(groupOf("Explore")).toContainElement(link);
    expect(groupOf("Automate")).not.toContainElement(link);
    expect(link).toHaveAttribute("href", "/explore/eng/alerts");
    // The old Automate entry is gone, label and all.
    expect(screen.queryByRole("link", { name: "Alerts" })).not.toBeInTheDocument();
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
    // Data Lineage is no longer a workspace item (moved to the Control Plane).
    expect(screen.queryByRole("link", { name: "Data Lineage" })).not.toBeInTheDocument();
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

describe("Sidebar – Infrastructure group", () => {
  test("renders the Infrastructure items (Workspaces, Engines, Warehouses, Storage, Secrets)", () => {
    renderSidebar("/control-plane/workspaces");
    expect(screen.getByText("Infrastructure")).toBeInTheDocument();

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

describe("Sidebar – Identity + Access groups", () => {
  test("renders Users, Groups, Roles, Permissions at their /control-plane routes", () => {
    renderSidebar("/control-plane/workspaces");
    // The old flat "Access control" group is split into a mental-model IA.
    expect(screen.getByText("Identity")).toBeInTheDocument();
    expect(screen.getByText("Access")).toBeInTheDocument();

    expect(screen.getByRole("link", { name: "Users" })).toHaveAttribute(
      "href",
      "/control-plane/users",
    );
    expect(screen.getByRole("link", { name: "Access overview" })).toHaveAttribute(
      "href",
      "/control-plane/access-overview",
    );
    expect(screen.getByRole("link", { name: "Access Explorer" })).toHaveAttribute(
      "href",
      "/control-plane/access-explorer",
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

  test("Overview leads the Control Plane, ahead of the Identity group", () => {
    renderSidebar("/control-plane/workspaces");
    const overview = screen.getByRole("link", { name: "Access overview" });
    const users = screen.getByRole("link", { name: "Users" });
    // Overview is its own top zone and must precede the Identity group's Users.
    expect(overview.compareDocumentPosition(users) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe("Sidebar – Control Plane Data Governance group", () => {
  test("renders the Policies item at /control-plane/policies", () => {
    renderSidebar("/control-plane/workspaces");
    expect(screen.getByText("Data governance")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Policies" })).toHaveAttribute(
      "href",
      "/control-plane/policies",
    );
    // Audit log has no `perm` → platform_admin-only, slugified to `audit-log`.
    expect(screen.getByRole("link", { name: "Audit log" })).toHaveAttribute(
      "href",
      "/control-plane/audit-log",
    );
    // Data Lineage now lives here (moved out of the workspace), admin-only.
    expect(screen.getByRole("link", { name: "Data Lineage" })).toHaveAttribute(
      "href",
      "/control-plane/data-lineage",
    );
  });

  test("renders the Classifications item at /control-plane/classifications", () => {
    renderSidebar("/control-plane/workspaces");
    expect(screen.getByRole("link", { name: "Classifications" })).toHaveAttribute(
      "href",
      "/control-plane/classifications",
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

    // Groups with no visible items are dropped entirely, heading included:
    // Identity + Access need user/group/role:read; Infrastructure needs
    // engine/warehouse/storage/secret:read — none visible for a policy:read user.
    expect(screen.queryByText("Identity")).not.toBeInTheDocument();
    expect(screen.queryByText("Access")).not.toBeInTheDocument();
    expect(screen.queryByText("Infrastructure")).not.toBeInTheDocument();
    // Only the Data governance group (Policies) survives.
    expect(screen.getByText("Data governance")).toBeInTheDocument();
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
      "Classifications",
      "Data Lineage",
      "Audit log",
    ]) {
      expect(screen.getByRole("link", { name: label })).toBeInTheDocument();
    }
  });
});

describe("Sidebar – workspace feature entitlements", () => {
  const wsResponse = (features: string[]) =>
    Promise.resolve(
      new Response(
        JSON.stringify([{ id: "1", slug: "eng", name: "Engineering", features }]),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    );

  test("while /me/workspaces is still loading, every Explore item shows (no flash)", () => {
    // fetchImpl never resolves — the query stays pending for the whole test.
    renderSidebar("/explore/eng/sql-lab");
    for (const label of [
      "SQL Lab",
      "Datasets",
      "Charts",
      "Dashboards",
      "Notebooks",
      "Alerts & Reports",
    ]) {
      expect(screen.getByRole("link", { name: label })).toBeInTheDocument();
    }
  });

  test("hides Alerts & Reports when the workspace lacks the alerts entitlement", async () => {
    // The failure mode this pins: a frontend-only move leaves `alerts` out of
    // the BACKEND feature catalog, so `/me/workspaces` never lists the key and
    // the item is filtered out of a workspace that should have it.
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () =>
      wsResponse(["sql_lab", "datasets", "charts", "dashboards", "notebooks"]),
    );
    await waitFor(() =>
      expect(screen.queryByRole("link", { name: "Alerts & Reports" })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("link", { name: "Notebooks" })).toBeInTheDocument();
  });

  test("shows Alerts & Reports when the workspace has the alerts entitlement", async () => {
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () => wsResponse(["alerts"]));
    // Wait on SQL Lab DISAPPEARING, not on Alerts appearing: the optimistic
    // loading state returns every key, so "Alerts is present" is already true
    // before the fetch resolves and would pass without proving anything.
    await waitFor(() =>
      expect(screen.queryByRole("link", { name: "SQL Lab" })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("link", { name: "Alerts & Reports" })).toBeInTheDocument();
  });

  test("hides a disabled item once the workspace's features load", async () => {
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () => wsResponse(["sql_lab", "datasets", "charts", "dashboards"]));
    await waitFor(() =>
      expect(screen.queryByRole("link", { name: "Notebooks" })).not.toBeInTheDocument(),
    );
    for (const label of ["SQL Lab", "Datasets", "Charts", "Dashboards"]) {
      expect(screen.getByRole("link", { name: label })).toBeInTheDocument();
    }
  });

  test("drops the Explore group entirely when every module is disabled", async () => {
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () => wsResponse([]));
    await waitFor(() => expect(screen.queryByText("Explore")).not.toBeInTheDocument());
    for (const label of [
      "SQL Lab",
      "Datasets",
      "Charts",
      "Dashboards",
      "Notebooks",
      "Alerts & Reports",
    ]) {
      expect(screen.queryByRole("link", { name: label })).not.toBeInTheDocument();
    }
    // Unaffected groups (no `feature` on their items) still render.
    expect(screen.getByText("Automate")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Workspace settings" })).toBeInTheDocument();
  });

  test("applies even to a platform_admin — entitlements have no bypass", async () => {
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () => wsResponse(["sql_lab"]));
    await waitFor(() =>
      expect(screen.queryByRole("link", { name: "Notebooks" })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("link", { name: "SQL Lab" })).toBeInTheDocument();
  });
});
