import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import GroupsPage from "@/routes/control-plane/access/GroupsPage";
import * as api from "@/api/access";

vi.mock("@/api/access");

const GROUPS: api.AccessGroup[] = [
  { id: "g1", name: "Engineering", source: "local", memberCount: 3, roles: ["Admin"] },
  { id: "g2", name: "SSO-Everyone", source: "oidc", memberCount: 5, roles: [] },
];

const ROLES: api.AccessRole[] = [
  { id: "r1", scope: "platform", name: "Admin", description: "", permissionCount: 0, assigneeCount: 1 },
  { id: "r2", scope: "platform", name: "Analyst", description: "", permissionCount: 0, assigneeCount: 0 },
];

const USERS: api.AccessUser[] = [
  { id: "u1", email: "alice@example.com", name: "Alice", provider: "local", status: "active", roles: [], groups: [] },
];

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <GroupsPage />
    </QueryClientProvider>,
  );
}

describe("GroupsPage", () => {
  beforeEach(() => {
    vi.mocked(api.listGroups).mockResolvedValue(GROUPS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    vi.mocked(api.listUsers).mockResolvedValue(USERS);
  });

  test("renders rows from the mocked group list", async () => {
    renderPage();
    expect(await screen.findByText("Engineering")).toBeInTheDocument();
    expect(screen.getByText("SSO-Everyone")).toBeInTheDocument();
    expect(screen.getByText("local")).toBeInTheDocument();
    expect(screen.getByText("oidc")).toBeInTheDocument();
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

    await user.type(screen.getByLabelText("Name"), "Data");
    await user.click(screen.getByRole("button", { name: /^Create$/i }));

    await waitFor(() => expect(api.createGroup).toHaveBeenCalledWith({ name: "Data" }));
  });

  test("Assign roles dialog diffs the selection into assign/unassign calls", async () => {
    const user = userEvent.setup();
    vi.mocked(api.assignGroupRole).mockResolvedValue(undefined);
    vi.mocked(api.unassignGroupRole).mockResolvedValue(undefined);

    renderPage();
    await screen.findByText("Engineering");

    await user.click(screen.getByRole("button", { name: /Assign roles to Engineering/i }));
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

  test("an oidc group has rename, delete, and manage-members disabled but roles enabled", async () => {
    renderPage();
    await screen.findByText("SSO-Everyone");

    expect(screen.getByRole("button", { name: /Rename SSO-Everyone/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Delete SSO-Everyone/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Manage members of SSO-Everyone/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /Assign roles to SSO-Everyone/i })).not.toBeDisabled();

    // Local group's actions remain enabled.
    expect(screen.getByRole("button", { name: /Rename Engineering/i })).not.toBeDisabled();
    expect(screen.getByRole("button", { name: /Delete Engineering/i })).not.toBeDisabled();
    expect(screen.getByRole("button", { name: /Manage members of Engineering/i })).not.toBeDisabled();
  });
});
