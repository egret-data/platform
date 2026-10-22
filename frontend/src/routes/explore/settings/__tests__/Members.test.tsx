import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

// ── Mock factories ────────────────────────────────────────────────────────────

const membersMock = vi.fn();
const rolesMock = vi.fn();
const addMutate = vi.fn();
const removeMutate = vi.fn();
const authMock = vi.fn();

vi.mock("@/api/members", () => ({
  membersKey: (ws: string) => ["ws", ws, "members"],
  useWorkspaceMembers: (...args: unknown[]) => membersMock(...args),
  useWorkspaceRoles: () => rolesMock(),
  useAddMember: () => ({ mutate: addMutate, mutateAsync: addMutate, isPending: false }),
  useRemoveMember: () => ({ mutate: removeMutate, mutateAsync: removeMutate, isPending: false }),
  useWorkspaceInvites: () => ({ data: [], isLoading: false, error: null }),
  useDeleteInvite: () => ({ mutate: vi.fn(), isPending: false }),
}));

vi.mock("@/api/hooks", async () => ({
  ...(await vi.importActual<typeof import("@/api/hooks")>("@/api/hooks")),
  useAuthConfig: () => ({ data: { oidc_enabled: false } }),
}));

vi.mock("@/api/policies", () => ({
  listPrincipals: vi.fn(() => Promise.resolve([])),
}));

vi.mock("@/auth/useAuth", () => ({
  useAuth: () => authMock(),
}));

import Members from "@/routes/explore/settings/Members";

// ── Render helper ─────────────────────────────────────────────────────────────

function renderPage() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/settings/members"]}>
          <Routes>
            <Route
              path="/explore/:workspace/settings/members"
              element={<Members />}
            />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────

const alice = {
  principal_type: "user" as const,
  principal_id: "alice-id",
  role: "ws_editor",
  principal_name: "alice@egret.local",
  granted_by_name: "admin@egret.local",
  added_at: "2026-06-01T10:00:00Z",
  is_platform_admin: true,
};

const bob = {
  principal_type: "user" as const,
  principal_id: "bob-id",
  role: "ws_viewer",
  principal_name: "bob@egret.local",
  granted_by_name: "admin@egret.local",
  added_at: "2026-06-02T10:00:00Z",
  is_platform_admin: false,
};

const roles = [{ name: "ws_admin" }, { name: "ws_editor" }, { name: "ws_viewer" }];

// ── Setup ─────────────────────────────────────────────────────────────────────

beforeEach(() => {
  membersMock.mockReset();
  rolesMock.mockReset();
  addMutate.mockReset();
  removeMutate.mockReset();
  authMock.mockReset();

  membersMock.mockReturnValue({
    data: [alice, bob],
    isLoading: false,
    isError: false,
    error: null,
  });
  rolesMock.mockReturnValue({ data: roles, isLoading: false, isError: false });
  authMock.mockReturnValue({
    me: {
      user_id: "me-id",
      email: "me@egret.local",
      platform_roles: [],
      permissions: [],
    },
    isLoading: false,
  });
});

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("Members page", () => {
  it("renders member names (not UUIDs) and a platform-admin badge", () => {
    renderPage();
    expect(screen.getByText("alice@egret.local")).toBeInTheDocument();
    expect(screen.getByText("bob@egret.local")).toBeInTheDocument();
    // UUID-ish ids are not shown as the visible label
    expect(screen.queryByText("alice-id")).not.toBeInTheDocument();
    expect(screen.getByText("platform admin")).toBeInTheDocument();
  });

  it("changing a member's role calls the add mutation with the new role", async () => {
    renderPage();

    // bob is a non-admin (ws_viewer); open his inline Role select and pick ws_admin.
    fireEvent.click(screen.getByLabelText("Role for bob@egret.local"));
    fireEvent.click(await screen.findByRole("option", { name: "ws_admin" }));

    await waitFor(() => {
      expect(addMutate).toHaveBeenCalledWith(
        expect.objectContaining({
          principal_type: "user",
          principal_id: "bob-id",
          role: "ws_admin",
        }),
        expect.anything(),
      );
    });
  });

  it("locks the sole admin's role and disables their remove", async () => {
    const carol = {
      ...bob,
      principal_id: "carol-id",
      principal_name: "carol@egret.local",
      role: "ws_admin",
      is_platform_admin: false,
    };
    membersMock.mockReturnValue({
      data: [carol, bob],
      isLoading: false,
      isError: false,
      error: null,
    });
    renderPage();

    // The sole admin shows a static (locked) role pill — its role text is
    // present, but no editable Role select is rendered for them.
    expect(screen.getByText("ws_admin")).toBeInTheDocument();
    expect(
      screen.queryByLabelText("Role for carol@egret.local"),
    ).not.toBeInTheDocument();

    // Their remove action is disabled.
    const user = userEvent.setup();
    await user.click(
      screen.getByRole("button", { name: "Actions for carol@egret.local" }),
    );
    const removeItem = await screen.findByText("Remove from workspace");
    expect(removeItem.closest('[role="menuitem"]')).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  it("shows the scope clarifier", () => {
    renderPage();
    expect(screen.getByText(/apply only inside/i)).toBeInTheDocument();
  });

  it("shows a skeleton while members are loading", () => {
    membersMock.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    });
    renderPage();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
  });

  it("entering bulk mode reveals the selection checkbox column", async () => {
    const user = userEvent.setup();
    renderPage();

    // No checkboxes until bulk mode is on.
    expect(screen.queryByLabelText("Select all")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));

    expect(screen.getByLabelText("Select all")).toBeInTheDocument();
    expect(screen.getAllByLabelText("Select row")).toHaveLength(2);
  });

  it("bulk-removing a selected member calls removeMut with the composite principal", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));

    // Rows sort by member asc → [alice, bob]; select the first (alice).
    const rowChecks = screen.getAllByLabelText("Select row");
    await user.click(rowChecks[0]);

    await user.click(screen.getByRole("button", { name: "Remove" }));

    await waitFor(() => {
      expect(removeMutate).toHaveBeenCalledWith({
        principal_type: "user",
        principal_id: "alice-id",
      });
    });
  });
});
