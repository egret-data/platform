import { beforeEach, describe, expect, test, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

import { MembersPanel } from "@/components/access/MembersPanel";
import type { WorkspaceMember } from "@/api/members";

vi.mock("@/api/members", async () => {
  const actual = await vi.importActual<typeof import("@/api/members")>("@/api/members");
  return {
    ...actual,
    useWorkspaceMembers: vi.fn(),
    useWorkspaceRoles: vi.fn(),
    useAddMember: vi.fn(),
    useRemoveMember: vi.fn(),
    useWorkspaceInvites: vi.fn(),
    useDeleteInvite: vi.fn(),
  };
});
vi.mock("@/api/hooks", async () => ({
  ...(await vi.importActual<typeof import("@/api/hooks")>("@/api/hooks")),
  useAuthConfig: () => ({ data: { oidc_enabled: true } }),
}));
vi.mock("@/auth/useAuth", () => ({
  useAuth: () => ({ me: { id: "me", email: "me@example.com", platform_roles: ["platform_admin"] } }),
}));
vi.mock("@/api/policies", () => ({ listPrincipals: vi.fn().mockResolvedValue([]) }));

import * as members from "@/api/members";

const MEMBERS: WorkspaceMember[] = [
  {
    principal_type: "user",
    principal_id: "u1",
    role: "ws_admin",
    principal_name: "alice@example.com",
    granted_by_name: null,
    added_at: "2026-01-01T00:00:00Z",
    is_platform_admin: false,
  },
  {
    principal_type: "user",
    principal_id: "u2",
    role: "ws_member",
    principal_name: "bob@example.com",
    granted_by_name: null,
    added_at: "2026-01-02T00:00:00Z",
    is_platform_admin: false,
  },
];

const removeMutateAsync = vi.fn().mockResolvedValue(undefined);
const deleteInviteMutate = vi.fn();

function renderPanel() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MembersPanel workspaceId="demo" />
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("MembersPanel", () => {
  beforeEach(() => {
    removeMutateAsync.mockClear();
    vi.mocked(members.useWorkspaceMembers).mockReturnValue({
      data: MEMBERS,
      isLoading: false,
      error: null,
    } as ReturnType<typeof members.useWorkspaceMembers>);
    vi.mocked(members.useWorkspaceRoles).mockReturnValue({
      data: [{ name: "ws_admin" }, { name: "ws_member" }],
      isLoading: false,
      error: null,
    } as ReturnType<typeof members.useWorkspaceRoles>);
    vi.mocked(members.useAddMember).mockReturnValue({
      mutate: vi.fn(),
      mutateAsync: vi.fn().mockResolvedValue(undefined),
    } as unknown as ReturnType<typeof members.useAddMember>);
    vi.mocked(members.useRemoveMember).mockReturnValue({
      mutate: vi.fn(),
      mutateAsync: removeMutateAsync,
    } as unknown as ReturnType<typeof members.useRemoveMember>);
    vi.mocked(members.useWorkspaceInvites).mockReturnValue({
      data: [],
      isLoading: false,
      error: null,
    } as unknown as ReturnType<typeof members.useWorkspaceInvites>);
    vi.mocked(members.useDeleteInvite).mockReturnValue({
      mutate: deleteInviteMutate,
    } as unknown as ReturnType<typeof members.useDeleteInvite>);
  });

  /**
   * `FilterableTable` owns the search box, so the list this panel hands it is a
   * superset of what is on screen. Targets used to be derived from that
   * superset, which meant a member selected while visible and then searched
   * away was still REMOVED FROM THE WORKSPACE — and the operator could no
   * longer see them to notice.
   *
   * Two members and a search term that separates them, so "removed the right
   * one" and "removed row 0" are different claims.
   */
  test("does not remove a member who was selected and then searched away", async () => {
    const user = userEvent.setup();
    renderPanel();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Bulk select/i }));
    const aliceRow = screen.getByText("alice@example.com").closest("tr") as HTMLElement;
    const bobRow = screen.getByText("bob@example.com").closest("tr") as HTMLElement;
    await user.click(within(aliceRow).getByRole("checkbox", { name: "Select row" }));
    await user.click(within(bobRow).getByRole("checkbox", { name: "Select row" }));
    expect(screen.getByText("2 selected")).toBeInTheDocument();

    await user.type(screen.getByPlaceholderText(/Search members/i), "alice");
    await waitFor(() => expect(screen.queryByText("bob@example.com")).toBeNull());

    // The count follows the screen, not the panel's unfiltered list.
    expect(screen.getByText("1 selected")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /^Remove$/ }));

    await waitFor(() =>
      expect(removeMutateAsync).toHaveBeenCalledWith({ principal_type: "user", principal_id: "u1" }),
    );
    // The half that matters: Bob is off screen, so Bob keeps his access.
    expect(removeMutateAsync).not.toHaveBeenCalledWith({
      principal_type: "user",
      principal_id: "u2",
    });
  });

  /**
   * KI-185: an email added before its owner's first sign-in is listed as
   * waiting, and withdrawing it asks first.
   */
  test("lists pending invitations and confirms before withdrawing one", async () => {
    vi.mocked(members.useWorkspaceInvites).mockReturnValue({
      data: [
        {
          id: "inv-1",
          email: "new@example.com",
          role: "ws_member",
          invited_by_name: "alice@example.com",
          created_at: "2026-01-03T00:00:00Z",
        },
      ],
      isLoading: false,
      error: null,
    } as unknown as ReturnType<typeof members.useWorkspaceInvites>);
    const user = userEvent.setup();
    renderPanel();

    const section = await screen.findByRole("region", { name: "Waiting for first sign-in" });
    expect(within(section).getByText("new@example.com")).toBeInTheDocument();

    await user.click(
      within(section).getByRole("button", { name: "Withdraw the invitation for new@example.com" }),
    );
    expect(deleteInviteMutate).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Withdraw" }));
    expect(deleteInviteMutate).toHaveBeenCalledWith("inv-1", expect.anything());
  });

  test("says when an added email is waiting for its first sign-in", async () => {
    const mutateAsync = vi.fn().mockResolvedValue({ ok: true, invited: true });
    vi.mocked(members.useAddMember).mockReturnValue({
      mutate: vi.fn(),
      mutateAsync,
    } as unknown as ReturnType<typeof members.useAddMember>);
    const user = userEvent.setup();
    renderPanel();

    await user.click(await screen.findByRole("button", { name: /Add member/i }));
    expect(screen.getByText(/They join when they first sign in with SSO/)).toBeInTheDocument();
    await user.type(screen.getByLabelText("User email"), "new@example.com");
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Add member" }));

    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith({
        principal_type: "user",
        principal_id: "new@example.com",
        role: "ws_admin",
      }),
    );
  });
});
