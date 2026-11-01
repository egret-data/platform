import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import GroupDetail from "@/routes/control-plane/access/GroupDetail";
import * as api from "@/api/access";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import { ApiError } from "@/api/client";
import { toast } from "sonner";

vi.mock("@/api/access");
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const ADMIN_ME: Me = {
  user_id: "1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};

const GROUPS: api.AccessGroup[] = [
  { id: "g1", name: "Engineering", source: "local", memberCount: 1, roles: ["data_steward"] },
  { id: "g2", name: "SSO-Everyone", source: "oidc", memberCount: 5, roles: [] },
];

const ROLES: api.AccessRole[] = [
  { id: "r1", scope: "platform", name: "data_steward", description: "", permissionCount: 2, assigneeCount: 1, inheritedPermissions: [] },
  { id: "r2", scope: "platform", name: "auditor", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
];

const USERS: api.AccessUser[] = [
  { id: "u1", email: "alice@example.com", name: "Alice", provider: "local", status: "active", roles: [], effectiveRoles: [], groups: [] },
  { id: "u2", email: "bob@example.com", name: "Bob", provider: "local", status: "active", roles: [], effectiveRoles: [], groups: [] },
];

const MEMBERS: api.GroupMember[] = [{ id: "u1", email: "alice@example.com", name: "Alice" }];

function renderDetail(id = "g1", me: Me | null = ADMIN_ME) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/control-plane/groups/${id}`]}>
          <Routes>
            <Route path="/control-plane/groups/:id" element={<GroupDetail />} />
            <Route path="/control-plane/groups" element={<div>groups list</div>} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("GroupDetail", () => {
  beforeEach(() => {
    vi.mocked(api.listGroups).mockResolvedValue(GROUPS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    vi.mocked(api.listUsers).mockResolvedValue(USERS);
    vi.mocked(api.getGroupMembers).mockResolvedValue(MEMBERS);
    vi.mocked(api.addMember).mockResolvedValue(undefined);
    vi.mocked(api.removeMember).mockResolvedValue(undefined);
    vi.mocked(api.assignGroupRole).mockResolvedValue(undefined);
    vi.mocked(api.unassignGroupRole).mockResolvedValue(undefined);
    vi.mocked(api.renameGroup).mockResolvedValue(GROUPS[0]);
    vi.mocked(api.deleteGroup).mockResolvedValue(undefined);
    vi.mocked(toast.success).mockClear();
    vi.mocked(toast.error).mockClear();
  });

  test("renders a local group with its member and assigned role", async () => {
    renderDetail("g1");

    expect(await screen.findByRole("heading", { name: "Engineering" })).toBeInTheDocument();
    expect(await screen.findByText("alice@example.com")).toBeInTheDocument();
    expect(screen.getByText("data_steward")).toBeInTheDocument();
  });

  test("adding a member: search → pick → Add calls addMember", async () => {
    const user = userEvent.setup();
    renderDetail("g1");
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("combobox", { name: "User to add" }));
    await user.type(screen.getByPlaceholderText("Search by email…"), "bob");
    await user.click(await screen.findByText("bob@example.com"));

    const membersSection = screen
      .getByRole("heading", { name: "Members" })
      .closest("section") as HTMLElement;
    const addBtn = within(membersSection).getByRole("button", { name: /^Add$/ });
    expect(api.addMember).not.toHaveBeenCalled();
    await user.click(addBtn);

    await waitFor(() => expect(api.addMember).toHaveBeenCalledWith("g1", "u2"));
  });

  test("a refused add (409 on an OIDC-synced group) shows the server's reason", async () => {
    // The page hides Add on a group it knows is OIDC-synced; a stale list can
    // still offer it, and the server refuses (design R4). The reason must
    // reach the admin rather than a generic failure.
    const reason =
      "conflict: Cannot add members to an OIDC-synced group; its membership is owned by the identity provider";
    vi.mocked(api.addMember).mockRejectedValueOnce(new ApiError(409, reason));
    const user = userEvent.setup();
    renderDetail("g1");
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("combobox", { name: "User to add" }));
    await user.type(screen.getByPlaceholderText("Search by email…"), "bob");
    await user.click(await screen.findByText("bob@example.com"));
    const membersSection = screen
      .getByRole("heading", { name: "Members" })
      .closest("section") as HTMLElement;
    await user.click(within(membersSection).getByRole("button", { name: /^Add$/ }));

    await waitFor(() => expect(toast.error).toHaveBeenCalledWith(reason));
    expect(toast.success).not.toHaveBeenCalledWith("Member added");
  });

  test("removing a member calls removeMember", async () => {
    const user = userEvent.setup();
    renderDetail("g1");

    await user.click(await screen.findByRole("button", { name: "Remove alice@example.com" }));
    await waitFor(() => expect(api.removeMember).toHaveBeenCalledWith("g1", "u1"));
  });

  test("assigning a role to the group calls assignGroupRole", async () => {
    const user = userEvent.setup();
    renderDetail("g1");
    await screen.findByText("data_steward");

    await user.click(screen.getByRole("combobox", { name: "Role scope" }));
    await user.click(await screen.findByRole("option", { name: "Platform" }));
    await user.click(screen.getByRole("combobox", { name: "Role to add" }));
    await user.click(await screen.findByRole("option", { name: "auditor" }));

    const rolesSection = screen
      .getByRole("heading", { name: "Roles" })
      .closest("section") as HTMLElement;
    await user.click(within(rolesSection).getByRole("button", { name: /^Add$/ }));

    await waitFor(() => expect(api.assignGroupRole).toHaveBeenCalledWith("g1", "r2"));
  });

  test("an OIDC group is read-only for name and membership but roles stay editable", async () => {
    renderDetail("g2");

    await screen.findByRole("heading", { name: "SSO-Everyone" });
    expect(screen.getByText(/can't be changed here/i)).toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toBeDisabled();
    // No member add control for a synced group…
    expect(screen.queryByRole("combobox", { name: "User to add" })).not.toBeInTheDocument();
    // …but roles can still be assigned.
    expect(screen.getByRole("combobox", { name: "Role scope" })).toBeInTheDocument();
  });

  test("renaming the group on blur calls renameGroup", async () => {
    const user = userEvent.setup();
    renderDetail("g1");

    const nameField = await screen.findByLabelText("Name");
    await user.clear(nameField);
    await user.type(nameField, "Eng Team");
    await user.tab();

    await waitFor(() => expect(api.renameGroup).toHaveBeenCalledWith("g1", "Eng Team"));
  });
});
