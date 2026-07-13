import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
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
        <RolesPage />
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

    await user.type(screen.getByLabelText("Name"), "auditor");
    await user.type(screen.getByLabelText("Description"), "Read-only audit access");
    await user.click(screen.getByRole("button", { name: /^Create$/i }));

    await waitFor(() =>
      expect(api.createRole).toHaveBeenCalledWith({
        scope: "platform",
        name: "auditor",
        description: "Read-only audit access",
      }),
    );
  });

  test("Edit dialog prefills checked permissions from listPermissions and Save persists the toggled set", async () => {
    const user = userEvent.setup();
    vi.mocked(api.setRolePermissions).mockResolvedValue(undefined);
    // custom_role is a workspace role; the matrix only shows workspace-scoped
    // permissions, so this fixture matches the role's scope.
    vi.mocked(api.listPermissions).mockResolvedValue([
      {
        group: "Catalog",
        permissions: [
          {
            key: "catalog.read",
            description: "Read catalog metadata",
            roles: ["data_steward", "custom_role"],
            scope: "workspace",
          },
          {
            key: "catalog.write",
            description: "Write catalog metadata",
            roles: ["data_steward"],
            scope: "workspace",
          },
        ],
      },
    ]);

    renderPage();
    await screen.findByText("custom_role");

    await user.click(screen.getByRole("button", { name: /Actions for custom_role/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit permissions" }));
    await screen.findByRole("heading", { name: /Edit role — custom_role/i });

    // custom_role is only in the `roles` list of catalog.read, so it should
    // start checked; catalog.write should start unchecked.
    const readCheckbox = screen.getByRole("checkbox", { name: "catalog.read" });
    const writeCheckbox = screen.getByRole("checkbox", { name: "catalog.write" });
    expect(readCheckbox).toHaveAttribute("data-state", "checked");
    expect(writeCheckbox).toHaveAttribute("data-state", "unchecked");

    await user.click(writeCheckbox);
    await user.click(screen.getByRole("button", { name: /^Save$/i }));

    await waitFor(() => expect(api.setRolePermissions).toHaveBeenCalledTimes(1));
    const [roleId, keys] = vi.mocked(api.setRolePermissions).mock.calls[0];
    expect(roleId).toBe("r2");
    expect(new Set(keys)).toEqual(new Set(["catalog.read", "catalog.write"]));
  });

  test("Edit dialog offers a same-scope Inherits from picker and shows inherited permissions read-only", async () => {
    const user = userEvent.setup();
    // Two workspace roles so the picker has a same-scope option other than self.
    vi.mocked(api.listRoles).mockResolvedValue([
      ...ROLES,
      {
        id: "r4",
        scope: "workspace",
        name: "ws_base",
        description: "",
        permissionCount: 1,
        assigneeCount: 0,
        inheritedPermissions: [],
      },
    ]);
    // custom_role is a workspace role; scope its catalog perms to workspace so
    // the (scope-filtered) matrix shows them.
    vi.mocked(api.listPermissions).mockResolvedValue([
      {
        group: "Catalog",
        permissions: [
          {
            key: "catalog.read",
            description: "Read catalog metadata",
            roles: ["data_steward", "custom_role"],
            scope: "workspace",
          },
          {
            key: "catalog.write",
            description: "Write catalog metadata",
            roles: ["data_steward"],
            scope: "workspace",
          },
        ],
      },
    ]);
    // custom_role (r2) inherits catalog.write from its parent (read-only row).
    vi.mocked(api.getRole).mockResolvedValue({
      id: "r2",
      scope: "workspace",
      name: "custom_role",
      description: "A custom workspace role",
      permissionCount: 1,
      assigneeCount: 0,
      parentRoleId: null,
      inheritedPermissions: ["catalog.write"],
    });

    renderPage();
    await screen.findByText("custom_role");

    await user.click(screen.getByRole("button", { name: /Actions for custom_role/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit permissions" }));
    await screen.findByRole("heading", { name: /Edit role — custom_role/i });

    // The picker offers the same-scope role ws_base, but not the role itself
    // (data_steward is platform scope, so it must not appear either).
    const picker = await screen.findByLabelText("Inherits from");
    await user.click(picker);
    expect(await screen.findByRole("option", { name: "ws_base" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "custom_role" })).toBeNull();
    expect(screen.queryByRole("option", { name: "data_steward" })).toBeNull();
    await user.keyboard("{Escape}");

    // The inherited permission renders checked + disabled and badged "inherited".
    const inheritedCb = await screen.findByRole("checkbox", { name: "catalog.write" });
    expect(inheritedCb).toBeDisabled();
    expect(inheritedCb).toHaveAttribute("data-state", "checked");
    expect(screen.getByText("inherited")).toBeInTheDocument();
  });

  test("Edit dialog for a workspace role hides platform-scoped permissions, filters by search, and select-all-per-group toggles", async () => {
    const user = userEvent.setup();
    vi.mocked(api.setRolePermissions).mockResolvedValue(undefined);
    // A workspace group and a platform group; only the workspace one is holdable
    // by custom_role (a workspace role).
    vi.mocked(api.listPermissions).mockResolvedValue([
      {
        group: "Charts",
        permissions: [
          {
            key: "chart:read",
            description: "Read charts",
            roles: [],
            scope: "workspace",
          },
        ],
      },
      {
        group: "Engines",
        permissions: [
          {
            key: "engine:read",
            description: "Read engines",
            roles: [],
            scope: "platform",
          },
        ],
      },
    ]);
    vi.mocked(api.getRole).mockResolvedValue({
      id: "r2",
      scope: "workspace",
      name: "custom_role",
      description: "A custom workspace role",
      permissionCount: 1,
      assigneeCount: 0,
      parentRoleId: null,
      inheritedPermissions: [],
    });

    renderPage();
    await screen.findByText("custom_role");

    await user.click(screen.getByRole("button", { name: /Actions for custom_role/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit permissions" }));
    await screen.findByRole("heading", { name: /Edit role — custom_role/i });

    // The workspace permission renders; the platform group/permission does not.
    expect(await screen.findByRole("checkbox", { name: "chart:read" })).toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: "engine:read" })).toBeNull();
    expect(screen.queryByText("Engines")).toBeNull();

    // Searching for "chart" keeps chart:read visible.
    await user.type(screen.getByPlaceholderText("Search permissions…"), "chart");
    expect(screen.getByRole("checkbox", { name: "chart:read" })).toBeInTheDocument();

    // Clicking the group's select-all toggles chart:read's checkbox on.
    const chartCb = screen.getByRole("checkbox", { name: "chart:read" });
    expect(chartCb).toHaveAttribute("data-state", "unchecked");
    await user.click(screen.getByRole("checkbox", { name: "Select all Charts" }));
    expect(chartCb).toHaveAttribute("data-state", "checked");
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

  test("platform_admin's checklist is shown read-only with a superuser note", async () => {
    vi.mocked(api.listRoles).mockResolvedValue([
      {
        id: "r0",
        scope: "platform",
        name: "platform_admin",
        description: "Superuser",
        permissionCount: 0,
        assigneeCount: 1,
        inheritedPermissions: [],
      },
      ...ROLES,
    ]);
    const user = userEvent.setup();

    renderPage();
    await screen.findByText("platform_admin");

    await user.click(screen.getByRole("button", { name: /Actions for platform_admin/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit permissions" }));
    await screen.findByRole("heading", { name: /Edit role — platform_admin/i });

    expect(screen.getByText(/Superuser — bypasses all permission checks/i)).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "catalog.read" })).toBeDisabled();
  });

  test("renders an empty state when there are no roles", async () => {
    vi.mocked(api.listRoles).mockResolvedValue([]);
    renderPage();
    expect(await screen.findByText("No roles.")).toBeInTheDocument();
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

  test("the Inherits from column shows the parent role's name as a chip, and — for a root role", async () => {
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

    // The child's row renders the parent's name (base_role) in the Inherits
    // from column. base_role also appears as its own row's Name cell, so assert
    // that the parent name is present at least twice (own row + child's chip).
    expect(screen.getAllByText("base_role").length).toBeGreaterThanOrEqual(2);

    // The root role (base_role, no parentRoleId) renders an em dash placeholder.
    expect(screen.getByText("—")).toBeInTheDocument();
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
