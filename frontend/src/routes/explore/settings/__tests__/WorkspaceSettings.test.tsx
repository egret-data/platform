import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route, Navigate } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

// ── Mock factories ────────────────────────────────────────────────────────────

const roleCatalogMock = vi.fn();
const sharesMock = vi.fn();
const membersMock = vi.fn();
const rolesMock = vi.fn();
const authMock = vi.fn();

vi.mock("@/api/workspaceAccess", () => ({
  useWorkspaceRoleCatalog: (...args: unknown[]) => roleCatalogMock(...args),
  useWorkspaceShares: (...args: unknown[]) => sharesMock(...args),
}));

vi.mock("@/api/members", () => ({
  membersKey: (ws: string) => ["ws", ws, "members"],
  useWorkspaceMembers: (...args: unknown[]) => membersMock(...args),
  useWorkspaceRoles: () => rolesMock(),
  useAddMember: () => ({ mutate: vi.fn(), mutateAsync: vi.fn(), isPending: false }),
  useRemoveMember: () => ({ mutate: vi.fn(), mutateAsync: vi.fn(), isPending: false }),
}));

vi.mock("@/api/policies", () => ({
  listPrincipals: vi.fn(() => Promise.resolve([])),
}));

vi.mock("@/auth/useAuth", () => ({
  useAuth: () => authMock(),
}));

vi.mock("@/components/sharing/ShareDialog", () => ({
  ShareDialog: () => null,
}));

const requestsMock = vi.fn();

vi.mock("@/api/accessRequests", () => ({
  useWorkspaceRequests: (...args: unknown[]) => requestsMock(...args),
  useApproveRequest: () => ({ mutate: vi.fn(), isPending: false }),
  useDenyRequest: () => ({ mutate: vi.fn(), isPending: false }),
}));

import WorkspaceSettings from "@/routes/explore/settings/WorkspaceSettings";
import Members from "@/routes/explore/settings/Members";
import RolesPermissions from "@/routes/explore/settings/RolesPermissions";
import SharedObjects from "@/routes/explore/settings/SharedObjects";
import AccessRequests from "@/routes/explore/settings/AccessRequests";

// ── Render helper ─────────────────────────────────────────────────────────────

function renderAt(path: string) {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path="/explore/:workspace/settings" element={<WorkspaceSettings />}>
              <Route index element={<Navigate to="members" replace />} />
              <Route path="members" element={<Members />} />
              <Route path="roles" element={<RolesPermissions />} />
              <Route path="shares" element={<SharedObjects />} />
              <Route path="requests" element={<AccessRequests />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

// ── Setup ─────────────────────────────────────────────────────────────────────

beforeEach(() => {
  roleCatalogMock.mockReset();
  sharesMock.mockReset();
  membersMock.mockReset();
  rolesMock.mockReset();
  authMock.mockReset();
  requestsMock.mockReset();

  roleCatalogMock.mockReturnValue({ data: [], isLoading: false, isError: false, error: null });
  sharesMock.mockReturnValue({ data: [], isLoading: false, isError: false, error: null });
  requestsMock.mockReturnValue({ data: [], isLoading: false, isError: false, error: null });
  membersMock.mockReturnValue({ data: [], isLoading: false, isError: false, error: null });
  rolesMock.mockReturnValue({ data: [], isLoading: false, isError: false });
  authMock.mockReturnValue({
    me: { user_id: "me-id", email: "me@egret.local", platform_roles: [], permissions: [] },
    isLoading: false,
  });
});

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("WorkspaceSettings tab navigation", () => {
  it("renders the three tab links", () => {
    renderAt("/explore/demo/settings/members");

    expect(screen.getByRole("link", { name: "Members" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Roles & permissions" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Shared objects" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Requests" })).toBeInTheDocument();
  });

  it("marks Roles & permissions active at /roles, and Members not active", () => {
    renderAt("/explore/demo/settings/roles");

    const rolesTab = screen.getByRole("link", { name: "Roles & permissions" });
    const membersTab = screen.getByRole("link", { name: "Members" });

    expect(rolesTab).toHaveAttribute("aria-current", "page");
    expect(membersTab).not.toHaveAttribute("aria-current", "page");

    // Not color-only: active tab is visually distinguished by more than color
    // (e.g. an underline/border-weight change), so assert the active class
    // differs structurally from the inactive one too.
    expect(rolesTab.className).not.toEqual(membersTab.className);

    // The Roles & permissions page itself renders.
    expect(screen.getByText(/what each workspace role can do/i)).toBeInTheDocument();
  });

  it("redirects the settings index to members", () => {
    renderAt("/explore/demo/settings");

    const membersTab = screen.getByRole("link", { name: "Members" });
    expect(membersTab).toHaveAttribute("aria-current", "page");
  });

  it("marks Shared objects active at /shares", () => {
    renderAt("/explore/demo/settings/shares");

    expect(screen.getByRole("link", { name: "Shared objects" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(screen.getByText(/everything shared inside/i)).toBeInTheDocument();
  });
});
