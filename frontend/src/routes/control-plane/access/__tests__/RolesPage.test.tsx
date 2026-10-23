import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import RolesPage from "@/routes/control-plane/access/RolesPage";
import * as api from "@/api/access";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

vi.mock("@/api/access");

// RBAC-granting writes (create/update/delete role, set permissions) are
// platform_admin-only; default to a platform_admin `me` so existing
// assertions (which exercise those controls) keep passing. Tests that care
// about the non-admin, read-only view pass their own `me`.
const ADMIN_ME: Me = {
  user_id: "1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};

const ROLES: api.AccessRole[] = [
  {
    id: "r1",
    scope: "platform",
    name: "data_steward",
    description: "Manages data governance",
    permissionCount: 2,
    assigneeCount: 1,
    inheritedPermissions: [],
  },
  {
    id: "r2",
    scope: "workspace",
    name: "custom_role",
    description: "A custom workspace role",
    permissionCount: 1,
    assigneeCount: 0,
    inheritedPermissions: [],
  },
];

const PERMISSIONS: api.PermissionCatalogGroup[] = [
  {
    group: "Catalog",
    permissions: [
      {
        key: "catalog.read",
        description: "Read catalog metadata",
        roles: ["data_steward", "custom_role"],
        scope: "platform",
      },
      {
        key: "catalog.write",
        description: "Write catalog metadata",
        roles: ["data_steward"],
        scope: "platform",
      },
    ],
  },
];

function renderPage(me: Me | null = ADMIN_ME) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter>
          <RolesPage />
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("RolesPage", () => {
  beforeEach(() => {
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    vi.mocked(api.listPermissions).mockResolvedValue(PERMISSIONS);
    // The edit dialog fetches the single role for its inherited permissions.
    vi.mocked(api.getRole).mockImplementation(async (id) => {
      const found = ROLES.find((r) => r.id === id) ?? ROLES[0];
      return { ...found, inheritedPermissions: [] };
    });
  });

  test("shows a skeleton while the role list is loading", async () => {
    // A promise that never resolves keeps the query in its loading state.
    vi.mocked(api.listRoles).mockReturnValue(new Promise<api.AccessRole[]>(() => {}));
    renderPage();
    expect(await screen.findByTestId("list-skeleton")).toBeInTheDocument();
    expect(screen.queryByText("No roles.")).not.toBeInTheDocument();
  });

  test("renders rows, marks the seeded role built-in, and disables its delete action", async () => {
    const user = userEvent.setup();
    renderPage();

    expect(await screen.findByText("data_steward")).toBeInTheDocument();
    expect(screen.getByText("custom_role")).toBeInTheDocument();
    expect(screen.getByText("built-in")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Actions for data_steward/i }));
    expect(await screen.findByRole("menuitem", { name: "Delete" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await user.keyboard("{Escape}");

    await user.click(screen.getByRole("button", { name: /Actions for custom_role/i }));
    expect(await screen.findByRole("menuitem", { name: "Delete" })).not.toHaveAttribute(
      "aria-disabled",
    );
  });

  test("New role dialog calls createRole with the entered fields", async () => {
    const user = userEvent.setup();
    vi.mocked(api.createRole).mockResolvedValue({
      id: "r3",
      scope: "platform",
      name: "auditor",
      description: "Read-only audit access",
      permissionCount: 0,
      assigneeCount: 0,
      inheritedPermissions: [],
    });

    renderPage();
    await screen.findByText("data_steward");

    await user.click(screen.getByRole("button", { name: /New role/i }));
    await screen.findByRole("heading", { name: /New role/i });

    // Create is gated until a name is entered.
    const create = screen.getByRole("button", { name: /^Create$/i });
    expect(create).toBeDisabled();

    await user.type(screen.getByLabelText("Name"), "auditor");
    expect(create).toBeEnabled();
    await user.type(screen.getByLabelText("Description"), "Read-only audit access");
    await user.click(create);

    await waitFor(() =>
      expect(api.createRole).toHaveBeenCalledWith({
        scope: "platform",
        name: "auditor",
        description: "Read-only audit access",
      }),
    );
  });

  test("clicking a role name navigates to its detail page", async () => {
    const user = userEvent.setup();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <AuthContext.Provider value={{ me: ADMIN_ME, isLoading: false }}>
        <QueryClientProvider client={qc}>
          <MemoryRouter initialEntries={["/control-plane/roles"]}>
            <Routes>
              <Route path="/control-plane/roles" element={<RolesPage />} />
              <Route path="/control-plane/roles/:id" element={<div>role detail page</div>} />
            </Routes>
          </MemoryRouter>
        </QueryClientProvider>
      </AuthContext.Provider>,
    );

    // The name is a link (D4, KI-223): one colour and one behaviour for names in every list.
    await user.click(await screen.findByRole("link", { name: "custom_role" }));
    expect(await screen.findByText("role detail page")).toBeInTheDocument();
  });

  test("clicking the assignee count opens the assignees dialog and lists them", async () => {
    const user = userEvent.setup();
    vi.mocked(api.getRoleAssignees).mockResolvedValue([
      { kind: "user", name: "alice@example.com" },
      { kind: "group", name: "Engineering" },
    ]);

    renderPage();
    await screen.findByText("data_steward");

    // data_steward has assigneeCount 1, so its count is a clickable control;
    // custom_role has 0, so it stays a plain, non-interactive badge.
    await user.click(screen.getByRole("button", { name: "View assignees of data_steward" }));
    expect(
      screen.queryByRole("button", { name: "View assignees of custom_role" }),
    ).not.toBeInTheDocument();

    await screen.findByRole("heading", { name: /Assignees — data_steward/i });
    expect(await screen.findByText("alice@example.com")).toBeInTheDocument();
    expect(screen.getByText("Engineering")).toBeInTheDocument();
    expect(api.getRoleAssignees).toHaveBeenCalledWith("r1");
  });

  test("renders an empty state when there are no roles", async () => {
    vi.mocked(api.listRoles).mockResolvedValue([]);
    renderPage();
    expect(await screen.findByText("No roles yet")).toBeInTheDocument();
  });

  test("the Scope facet filters to platform-only rows", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("data_steward");
    expect(screen.getByText("custom_role")).toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: "Scope" }));
    await user.click(await screen.findByRole("option", { name: "Platform" }));

    // data_steward is platform scope; custom_role (workspace) is filtered out.
    expect(screen.getByText("data_steward")).toBeInTheDocument();
    expect(screen.queryByText("custom_role")).not.toBeInTheDocument();
  });

  test("typing in the search box narrows rows by name", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("data_steward");

    await user.type(screen.getByPlaceholderText("Search roles…"), "custom");
    expect(screen.getByText("custom_role")).toBeInTheDocument();
    expect(screen.queryByText("data_steward")).not.toBeInTheDocument();
  });

  test("the Tree view toggle renders the hierarchy and hides the search box; List view restores it", async () => {
    const user = userEvent.setup();
    vi.mocked(api.listRoles).mockResolvedValue([
      {
        id: "p1",
        scope: "platform",
        name: "base_role",
        description: "Parent role",
        permissionCount: 1,
        assigneeCount: 0,
        inheritedPermissions: [],
      },
      {
        id: "c1",
        scope: "platform",
        name: "child_role",
        description: "Inherits from base_role",
        permissionCount: 2,
        assigneeCount: 0,
        parentRoleId: "p1",
        inheritedPermissions: [],
      },
      {
        id: "w1",
        scope: "workspace",
        name: "ws_role",
        description: "A workspace role",
        permissionCount: 1,
        assigneeCount: 0,
        inheritedPermissions: [],
      },
    ]);

    renderPage();
    await screen.findByText("ws_role");
    expect(screen.getByPlaceholderText("Search roles…")).toBeInTheDocument();

    // Switch to Tree view: the (nested) child role renders and the search box
    // disappears (search/filter live in the table, which is unmounted).
    await user.click(screen.getByRole("button", { name: "Tree view" }));
    expect(screen.getByText("child_role")).toBeInTheDocument();
    expect(screen.getByText("ws_role")).toBeInTheDocument();
    expect(screen.queryByPlaceholderText("Search roles…")).toBeNull();

    // Switch back to List view: the search box returns.
    await user.click(screen.getByRole("button", { name: "List view" }));
    expect(screen.getByPlaceholderText("Search roles…")).toBeInTheDocument();
  });

  test("the summary strip shows total, platform, and workspace counts", async () => {
    renderPage();
    await screen.findByText("data_steward");

    // ROLES has 2 roles: 1 platform (data_steward), 1 workspace (custom_role).
    // "platform"/"workspace" also appear as scope badges, so assert against the
    // strip container (anchored on the unique "roles" label) with its counts.
    const strip = screen.getByText("roles").closest("div")!;
    expect(strip).toHaveTextContent("2roles");
    expect(strip).toHaveTextContent("1platform");
    expect(strip).toHaveTextContent("1workspace");
  });

  test("the built-in badge and assignee drill-in still render", async () => {
    renderPage();
    await screen.findByText("data_steward");

    // Seeded role keeps its built-in badge.
    expect(screen.getByText("built-in")).toBeInTheDocument();
    // data_steward (assigneeCount 1) exposes the drill-in control; custom_role
    // (0) stays a plain badge.
    expect(
      screen.getByRole("button", { name: "View assignees of data_steward" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "View assignees of custom_role" }),
    ).not.toBeInTheDocument();
  });

  test("a child role's subtitle names the parent it inherits from", async () => {
    // A parent role and a child whose parentRoleId points at it.
    vi.mocked(api.listRoles).mockResolvedValue([
      {
        id: "p1",
        scope: "workspace",
        name: "base_role",
        description: "Parent role",
        permissionCount: 1,
        assigneeCount: 0,
        inheritedPermissions: [],
      },
      {
        id: "c1",
        scope: "workspace",
        name: "child_role",
        description: "Inherits from base_role",
        permissionCount: 2,
        assigneeCount: 0,
        parentRoleId: "p1",
        inheritedPermissions: [],
      },
    ]);

    renderPage();
    await screen.findByText("child_role");

    // Inheritance is folded into the child's identity subtitle rather than a
    // separate column ("Workspace role · inherits base_role").
    expect(screen.getByText(/inherits base_role/i)).toBeInTheDocument();
    // base_role still appears as its own row's name.
    expect(screen.getByText("base_role")).toBeInTheDocument();
  });

  test("Delete confirmation shows the impact: assignee count and inheriting-role count", async () => {
    const user = userEvent.setup();
    vi.mocked(api.listRoles).mockResolvedValue([
      {
        id: "analyst-id",
        scope: "workspace",
        name: "analyst",
        description: "Analyst role",
        permissionCount: 3,
        assigneeCount: 5,
        inheritedPermissions: [],
      },
      {
        id: "junior-id",
        scope: "workspace",
        name: "junior",
        description: "Junior role",
        permissionCount: 1,
        assigneeCount: 0,
        parentRoleId: "analyst-id",
        inheritedPermissions: [],
      },
    ]);

    renderPage();
    await screen.findAllByText("analyst");

    await user.click(screen.getByRole("button", { name: /Actions for analyst/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));

    await screen.findByRole("heading", { name: /Delete "analyst"\?/i });
    expect(screen.getByText(/5 assignee/i)).toBeInTheDocument();
    expect(screen.getByText(/1 role[^.]*inherit/i)).toBeInTheDocument();
  });

  test("a role with no assignees and no inheriting roles does not claim assignees", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("custom_role");

    // custom_role has assigneeCount 0 and nothing inherits from it in ROLES.
    await user.click(screen.getByRole("button", { name: /Actions for custom_role/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));

    await screen.findByRole("heading", { name: /Delete "custom_role"\?/i });
    expect(screen.queryByText(/[1-9]\d* assignee/i)).not.toBeInTheDocument();
  });

  test("a non-admin role:read viewer sees a read-only table: no New role button and no Actions menu", async () => {
    const NON_ADMIN_ME: Me = {
      user_id: "2",
      email: "viewer@example.com",
      platform_roles: [],
      permissions: ["role:read"],
    };
    renderPage(NON_ADMIN_ME);

    expect(await screen.findByText("data_steward")).toBeInTheDocument();
    expect(screen.getByText("custom_role")).toBeInTheDocument();

    expect(screen.queryByRole("button", { name: /New role/i })).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Actions for data_steward/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Actions for custom_role/i }),
    ).not.toBeInTheDocument();

    // The read-only assignee drill-in still works.
    vi.mocked(api.getRoleAssignees).mockResolvedValue([
      { kind: "user", name: "alice@example.com" },
    ]);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "View assignees of data_steward" }));
    expect(await screen.findByText("alice@example.com")).toBeInTheDocument();
  });
});
