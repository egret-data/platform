/**
 * Sidebar zone tests
 *
 * Verifies the workspace sidebar renders the IA zones (Explore / Data API /
 * Manage) with their items at the right
 * `/explore/:ws/...` hrefs, and the Control Plane group at `/control-plane/...`.
 *
 * Sidebar requires:
 *   - MemoryRouter (uses useLocation + useParams)
 *   - TooltipProvider (renders Radix Tooltip in collapsed mode)
 *   - QueryClientProvider (useWorkspaceFeatures reads useMyWorkspaces)
 *   - collapsed: boolean + onToggle: () => void props
 */
import { render, screen, waitFor, within } from "@testing-library/react";
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
  test("renders the three zone headings", () => {
    renderSidebar("/explore/eng/sql-lab");
    expect(screen.getByText("Explore")).toBeInTheDocument();
    expect(screen.getByText("Data API")).toBeInTheDocument();
    expect(screen.getByText("Manage")).toBeInTheDocument();
    // Automate and Data Engineering each held exactly one placeholder item
    // (Workflows, Pipelines). Both were removed in W0, and Workflows came back
    // in W7a as its own group (tested below), not under either old heading.
    expect(screen.queryByText("Automate")).not.toBeInTheDocument();
    expect(screen.queryByText("Data Engineering")).not.toBeInTheDocument();
    expect(screen.queryByText("Data Governance")).not.toBeInTheDocument();
  });

  test("renders representative items from each zone", () => {
    renderSidebar("/explore/eng/sql-lab");
    for (const label of [
      "SQL Lab",
      "Endpoints",
      "Clients",
      "Activity",
      "Workspace settings",
    ]) {
      expect(screen.getByRole("link", { name: label })).toBeInTheDocument();
    }
    // Data API is a sidebar GROUP now (Endpoints/Clients/Activity), not an
    // item with its own link — there is no "Data API" link to find.
    expect(screen.queryByRole("link", { name: "Data API" })).not.toBeInTheDocument();
    // Engine connections moved into Workspace settings → Connections, so they're
    // no longer a top-level Manage item; the Manage entry is Workspace settings.
    expect(screen.queryByRole("link", { name: "Databases" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Engines" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Workspace settings" })).toHaveAttribute(
      "href",
      "/explore/eng/settings/members",
    );
    // Workflows is a GROUP (Flows/Executions), like Data API, so there is no
    // "Workflows" link; Pipelines was a W0 placeholder and stays gone.
    expect(screen.queryByRole("link", { name: "Workflows" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Pipelines" })).not.toBeInTheDocument();
  });

  /**
   * The <div> holding a group's heading toggle and its items. Asserted
   * structurally rather than by mere presence: "Alerts & Reports is somewhere
   * in the sidebar" was already true before the move, so only containment
   * inside the right group can tell the two states apart.
   */
  const groupOf = (heading: string) =>
    screen.getByRole("button", { name: new RegExp(`^${heading}$`, "i") }).parentElement;

  test("Alerts & Reports is an Explore module", () => {
    renderSidebar("/explore/eng/sql-lab");
    const link = screen.getByRole("link", { name: "Alerts & Reports" });
    expect(groupOf("Explore")).toContainElement(link);
    // Automate no longer exists as a group at all (W0 removed its only item).
    expect(screen.queryByText("Automate")).not.toBeInTheDocument();
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
    expect(screen.getByRole("link", { name: "Endpoints" })).toHaveAttribute(
      "href",
      "/explore/eng/data-api/endpoints",
    );
    expect(screen.getByRole("link", { name: "Clients" })).toHaveAttribute(
      "href",
      "/explore/eng/data-api/clients",
    );
    expect(screen.getByRole("link", { name: "Activity" })).toHaveAttribute(
      "href",
      "/explore/eng/data-api/activity",
    );
    // Data Lineage is not a workspace item (it lives in the Governance app).
    expect(screen.queryByRole("link", { name: "Data Lineage" })).not.toBeInTheDocument();
  });

  test("a group heading collapses/expands its items", async () => {
    const user = userEvent.setup();
    renderSidebar("/explore/eng/sql-lab");
    // 'Data API' starts expanded → its three items are visible.
    expect(screen.getByRole("link", { name: "Endpoints" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Data API$/i }));
    expect(screen.queryByRole("link", { name: "Endpoints" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Clients" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Activity" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Data API$/i }));
    expect(screen.getByRole("link", { name: "Endpoints" })).toBeInTheDocument();
  });

  test("Data API sits right after Explore and before Manage", () => {
    renderSidebar("/explore/eng/sql-lab");
    const dataApiHeading = screen.getByRole("button", { name: /^Data API$/i });
    const exploreHeading = screen.getByText("Explore");
    const manageHeading = screen.getByRole("button", { name: /^Manage$/i });
    expect(
      exploreHeading.compareDocumentPosition(dataApiHeading) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      dataApiHeading.compareDocumentPosition(manageHeading) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  // The whole Data API group is gated on the `data_api` workspace
  // entitlement — every item in it carries `feature: "data_api"`, so with the
  // feature off the group filters down to zero items and the heading itself
  // (per the generic group-drop rule tested elsewhere in this file) is
  // dropped too, not just its items.
  test("hides the entire Data API group when the workspace lacks the entitlement", async () => {
    const wsResponse = (features: string[]) =>
      Promise.resolve(
        new Response(
          JSON.stringify([{ id: "1", slug: "eng", name: "Engineering", features }]),
          { status: 200, headers: { "content-type": "application/json" } },
        ),
      );
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () =>
      wsResponse(["sql_lab", "datasets", "charts", "dashboards", "apps"]),
    );
    await waitFor(() => expect(screen.queryByRole("link", { name: "Endpoints" })).not.toBeInTheDocument());
    expect(screen.queryByRole("link", { name: "Clients" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Activity" })).not.toBeInTheDocument();
    expect(screen.queryByText("Data API")).not.toBeInTheDocument();
    // Unaffected groups still render.
    expect(screen.getByRole("link", { name: "SQL Lab" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Workspace settings" })).toBeInTheDocument();
  });
});

describe("Sidebar – Infrastructure group", () => {
  test("renders the Infrastructure items (Workspaces, Engines, Catalogs, Storage, Secrets)", () => {
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
    // SP3c-2 (Task 8): the Warehouses entry is gone — its five tabs moved onto
    // CatalogDetail in SP3c-1, and `/control-plane/warehouses/:id` survives
    // only as an unlisted compatibility redirect. Catalogs takes its path
    // from `slug("Catalogs")`, no explicit `to`.
    expect(screen.getByRole("link", { name: "Catalogs" })).toHaveAttribute(
      "href",
      "/control-plane/catalogs",
    );
    expect(screen.queryByRole("link", { name: "Warehouses" })).not.toBeInTheDocument();
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

  test("Catalogs sits between Engines and Storage", () => {
    renderSidebar("/control-plane/workspaces");
    const engines = screen.getByRole("link", { name: "Engines" });
    const catalogs = screen.getByRole("link", { name: "Catalogs" });
    const storage = screen.getByRole("link", { name: "Storage" });
    expect(engines.compareDocumentPosition(catalogs) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(catalogs.compareDocumentPosition(storage) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  // The other half of SP3c D3's lock-out guard: `catalog_registry:read` must
  // both admit the user to Control Plane (see appAccess.test.ts) and unlock a
  // section once they are in — a user admitted to an app with an empty sidebar
  // is locked out just as effectively.
  test("a catalog_registry:read-only user sees Catalogs, no other Infrastructure item", () => {
    renderSidebar("/control-plane/catalogs", {
      user_id: "9",
      email: "catalogs@example.com",
      platform_roles: [],
      permissions: ["catalog_registry:read"],
    });
    expect(screen.getByRole("link", { name: "Catalogs" })).toHaveAttribute(
      "href",
      "/control-plane/catalogs",
    );
    for (const gone of ["Warehouses", "Engines", "Storage", "Secrets", "Workspaces"]) {
      expect(screen.queryByRole("link", { name: gone })).not.toBeInTheDocument();
    }
  });
});

/**
 * The `Access` heading belongs to `CONTROL_PLANE_GROUPS`, and `groups` is a
 * hard either/or on the app (`appForPath`) — permission groups OR Explore
 * groups, never both. So on an Explore route there is no `Access` heading at all, for anyone,
 * admin included.
 *
 * This is asserted because its ABSENCE was load-bearing and nobody had written
 * it down. Every existing `Access` assertion in this file renders
 * `/control-plane/...`; nothing said what an Explore route shows. A Data API
 * doc comment and a `ui-conventions` §3.9 bullet were both written on the
 * belief that a reader on `/explore/:ws/data-api/clients/new` could see an
 * `Access` sidebar group and confuse it with an endpoint's Access TAB. They
 * cannot — the tab needs disambiguating from nothing, it needs a PATH, which
 * is a different argument for the same wording. Both were corrected against
 * this test.
 */
describe("Sidebar – no Access heading on an Explore route", () => {
  test("an Explore route renders the Explore groups and no control-plane Access group", () => {
    renderSidebar("/explore/eng/data-api/clients");

    // The positive half first: this really is the Explore sidebar, so an empty
    // render cannot satisfy the negative below.
    expect(screen.getByText("Data API")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Clients" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Endpoints" })).toBeInTheDocument();

    // By identity over the whole rendered sidebar, not `queryByText("Access")`
    // — an exact-text query would miss a heading spelled "Access control" and
    // report the absence this comment depends on.
    expect(screen.queryByText(/access/i)).not.toBeInTheDocument();
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

describe("Sidebar – Control Plane System group", () => {
  test("Audit log sits under System and the governance items are gone from the Control Plane", () => {
    renderSidebar("/control-plane/workspaces");
    const auditLog = screen.getByRole("link", { name: "Audit log" });
    // Audit log has no `perm` → platform_admin-only, slugified to `audit-log`.
    expect(auditLog).toHaveAttribute("href", "/control-plane/audit-log");
    expect(screen.getByRole("button", { name: /^System$/i }).parentElement).toContainElement(auditLog);

    // The link above proves this is the Control Plane sidebar, so these absences mean something.
    for (const moved of ["Policies", "Classifications", "Data Lineage"]) {
      expect(screen.queryByRole("link", { name: moved })).not.toBeInTheDocument();
    }
    expect(screen.queryByText(/data governance/i)).not.toBeInTheDocument();
  });
});

describe("Sidebar – Governance app", () => {
  const groupOf = (heading: string) =>
    screen.getByRole("button", { name: new RegExp(`^${heading}$`, "i") }).parentElement;

  test("renders Policies, Classification and Lineage groups at /governance routes", () => {
    renderSidebar("/governance/policies");
    const policies = screen.getByRole("link", { name: "Policies" });
    const classifications = screen.getByRole("link", { name: "Classifications" });
    const lineage = screen.getByRole("link", { name: "Data Lineage" });

    expect(policies).toHaveAttribute("href", "/governance/policies");
    expect(classifications).toHaveAttribute("href", "/governance/classifications");
    expect(lineage).toHaveAttribute("href", "/governance/data-lineage");

    expect(groupOf("Policies")).toContainElement(policies);
    expect(groupOf("Classification")).toContainElement(classifications);
    expect(groupOf("Lineage")).toContainElement(lineage);
  });

  test("renders neither Control Plane nor Explore items", () => {
    renderSidebar("/governance/classifications");
    expect(screen.getByRole("link", { name: "Classifications" })).toBeInTheDocument();
    for (const label of ["Engines", "Users", "Audit log", "Workspaces", "SQL Lab", "Workspace settings"]) {
      expect(screen.queryByRole("link", { name: label })).not.toBeInTheDocument();
    }
  });

  test("a tag-only user sees Classifications and nothing else", () => {
    renderSidebar("/governance/classifications", {
      user_id: "4",
      email: "tags@example.com",
      platform_roles: [],
      permissions: ["tag:read"],
    });
    expect(screen.getByRole("link", { name: "Classifications" })).toHaveAttribute(
      "href",
      "/governance/classifications",
    );
    expect(screen.queryByRole("link", { name: "Policies" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Data Lineage" })).not.toBeInTheDocument();
    // Groups left with no visible items are dropped, heading included.
    expect(screen.queryByRole("button", { name: /^Policies$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Lineage$/i })).not.toBeInTheDocument();
  });

  test("a policy-only user sees Policies and not Classifications", () => {
    renderSidebar("/governance/policies", {
      user_id: "5",
      email: "policies@example.com",
      platform_roles: [],
      permissions: ["policy:read"],
    });
    expect(screen.getByRole("link", { name: "Policies" })).toHaveAttribute("href", "/governance/policies");
    expect(screen.queryByRole("link", { name: "Classifications" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Classification$/i })).not.toBeInTheDocument();
  });
});

describe("Sidebar – permission-based filtering", () => {
  test("a scoped (non platform_admin) user with only engine:read sees Engines and nothing gated elsewhere", () => {
    renderSidebar("/control-plane/engines", {
      user_id: "2",
      email: "scoped@example.com",
      platform_roles: [],
      permissions: ["engine:read"],
    });

    expect(screen.getByRole("link", { name: "Engines" })).toBeInTheDocument();
    for (const label of ["Users", "Roles", "Workspaces", "Secrets", "Audit log"]) {
      expect(screen.queryByRole("link", { name: label })).not.toBeInTheDocument();
    }

    // Groups with no visible items are dropped entirely, heading included.
    expect(screen.queryByText("Identity")).not.toBeInTheDocument();
    expect(screen.queryByText("Access")).not.toBeInTheDocument();
    // System holds only the admin-only Audit log.
    expect(screen.queryByText("System")).not.toBeInTheDocument();
    expect(screen.getByText("Infrastructure")).toBeInTheDocument();
  });

  test("a platform_admin sees every Control Plane item regardless of permissions", () => {
    renderSidebar("/control-plane/workspaces", {
      user_id: "3",
      email: "admin2@example.com",
      platform_roles: ["platform_admin"],
      permissions: [],
    });

    for (const label of [
      "Access overview",
      "Users",
      "Groups",
      "Roles",
      "Access Explorer",
      "Permissions",
      "Workspaces",
      "Engines",
      "Storage",
      "Secrets",
      "Audit log",
    ]) {
      expect(screen.getByRole("link", { name: label })).toBeInTheDocument();
    }
  });
});

describe("Sidebar – Workflows group", () => {
  const wsResponse = (features: string[]) =>
    Promise.resolve(
      new Response(
        JSON.stringify([{ id: "1", slug: "eng", name: "Engineering", features }]),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    );

  test("shows Workflows with Flows and Executions when the workspace has workflows", async () => {
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () => wsResponse(["sql_lab", "workflows"]));
    // Wait on Datasets DISAPPEARING: the optimistic loading state already shows
    // every group, so waiting on Workflows appearing would prove nothing.
    await waitFor(() => expect(screen.queryByRole("link", { name: "Datasets" })).not.toBeInTheDocument());
    const heading = screen.getByRole("button", { name: /^Workflows$/i });
    const group = heading.parentElement as HTMLElement;
    expect(within(group).getByRole("link", { name: "Flows" })).toHaveAttribute(
      "href",
      "/explore/eng/workflows/flows",
    );
    expect(within(group).getByRole("link", { name: "Executions" })).toHaveAttribute(
      "href",
      "/explore/eng/workflows/executions",
    );
  });

  test("lists Namespaces after Executions in the Workflows group (W7d)", async () => {
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () => wsResponse(["sql_lab", "workflows"]));
    await waitFor(() => expect(screen.queryByRole("link", { name: "Datasets" })).not.toBeInTheDocument());
    const group = screen.getByRole("button", { name: /^Workflows$/i }).parentElement as HTMLElement;
    const namespaces = within(group).getByRole("link", { name: "Namespaces" });
    expect(namespaces).toHaveAttribute("href", "/explore/eng/workflows/namespaces");
    const executions = within(group).getByRole("link", { name: "Executions" });
    expect(executions.compareDocumentPosition(namespaces) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  test("lists Triggers after Namespaces in the Workflows group (W7e)", async () => {
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () => wsResponse(["sql_lab", "workflows"]));
    await waitFor(() => expect(screen.queryByRole("link", { name: "Datasets" })).not.toBeInTheDocument());
    const group = screen.getByRole("button", { name: /^Workflows$/i }).parentElement as HTMLElement;
    const triggers = within(group).getByRole("link", { name: "Triggers" });
    expect(triggers).toHaveAttribute("href", "/explore/eng/workflows/triggers");
    const links = within(group).getAllByRole("link").map((l) => l.textContent);
    expect(links).toEqual(["Flows", "Executions", "Namespaces", "Triggers"]);
  });

  test("hides Triggers with the rest of the group when the workspace lacks workflows (W7e)", async () => {
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () => wsResponse(["sql_lab", "data_api"]));
    await waitFor(() => expect(screen.queryByRole("link", { name: "Flows" })).not.toBeInTheDocument());
    expect(screen.queryByRole("link", { name: "Triggers" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Endpoints" })).toBeInTheDocument();
  });

  test("hides the whole Workflows group when the workspace lacks workflows", async () => {
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () => wsResponse(["sql_lab", "data_api"]));
    await waitFor(() => expect(screen.queryByRole("link", { name: "Flows" })).not.toBeInTheDocument());
    expect(screen.queryByRole("link", { name: "Executions" })).not.toBeInTheDocument();
    expect(screen.queryByText("Workflows")).not.toBeInTheDocument();
    // Unaffected groups still render.
    expect(screen.getByRole("link", { name: "Endpoints" })).toBeInTheDocument();
  });

  test("Workflows sits after Data API and before Manage", () => {
    renderSidebar("/explore/eng/sql-lab");
    const dataApi = screen.getByRole("button", { name: /^Data API$/i });
    const workflows = screen.getByRole("button", { name: /^Workflows$/i });
    const manage = screen.getByRole("button", { name: /^Manage$/i });
    expect(dataApi.compareDocumentPosition(workflows) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(workflows.compareDocumentPosition(manage) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
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
      "Apps",
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
      wsResponse(["sql_lab", "datasets", "charts", "dashboards", "apps"]),
    );
    await waitFor(() =>
      expect(screen.queryByRole("link", { name: "Alerts & Reports" })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("link", { name: "Apps" })).toBeInTheDocument();
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
      expect(screen.queryByRole("link", { name: "Apps" })).not.toBeInTheDocument(),
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
      "Apps",
      "Alerts & Reports",
    ]) {
      expect(screen.queryByRole("link", { name: label })).not.toBeInTheDocument();
    }
    // Unaffected groups (no `feature` on their items) still render.
    expect(screen.getByText("Manage")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Workspace settings" })).toBeInTheDocument();
  });

  test("applies even to a platform_admin — entitlements have no bypass", async () => {
    renderSidebar("/explore/eng/sql-lab", ADMIN_ME, () => wsResponse(["sql_lab"]));
    await waitFor(() =>
      expect(screen.queryByRole("link", { name: "Apps" })).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("link", { name: "SQL Lab" })).toBeInTheDocument();
  });
});
