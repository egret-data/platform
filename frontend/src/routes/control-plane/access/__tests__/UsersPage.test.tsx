import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import UsersPage from "@/routes/control-plane/access/UsersPage";
import * as api from "@/api/access";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

vi.mock("@/api/access");

// Role assignment is platform_admin-only; default to a platform_admin `me`
// so existing assertions (which exercise "Assign roles") keep passing.
// Tests that care about the non-admin view pass their own `me`.
const ADMIN_ME: Me = {
  user_id: "1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};

const USERS: api.AccessUser[] = [
  {
    id: "u1",
    email: "alice@example.com",
    name: "Alice",
    provider: "local",
    status: "active",
    roles: ["Admin"],
    groups: [],
  },
  {
    id: "u2",
    email: "bob@example.com",
    name: "Bob",
    provider: "oidc",
    status: "disabled",
    roles: [],
    groups: ["Engineering"],
  },
];

const ROLES: api.AccessRole[] = [
  { id: "r1", scope: "platform", name: "Admin", description: "", permissionCount: 0, assigneeCount: 1, inheritedPermissions: [] },
  { id: "r2", scope: "platform", name: "Analyst", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
];

function renderPage(me: Me | null = ADMIN_ME) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter>
          <UsersPage />
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("UsersPage", () => {
  beforeEach(() => {
    vi.mocked(api.listUsers).mockResolvedValue(USERS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
  });

  test("renders rows from the mocked user list", async () => {
    renderPage();
    expect(await screen.findByText("alice@example.com")).toBeInTheDocument();
    expect(screen.getByText("bob@example.com")).toBeInTheDocument();
    expect(screen.getByText("Alice")).toBeInTheDocument();
    expect(screen.getByText("Admin")).toBeInTheDocument();
    expect(screen.getByText("active")).toBeInTheDocument();
    expect(screen.getByText("disabled")).toBeInTheDocument();
  });

  test("renders a user's groups as chips in their own Groups column", async () => {
    renderPage();
    await screen.findByText("bob@example.com");

    // Bob belongs to "Engineering"; the chip renders as its own column, no
    // longer as a sub-line under the Name cell.
    expect(screen.getByText("Engineering")).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Groups" })).toBeInTheDocument();

    // Alice has no groups, so her Groups cell falls back to a dash.
    const aliceRow = screen.getByText("alice@example.com").closest("tr");
    expect(aliceRow).not.toBeNull();
    expect(within(aliceRow as HTMLElement).getByText("—")).toBeInTheDocument();
  });

  test("renders an empty state when there are no users", async () => {
    vi.mocked(api.listUsers).mockResolvedValue([]);
    renderPage();
    expect(await screen.findByText("No users yet.")).toBeInTheDocument();
  });

  test("New user dialog calls createUser with the entered fields", async () => {
    const user = userEvent.setup();
    vi.mocked(api.createUser).mockResolvedValue({
      id: "u3",
      email: "carol@example.com",
      name: "Carol",
      provider: "local",
      status: "active",
      roles: [],
      groups: [],
    });

    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /New user/i }));
    await screen.findByRole("heading", { name: /New user/i });

    await user.type(screen.getByLabelText("Email"), "carol@example.com");
    await user.type(screen.getByLabelText("Name"), "Carol");
    await user.type(screen.getByLabelText("Password"), "s3cret!");
    await user.click(screen.getByRole("button", { name: /^Create$/i }));

    await waitFor(() =>
      expect(api.createUser).toHaveBeenCalledWith({
        email: "carol@example.com",
        name: "Carol",
        password: "s3cret!",
      }),
    );
  });

  test("Assign roles dialog diffs the selection into assign/unassign calls", async () => {
    const user = userEvent.setup();
    vi.mocked(api.assignUserRole).mockResolvedValue(undefined);
    vi.mocked(api.unassignUserRole).mockResolvedValue(undefined);

    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Assign roles" }));
    await screen.findByRole("heading", { name: /Assign roles/i });

    // Alice currently has "Admin" as a chip; remove it.
    await user.click(screen.getByRole("button", { name: "remove Admin" }));

    // Open the combobox and add "Analyst" from the suggestion list. The
    // combobox trigger is opened with a raw click event rather than
    // userEvent.click on the nested input: userEvent's full pointerdown /
    // focus / mouseup sequence interacts badly with the surrounding Dialog's
    // focus trap in jsdom (focus bounces back to the trigger before the
    // popover's open state settles), whereas a plain click event does not.
    const roleInput = screen.getByLabelText("Add role");
    fireEvent.click(roleInput.closest('[role="button"]') as HTMLElement);
    await user.click(await screen.findByText("Analyst"));

    await user.click(screen.getByRole("button", { name: /^Save$/i }));

    await waitFor(() => expect(api.assignUserRole).toHaveBeenCalledWith("u1", "r2"));
    await waitFor(() => expect(api.unassignUserRole).toHaveBeenCalledWith("u1", "r1"));
  });

  test("a non-admin user:read viewer can still create/edit users but not assign roles", async () => {
    const NON_ADMIN_ME: Me = {
      user_id: "2",
      email: "viewer@example.com",
      platform_roles: [],
      permissions: ["user:read", "user:create", "user:update"],
    };
    const user = userEvent.setup();
    renderPage(NON_ADMIN_ME);

    expect(await screen.findByText("alice@example.com")).toBeInTheDocument();

    // Delegatable controls remain.
    expect(screen.getByRole("button", { name: /New user/i })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    expect(await screen.findByRole("menuitem", { name: "Edit" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Assign roles" })).not.toBeInTheDocument();
  });
});
