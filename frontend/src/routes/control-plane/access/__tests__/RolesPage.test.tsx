import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import RolesPage from "@/routes/control-plane/access/RolesPage";
import * as api from "@/api/access";

vi.mock("@/api/access");

const ROLES: api.AccessRole[] = [
  {
    id: "r1",
    scope: "platform",
    name: "data_steward",
    description: "Manages data governance",
    permissionCount: 2,
    assigneeCount: 1,
  },
  {
    id: "r2",
    scope: "workspace",
    name: "custom_role",
    description: "A custom workspace role",
    permissionCount: 1,
    assigneeCount: 0,
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
      },
      {
        key: "catalog.write",
        description: "Write catalog metadata",
        roles: ["data_steward"],
      },
    ],
  },
];

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <RolesPage />
    </QueryClientProvider>,
  );
}

describe("RolesPage", () => {
  beforeEach(() => {
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    vi.mocked(api.listPermissions).mockResolvedValue(PERMISSIONS);
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

  test("platform_admin's checklist is shown read-only with a superuser note", async () => {
    vi.mocked(api.listRoles).mockResolvedValue([
      {
        id: "r0",
        scope: "platform",
        name: "platform_admin",
        description: "Superuser",
        permissionCount: 0,
        assigneeCount: 1,
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
});
