import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import RoleDetail from "@/routes/control-plane/access/RoleDetail";
import * as api from "@/api/access";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

vi.mock("@/api/access");

const ADMIN_ME: Me = {
  user_id: "1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};
const VIEWER_ME: Me = {
  user_id: "2",
  email: "viewer@example.com",
  platform_roles: [],
  permissions: ["role:read"],
};

const ROLES: api.AccessRole[] = [
  {
    id: "r2",
    scope: "workspace",
    name: "custom_role",
    description: "A custom workspace role",
    permissionCount: 1,
    assigneeCount: 0,
    inheritedPermissions: [],
  },
  {
    id: "pa",
    scope: "platform",
    name: "platform_admin",
    description: "",
    permissionCount: 0,
    assigneeCount: 3,
    inheritedPermissions: [],
  },
];

const PERMISSIONS: api.PermissionCatalogGroup[] = [
  {
    group: "Datasets",
    permissions: [
      { key: "dataset:read", description: "Read datasets", roles: ["custom_role"], scope: "workspace" },
      { key: "dataset:manage", description: "Manage datasets", roles: [], scope: "workspace" },
    ],
  },
  {
    group: "Catalog",
    // Platform-scoped: must never appear for a workspace role.
    permissions: [
      { key: "catalog.read", description: "Read catalog", roles: [], scope: "platform" },
    ],
  },
];

function renderDetail(id = "r2", me: Me | null = ADMIN_ME) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/control-plane/roles/${id}`]}>
          <Routes>
            <Route path="/control-plane/roles/:id" element={<RoleDetail />} />
            <Route path="/control-plane/roles" element={<div>roles list</div>} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("RoleDetail", () => {
  beforeEach(() => {
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    vi.mocked(api.listPermissions).mockResolvedValue(PERMISSIONS);
    vi.mocked(api.getRole).mockImplementation(async (id) => {
      const found = ROLES.find((r) => r.id === id) ?? ROLES[0];
      return { ...found };
    });
    vi.mocked(api.setRolePermissions).mockResolvedValue(undefined);
    vi.mocked(api.updateRole).mockResolvedValue(ROLES[0]);
  });

  test("shows the identity, the granted permission, and only scope-matching perms", async () => {
    renderDetail("r2");

    expect(await screen.findByRole("heading", { name: "custom_role" })).toBeInTheDocument();
    expect(screen.getByText(/Workspace role/)).toBeInTheDocument();

    // dataset:read is granted (its catalogue entry lists custom_role) and removable.
    expect(await screen.findByText("dataset:read")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove dataset:read" })).toBeInTheDocument();

    // A platform-scoped permission never shows for a workspace role.
    expect(screen.queryByText("catalog.read")).not.toBeInTheDocument();
  });

  test("adding a permission via the category picker PUTs the full new set", async () => {
    const user = userEvent.setup();
    renderDetail("r2");
    await screen.findByText("dataset:read");

    // Choose the category, then the permission in it — no full-catalogue dump.
    await user.click(screen.getByRole("combobox", { name: "Permission category" }));
    await user.click(await screen.findByRole("option", { name: "Datasets" }));

    await user.click(screen.getByRole("combobox", { name: "Permission to add" }));
    await user.click(await screen.findByRole("option", { name: /dataset:manage/ }));

    // Selecting doesn't apply — an explicit Add confirms it.
    expect(api.setRolePermissions).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: /^Add$/ }));

    await waitFor(() =>
      expect(api.setRolePermissions).toHaveBeenCalledWith(
        "r2",
        expect.arrayContaining(["dataset:read", "dataset:manage"]),
      ),
    );
  });

  test("removing a permission PUTs the set without it", async () => {
    const user = userEvent.setup();
    renderDetail("r2");

    await user.click(await screen.findByRole("button", { name: "Remove dataset:read" }));

    await waitFor(() => expect(api.setRolePermissions).toHaveBeenCalledWith("r2", []));
  });

  test("editing the description auto-saves on blur", async () => {
    const user = userEvent.setup();
    renderDetail("r2");

    const desc = await screen.findByLabelText("Description");
    await user.clear(desc);
    await user.type(desc, "Updated description");
    await user.tab(); // blur

    await waitFor(() =>
      expect(api.updateRole).toHaveBeenCalledWith("r2", "Updated description", null),
    );
  });

  test("inherited permissions render read-only with no remove control", async () => {
    vi.mocked(api.getRole).mockResolvedValue({
      ...ROLES[0],
      inheritedPermissions: ["dataset:manage"],
    });
    renderDetail("r2");

    expect(await screen.findByText("dataset:manage")).toBeInTheDocument();
    expect(screen.getByText("inherited")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Remove dataset:manage" }),
    ).not.toBeInTheDocument();
  });

  test("the superuser role shows a bypass note and no editor", async () => {
    renderDetail("pa");

    expect(
      await screen.findByText(/bypasses all permission checks/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("combobox", { name: "Permission category" }),
    ).not.toBeInTheDocument();
  });

  test("a non-admin viewer gets a read-only page (no add or remove)", async () => {
    renderDetail("r2", VIEWER_ME);
    await screen.findByText("dataset:read");

    expect(
      screen.queryByRole("combobox", { name: "Permission category" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Remove dataset:read" }),
    ).not.toBeInTheDocument();
    expect(screen.getByLabelText("Description")).toBeDisabled();
  });
});
