import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import GroupsPage from "@/routes/control-plane/access/GroupsPage";
import * as api from "@/api/access";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

vi.mock("@/api/access");

// Assigning roles to a group and managing its membership are
// platform_admin-only; default to a platform_admin `me` so existing
// assertions (which exercise those controls) keep passing. Tests that care
// about the non-admin view pass their own `me`.
const ADMIN_ME: Me = {
  user_id: "1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};

const GROUPS: api.AccessGroup[] = [
  { id: "g1", name: "Engineering", source: "local", memberCount: 3, roles: ["Admin"] },
  { id: "g2", name: "SSO-Everyone", source: "oidc", memberCount: 5, roles: [] },
];

const ROLES: api.AccessRole[] = [
  { id: "r1", scope: "platform", name: "Admin", description: "", permissionCount: 0, assigneeCount: 1, inheritedPermissions: [] },
  { id: "r2", scope: "platform", name: "Analyst", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
];

const USERS: api.AccessUser[] = [
  { id: "u1", email: "alice@example.com", name: "Alice", provider: "local", status: "active", roles: [], effectiveRoles: [], groups: [] },
  { id: "u2", email: "bob@example.com", name: "Bob", provider: "local", status: "active", roles: [], effectiveRoles: [], groups: [] },
];

const GROUP_MEMBERS: api.GroupMember[] = [
  { id: "u1", email: "alice@example.com", name: "Alice" },
];

function renderPage(me: Me | null = ADMIN_ME) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter>
          <GroupsPage />
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("GroupsPage", () => {
  beforeEach(() => {
    vi.mocked(api.listGroups).mockResolvedValue(GROUPS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    vi.mocked(api.listUsers).mockResolvedValue(USERS);
    vi.mocked(api.getGroupMembers).mockResolvedValue(GROUP_MEMBERS);
  });

  test("shows a skeleton while the group list is loading", async () => {
    // A promise that never resolves keeps the query in its loading state.
    vi.mocked(api.listGroups).mockReturnValue(new Promise<api.AccessGroup[]>(() => {}));
    renderPage();
    expect(await screen.findByTestId("list-skeleton")).toBeInTheDocument();
    expect(screen.queryByText("No groups.")).not.toBeInTheDocument();
  });

  test("renders rows from the mocked group list", async () => {
    renderPage();
    expect(await screen.findByText("Engineering")).toBeInTheDocument();
    expect(screen.getByText("SSO-Everyone")).toBeInTheDocument();
    // Source is folded into the identity cell as a secondary line.
    expect(screen.getByText("Local group")).toBeInTheDocument();
    expect(screen.getByText("Synced from OIDC")).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();
    expect(screen.getByText("5")).toBeInTheDocument();
    expect(screen.getByText("Admin")).toBeInTheDocument();
  });

  test("New group dialog calls createGroup with the entered name", async () => {
    const user = userEvent.setup();
    vi.mocked(api.createGroup).mockResolvedValue({
      id: "g3",
      name: "Data",
      source: "local",
      memberCount: 0,
      roles: [],
    });

    renderPage();
    await screen.findByText("Engineering");

    await user.click(screen.getByRole("button", { name: /New group/i }));
    await screen.findByRole("heading", { name: /New group/i });

    // Create is gated until a name is entered.
    const create = screen.getByRole("button", { name: /^Create$/i });
    expect(create).toBeDisabled();

    await user.type(screen.getByLabelText("Name"), "Data");
    expect(create).toBeEnabled();
    await user.click(create);

    await waitFor(() => expect(api.createGroup).toHaveBeenCalledWith({ name: "Data" }));
  });

  test("Assign roles dialog diffs the selection into assign/unassign calls", async () => {
    const user = userEvent.setup();
    vi.mocked(api.assignGroupRole).mockResolvedValue(undefined);
    vi.mocked(api.unassignGroupRole).mockResolvedValue(undefined);

    renderPage();
    await screen.findByText("Engineering");

    await user.click(screen.getByRole("button", { name: /Actions for Engineering/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Assign roles" }));
    await screen.findByRole("heading", { name: /Assign roles/i });

    await user.click(screen.getByRole("button", { name: "remove Admin" }));

    // See UsersPage.test.tsx: opening the combobox popover uses a raw click
    // event rather than userEvent.click, to avoid a focus-trap/portal timing
    // conflict between the surrounding Dialog and the Popover in jsdom.
    const roleInput = screen.getByLabelText("Add role");
    fireEvent.click(roleInput.closest('[role="button"]') as HTMLElement);
    await user.click(await screen.findByText("Analyst"));

    await user.click(screen.getByRole("button", { name: /^Save$/i }));

    await waitFor(() => expect(api.assignGroupRole).toHaveBeenCalledWith("g1", "r2"));
    await waitFor(() => expect(api.unassignGroupRole).toHaveBeenCalledWith("g1", "r1"));
  });

  test("Manage members dialog prefills current members and diffs the selection into add/remove calls", async () => {
    const user = userEvent.setup();
    vi.mocked(api.addMember).mockResolvedValue(undefined);
    vi.mocked(api.removeMember).mockResolvedValue(undefined);

    renderPage();
    await screen.findByText("Engineering");

    await user.click(screen.getByRole("button", { name: /Actions for Engineering/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Manage members" }));
    await screen.findByRole("heading", { name: /Manage members/i });

    // Prefilled with the fetched current member (alice), shown as a removable chip.
    await screen.findByText("alice@example.com");

    // Remove the existing member, add a new one.
    await user.click(screen.getByRole("button", { name: "remove alice@example.com" }));

    const memberInput = screen.getByLabelText("Add member");
    fireEvent.click(memberInput.closest('[role="button"]') as HTMLElement);
    await user.click(await screen.findByText("Bob"));

    await user.click(screen.getByRole("button", { name: /^Save$/i }));

    await waitFor(() => expect(api.addMember).toHaveBeenCalledWith("g1", "u2"));
    await waitFor(() => expect(api.removeMember).toHaveBeenCalledWith("g1", "u1"));
  });

  test("clicking the member count opens a read-only members dialog", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("Engineering");

    await user.click(screen.getByRole("button", { name: "View members of Engineering" }));
    await screen.findByRole("heading", { name: /Members — Engineering/i });

    expect(await screen.findByText("alice@example.com")).toBeInTheDocument();
    expect(api.getGroupMembers).toHaveBeenCalledWith("g1");
  });

  test("renders an empty state when there are no groups", async () => {
    vi.mocked(api.listGroups).mockResolvedValue([]);
    renderPage();
    expect(await screen.findByText("No groups yet")).toBeInTheDocument();
  });

  test("typing in the search box narrows the rows by name", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("Engineering");
    expect(screen.getByText("SSO-Everyone")).toBeInTheDocument();

    await user.type(screen.getByPlaceholderText("Search groups…"), "Engineer");

    expect(screen.getByText("Engineering")).toBeInTheDocument();
    expect(screen.queryByText("SSO-Everyone")).not.toBeInTheDocument();
  });

  test("the summary strip shows the group count", async () => {
    renderPage();
    await screen.findByText("Engineering");
    const strip = screen.getByText("groups").closest("span")?.parentElement as HTMLElement;
    expect(within(strip).getByText("2")).toBeInTheDocument();
    expect(within(strip).getByText("groups")).toBeInTheDocument();
  });

  test("the Source facet filters to local or oidc groups", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("Engineering");

    await user.click(screen.getByRole("combobox", { name: /Source/i }));
    await user.click(await screen.findByRole("option", { name: "OIDC" }));

    expect(screen.getByText("SSO-Everyone")).toBeInTheDocument();
    expect(screen.queryByText("Engineering")).not.toBeInTheDocument();
  });

  test("an oidc group has rename, delete, and manage-members disabled but roles enabled", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("SSO-Everyone");

    await user.click(screen.getByRole("button", { name: /Actions for SSO-Everyone/i }));
    expect(await screen.findByRole("menuitem", { name: "Rename" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(screen.getByRole("menuitem", { name: "Delete" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(screen.getByRole("menuitem", { name: "Manage members" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(screen.getByRole("menuitem", { name: "Assign roles" })).not.toHaveAttribute(
      "aria-disabled",
    );
    await user.keyboard("{Escape}");

    // Local group's actions remain enabled.
    await user.click(screen.getByRole("button", { name: /Actions for Engineering/i }));
    expect(await screen.findByRole("menuitem", { name: "Rename" })).not.toHaveAttribute(
      "aria-disabled",
    );
    expect(screen.getByRole("menuitem", { name: "Delete" })).not.toHaveAttribute(
      "aria-disabled",
    );
    expect(screen.getByRole("menuitem", { name: "Manage members" })).not.toHaveAttribute(
      "aria-disabled",
    );
  });

  test("a non-admin group:read viewer can still create/rename/delete groups but not assign roles or manage members", async () => {
    const NON_ADMIN_ME: Me = {
      user_id: "2",
      email: "viewer@example.com",
      platform_roles: [],
      permissions: ["group:read", "group:create", "group:update", "group:delete"],
    };
    const user = userEvent.setup();
    renderPage(NON_ADMIN_ME);

    expect(await screen.findByText("Engineering")).toBeInTheDocument();

    // Delegatable controls remain.
    expect(screen.getByRole("button", { name: /New group/i })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Actions for Engineering/i }));
    expect(await screen.findByRole("menuitem", { name: "Rename" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Delete" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Assign roles" })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Manage members" })).not.toBeInTheDocument();

    // The read-only members drill-in still works.
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "View members of Engineering" }));
    expect(await screen.findByText("alice@example.com")).toBeInTheDocument();
  });
});
