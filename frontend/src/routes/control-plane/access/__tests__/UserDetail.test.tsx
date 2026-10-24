import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import UserDetail from "@/routes/control-plane/access/UserDetail";
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
  permissions: ["user:read"],
};

const USERS: api.AccessUser[] = [
  {
    id: "u1",
    email: "alice@example.com",
    name: "Alice",
    provider: "local",
    status: "active",
    roles: ["data_steward"],
    groups: ["Engineering"],
    username: "alice_eng",
  },
  {
    id: "u2",
    email: "noname@example.com",
    name: "Nora",
    provider: "oidc",
    status: "active",
    roles: ["platform_admin"],
    groups: [],
    username: null,
  },
  {
    id: "u3",
    email: "off@example.com",
    name: "Otto",
    provider: "local",
    status: "disabled",
    roles: [],
    groups: [],
    username: "otto",
  },
];

const ROLES: api.AccessRole[] = [
  { id: "r1", scope: "platform", name: "data_steward", description: "", permissionCount: 2, assigneeCount: 1, inheritedPermissions: [] },
  { id: "r2", scope: "platform", name: "auditor", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
];

function renderDetail(id = "u1", me: Me | null = ADMIN_ME) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/control-plane/users/${id}`]}>
          <Routes>
            <Route path="/control-plane/users/:id" element={<UserDetail />} />
            <Route path="/control-plane/users" element={<div>users list</div>} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("UserDetail", () => {
  beforeEach(() => {
    vi.mocked(api.listUsers).mockResolvedValue(USERS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    vi.mocked(api.updateUser).mockResolvedValue(USERS[0]);
    vi.mocked(api.assignUserRole).mockResolvedValue(undefined);
    vi.mocked(api.unassignUserRole).mockResolvedValue(undefined);
    vi.mocked(api.deleteUser).mockResolvedValue(undefined);
  });

  test("renders identity, the assigned role, and group membership", async () => {
    renderDetail("u1");

    expect(await screen.findByRole("heading", { name: "Alice" })).toBeInTheDocument();
    expect(screen.getByText("alice@example.com")).toBeInTheDocument();
    expect(await screen.findByText("data_steward")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove data_steward" })).toBeInTheDocument();
    expect(screen.getByText("Engineering")).toBeInTheDocument();
  });

  test("assigning a role via scope → role → Add calls assignUserRole", async () => {
    const user = userEvent.setup();
    renderDetail("u1");
    await screen.findByText("data_steward");

    await user.click(screen.getByRole("combobox", { name: "Role scope" }));
    await user.click(await screen.findByRole("option", { name: "Platform" }));
    await user.click(screen.getByRole("combobox", { name: "Role to add" }));
    await user.click(await screen.findByRole("option", { name: "auditor" }));

    expect(api.assignUserRole).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: /^Add$/ }));

    await waitFor(() => expect(api.assignUserRole).toHaveBeenCalledWith("u1", "r2"));
  });

  test("removing a role calls unassignUserRole", async () => {
    const user = userEvent.setup();
    renderDetail("u1");

    await user.click(await screen.findByRole("button", { name: "Remove data_steward" }));
    await waitFor(() => expect(api.unassignUserRole).toHaveBeenCalledWith("u1", "r1"));
  });

  test("editing the name auto-saves on blur", async () => {
    const user = userEvent.setup();
    renderDetail("u1");

    const nameField = await screen.findByLabelText("Name");
    await user.clear(nameField);
    await user.type(nameField, "Alice B");
    await user.tab();

    await waitFor(() => expect(api.updateUser).toHaveBeenCalledWith("u1", { name: "Alice B" }));
  });

  // KI-209: Disable signs the user out at once, so it asks first.
  test("Disable asks for confirmation, then disables", async () => {
    const user = userEvent.setup();
    renderDetail("u1");
    await screen.findByRole("heading", { name: "Alice" });

    await user.click(screen.getByRole("button", { name: /^Disable$/ }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent(/signed out/i);
    expect(api.updateUser).not.toHaveBeenCalledWith("u1", { status: "disabled" });

    await user.click(within(dialog).getByRole("button", { name: "Disable user" }));
    await waitFor(() => expect(api.updateUser).toHaveBeenCalledWith("u1", { status: "disabled" }));
  });

  test("cancelling the Disable confirmation changes nothing", async () => {
    vi.mocked(api.updateUser).mockClear();
    const user = userEvent.setup();
    renderDetail("u1");
    await screen.findByRole("heading", { name: "Alice" });

    await user.click(screen.getByRole("button", { name: /^Disable$/ }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    expect(api.updateUser).not.toHaveBeenCalledWith("u1", { status: "disabled" });
  });

  test("a non-admin viewer gets a read-only page", async () => {
    renderDetail("u1", VIEWER_ME);
    await screen.findByText("data_steward");

    expect(screen.queryByRole("combobox", { name: "Role scope" })).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Remove data_steward" }),
    ).not.toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toBeDisabled();
    expect(screen.queryByRole("button", { name: /^Disable$/ })).not.toBeInTheDocument();
  });

  // Username takeover (spec 2026-10-23 §4.4): a delete retires the engine
  // username for good, so the dialog names it and steers to Disable.
  test("the delete dialog names the username it retires", async () => {
    const user = userEvent.setup();
    renderDetail("u1");
    await screen.findByRole("heading", { name: "Alice" });

    await user.click(screen.getByRole("button", { name: /^Delete$/ }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent(
      "Their engine username alice_eng is retired for good: nobody can be given it again",
    );
    expect(within(dialog).getByText("alice_eng").tagName).toBe("STRONG");
    expect(dialog).toHaveTextContent(/disable them instead/);
    expect(dialog).not.toHaveTextContent("They have no engine username yet.");
  });

  test("the delete dialog of a user with no username says so", async () => {
    const user = userEvent.setup();
    renderDetail("u2");
    await screen.findByRole("heading", { name: "Nora" });

    await user.click(screen.getByRole("button", { name: /^Delete$/ }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("They have no engine username yet.");
    expect(dialog).not.toHaveTextContent("is retired for good");
    expect(dialog).toHaveTextContent(/disable them instead/);
  });

  test("Disable instead opens the disable confirmation and never deletes", async () => {
    vi.mocked(api.updateUser).mockClear();
    vi.mocked(api.deleteUser).mockClear();
    const user = userEvent.setup();
    renderDetail("u1");
    await screen.findByRole("heading", { name: "Alice" });

    await user.click(screen.getByRole("button", { name: /^Delete$/ }));
    const deleteDialog = await screen.findByRole("alertdialog");
    await user.click(within(deleteDialog).getByRole("button", { name: "Disable instead" }));

    const disableDialog = await screen.findByRole("alertdialog");
    expect(
      within(disableDialog).getByRole("heading", { name: "Disable alice@example.com?" }),
    ).toBeInTheDocument();
    await user.click(within(disableDialog).getByRole("button", { name: "Disable user" }));
    await waitFor(() => expect(api.updateUser).toHaveBeenCalledWith("u1", { status: "disabled" }));
    expect(api.deleteUser).not.toHaveBeenCalled();
  });

  test("a disabled user's delete dialog offers no Disable instead", async () => {
    const user = userEvent.setup();
    renderDetail("u3");
    await screen.findByRole("heading", { name: "Otto" });

    await user.click(screen.getByRole("button", { name: /^Delete$/ }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Their engine username otto is retired for good");
    expect(within(dialog).queryByRole("button", { name: "Disable instead" })).not.toBeInTheDocument();
  });

  test("the disable dialog says the username stays theirs", async () => {
    const user = userEvent.setup();
    renderDetail("u1");
    await screen.findByRole("heading", { name: "Alice" });

    await user.click(screen.getByRole("button", { name: /^Disable$/ }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Their engine username stays theirs.");
    expect(dialog).not.toHaveTextContent("lose all-access on engines");
  });

  test("disabling a platform admin says they lose all-access on engines", async () => {
    const user = userEvent.setup();
    renderDetail("u2");
    await screen.findByRole("heading", { name: "Nora" });

    await user.click(screen.getByRole("button", { name: /^Disable$/ }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Their engine username stays theirs.");
    expect(dialog).toHaveTextContent("While disabled they lose all-access on engines.");
  });
});
